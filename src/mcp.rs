use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::{
    CoordinationContext, InputError, McpToolInput, capability_registry, mcp_tool_definitions,
};
use crate::service::RepositoryService;
#[cfg(test)]
use crate::service::{LocalRepositoryService, MAX_SCAN_BYTES};
use serde_json::{Value, json};

const MODERN_MCP_VERSION: &str = "2026-07-28";
const LEGACY_MCP_VERSION: &str = "2025-11-25";
const MAX_MCP_REQUEST_BYTES: usize = 256 * 1024;
const STATIC_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_INFLIGHT_TOOL_CALLS: usize = 4;
const MAX_GLOBAL_TOOL_CALLS_PER_WINDOW: usize = 24;
const MAX_ACTOR_TOOL_CALLS_PER_WINDOW: usize = 8;
const TOOL_RATE_WINDOW: Duration = Duration::from_secs(60);
const SERVER_INSTRUCTIONS: &str = "For repository-wide code discovery, use Sippion before broad recursive search or reading many files. Cooperating subagents should share session_id and use distinct agent_id values so Sippion can reuse structural analysis and diversify overlapping results. Use native file reads only after narrowing candidates. Sippion exposes one local/read-only tool, repo_context; skip it when the exact path/string is already known.";

const INFLIGHT_PENDING: u8 = 0;
const INFLIGHT_CANCELLED: u8 = 1;
const INFLIGHT_RESPONSE_COMMITTED: u8 = 2;

#[derive(Debug)]
struct InflightRequest {
    cancellation: AtomicBool,
    terminal_state: AtomicU8,
}

impl InflightRequest {
    fn new() -> Self {
        Self {
            cancellation: AtomicBool::new(false),
            terminal_state: AtomicU8::new(INFLIGHT_PENDING),
        }
    }

    fn cancellation(&self) -> &AtomicBool {
        &self.cancellation
    }

    fn try_cancel(&self) -> bool {
        if self
            .terminal_state
            .compare_exchange(
                INFLIGHT_PENDING,
                INFLIGHT_CANCELLED,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
        {
            self.cancellation.store(true, AtomicOrdering::Release);
            true
        } else {
            false
        }
    }

    fn try_commit_response(&self) -> bool {
        self.terminal_state
            .compare_exchange(
                INFLIGHT_PENDING,
                INFLIGHT_RESPONSE_COMMITTED,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
    }
}

type InflightRequests = Arc<Mutex<HashMap<String, Arc<InflightRequest>>>>;

#[derive(Debug)]
struct InflightGuard {
    inflight: InflightRequests,
    request_key: String,
    request: Arc<InflightRequest>,
}

impl InflightGuard {
    fn new(inflight: InflightRequests, request_key: String, request: Arc<InflightRequest>) -> Self {
        Self {
            inflight,
            request_key,
            request,
        }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        // This is the panic/unwind fallback. Normal completion removes the entry after response
        // commit/write without holding the in-flight lock during I/O. Only remove our own
        // registration: the same request id may already have been reused.
        remove_inflight_registration(&self.inflight, &self.request_key, &self.request);
    }
}

#[derive(Debug, Default)]
struct RateState {
    global: VecDeque<Instant>,
    actors: HashMap<String, VecDeque<Instant>>,
}

#[derive(Debug, Default)]
struct ToolRateLimiter {
    state: Mutex<RateState>,
}

impl ToolRateLimiter {
    fn try_acquire(&self, actor_key: &str) -> bool {
        self.try_acquire_at(actor_key, Instant::now())
    }

    fn try_acquire_at(&self, actor_key: &str, now: Instant) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        while state
            .global
            .front()
            .is_some_and(|started| now.saturating_duration_since(*started) >= TOOL_RATE_WINDOW)
        {
            state.global.pop_front();
        }
        state.actors.retain(|_, actor| {
            while actor
                .front()
                .is_some_and(|started| now.saturating_duration_since(*started) >= TOOL_RATE_WINDOW)
            {
                actor.pop_front();
            }
            !actor.is_empty()
        });
        let actor_len = state.actors.get(actor_key).map_or(0, VecDeque::len);
        if state.global.len() >= MAX_GLOBAL_TOOL_CALLS_PER_WINDOW
            || actor_len >= MAX_ACTOR_TOOL_CALLS_PER_WINDOW
        {
            return false;
        }
        state.global.push_back(now);
        state
            .actors
            .entry(actor_key.to_string())
            .or_default()
            .push_back(now);
        true
    }
}

fn rate_actor_key(context: &CoordinationContext) -> String {
    match (context.session_id.as_deref(), context.agent_id.as_deref()) {
        (Some(session), Some(agent)) => format!("{session}/{agent}"),
        (Some(session), None) => format!("{session}/__parent__"),
        (None, Some(agent)) => format!("__default__/{agent}"),
        (None, None) => "__legacy__".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolMode {
    Legacy,
    Modern,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: &'static str,
    data: Option<Value>,
}

impl RpcError {
    const fn new(code: i64, message: &'static str) -> Self {
        Self {
            code,
            message,
            data: None,
        }
    }

    fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

enum Frame {
    Eof,
    Data(Vec<u8>),
    TooLarge,
}

pub(crate) fn serve_stdio(service: Arc<dyn RepositoryService>) -> io::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let writer = Arc::new(Mutex::new(io::stdout()));
    let inflight: InflightRequests = Arc::new(Mutex::new(HashMap::new()));
    let rate_limiter = Arc::new(ToolRateLimiter::default());
    let legacy_initialized = Arc::new(AtomicBool::new(false));

    loop {
        match read_frame(&mut reader)? {
            Frame::Eof => return Ok(()),
            Frame::TooLarge => with_shared_writer(&writer, |out| {
                write_rpc_error(
                    out,
                    Value::Null,
                    &RpcError::new(-32600, "request exceeds local size limit"),
                )
            })?,
            Frame::Data(frame) => {
                if frame.iter().all(|byte| byte.is_ascii_whitespace()) {
                    continue;
                }
                let request = match serde_json::from_slice::<Value>(&frame) {
                    Ok(request) => request,
                    Err(_) => {
                        with_shared_writer(&writer, |out| {
                            write_rpc_error(out, Value::Null, &RpcError::new(-32700, "parse error"))
                        })?;
                        continue;
                    }
                };

                if handle_cancellation_notification(&request, &inflight) {
                    continue;
                }

                if let Some((request_id, request_key)) = async_tool_call_id(&request) {
                    let null_params = Value::Null;
                    let params = request.get("params").unwrap_or(&null_params);
                    if let Err(error) = validate_bound_protocol(params, false, &legacy_initialized)
                    {
                        with_shared_writer(&writer, |out| {
                            write_rpc_error(out, request_id.clone(), &error)
                        })?;
                        continue;
                    }
                    let inflight_request = Arc::new(InflightRequest::new());
                    let registration_error = {
                        let mut active = inflight
                            .lock()
                            .map_err(|_| io::Error::other("in-flight state poisoned"))?;
                        if active.contains_key(&request_key) {
                            Some(RpcError::new(-32600, "duplicate in-flight request id"))
                        } else if active.len() >= MAX_INFLIGHT_TOOL_CALLS {
                            Some(RpcError::new(-32603, "too many in-flight tool calls"))
                        } else {
                            active.insert(request_key.clone(), Arc::clone(&inflight_request));
                            None
                        }
                    };
                    if let Some(error) = registration_error {
                        with_shared_writer(&writer, |out| {
                            write_rpc_error(out, request_id.clone(), &error)
                        })?;
                        continue;
                    }

                    let worker_service = Arc::clone(&service);
                    let worker_writer = Arc::clone(&writer);
                    let worker_inflight = Arc::clone(&inflight);
                    let worker_rate_limiter = Arc::clone(&rate_limiter);
                    let worker_legacy_initialized = Arc::clone(&legacy_initialized);
                    let worker_request_key = request_key.clone();
                    let worker_inflight_request = Arc::clone(&inflight_request);
                    let spawn_result = std::thread::Builder::new()
                        .name("sippion-tool-call".to_string())
                        .spawn(move || {
                            let _inflight_guard = InflightGuard::new(
                                Arc::clone(&worker_inflight),
                                worker_request_key.clone(),
                                Arc::clone(&worker_inflight_request),
                            );
                            let mut response = Vec::new();
                            let completed = handle_request(
                                worker_service.as_ref(),
                                &request,
                                Some(worker_inflight_request.cancellation()),
                                &worker_rate_limiter,
                                &worker_legacy_initialized,
                                &mut response,
                            )
                            .is_ok();

                            if let Err(error) = finish_async_response(
                                &worker_writer,
                                &worker_inflight,
                                &worker_request_key,
                                &worker_inflight_request,
                                completed,
                                &response,
                            ) {
                                // A partial/failed stdout write can corrupt the newline-delimited
                                // JSON-RPC stream. Continuing could make every later response
                                // undecodable, so fail the process instead of silently proceeding.
                                eprintln!("Sippion: fatal stdout MCP failure: {error}");
                                std::process::exit(2);
                            }
                        });
                    if spawn_result.is_err() {
                        remove_inflight_registration(&inflight, &request_key, &inflight_request);
                        with_shared_writer(&writer, |out| {
                            write_rpc_error(
                                out,
                                request_id,
                                &RpcError::new(-32603, "cannot start tool worker"),
                            )
                        })?;
                    }
                    continue;
                }

                with_shared_writer(&writer, |out| {
                    handle_request(
                        service.as_ref(),
                        &request,
                        None,
                        &rate_limiter,
                        &legacy_initialized,
                        out,
                    )
                })?;
            }
        }
    }
}

fn with_shared_writer<F>(writer: &Arc<Mutex<io::Stdout>>, operation: F) -> io::Result<()>
where
    F: FnOnce(&mut io::Stdout) -> io::Result<()>,
{
    let mut out = writer
        .lock()
        .map_err(|_| io::Error::other("stdout state poisoned"))?;
    operation(&mut out)
}

fn request_id_key(id: &Value) -> Option<String> {
    if let Some(value) = id.as_str() {
        return Some(format!("s:{value}"));
    }
    let number = id.as_number()?;
    if let Some(value) = number.as_i64() {
        return Some(format!("n:{value}"));
    }
    if let Some(value) = number.as_u64() {
        return Some(format!("n:{value}"));
    }
    // JSON-RPC permits numeric ids and only discourages fractional values. Keep fractional ids
    // usable for response correlation/cancellation while preserving their numeric identity.
    number
        .as_f64()
        .filter(|value| value.is_finite())
        .map(|value| format!("f:{:016x}", value.to_bits()))
}

fn async_tool_call_id(request: &Value) -> Option<(Value, String)> {
    let object = request.as_object()?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str) != Some("tools/call")
    {
        return None;
    }
    let id = object.get("id")?;
    let key = request_id_key(id)?;
    Some((id.clone(), key))
}

fn handle_cancellation_notification(request: &Value, inflight: &InflightRequests) -> bool {
    let Some(object) = request.as_object() else {
        return false;
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("method").and_then(Value::as_str) != Some("notifications/cancelled")
        || object.contains_key("id")
    {
        return false;
    }

    let request_id = object
        .get("params")
        .and_then(|params| params.get("requestId"));
    let Some(key) = request_id.and_then(request_id_key) else {
        return true;
    };
    let request = inflight
        .lock()
        .ok()
        .and_then(|active| active.get(&key).cloned());
    if let Some(request) = request {
        // Cancellation and response commitment race on one atomic state transition. Whichever
        // wins first is final: cancellation that wins suppresses the response; a cancellation
        // arriving after processing has committed its response is safely ignored.
        request.try_cancel();
    }
    true
}

fn remove_inflight_registration(
    inflight: &InflightRequests,
    request_key: &str,
    request: &Arc<InflightRequest>,
) -> bool {
    let mut active = match inflight.lock() {
        Ok(active) => active,
        Err(poisoned) => poisoned.into_inner(),
    };
    if active
        .get(request_key)
        .is_some_and(|registered| Arc::ptr_eq(registered, request))
    {
        active.remove(request_key);
        true
    } else {
        false
    }
}

fn finish_async_response<W: Write>(
    writer: &Arc<Mutex<W>>,
    inflight: &InflightRequests,
    request_key: &str,
    request: &Arc<InflightRequest>,
    completed: bool,
    response: &[u8],
) -> io::Result<()> {
    // First verify that this worker still owns the registration. Then atomically arbitrate
    // completion against cancellation without holding the in-flight lock during stdout I/O.
    // This removes the check-then-write cancellation race: only one transition out of PENDING
    // can win. The registration remains present until I/O finishes, so blocked stdout still counts
    // toward MAX_INFLIGHT_TOOL_CALLS and request ids cannot be reused prematurely.
    let still_registered = {
        let active = match inflight.lock() {
            Ok(active) => active,
            Err(poisoned) => poisoned.into_inner(),
        };
        active
            .get(request_key)
            .is_some_and(|registered| Arc::ptr_eq(registered, request))
    };

    let write_result = if still_registered && completed {
        // Wait for the shared stdout writer before committing the response. While queued here, a
        // cancellation can still win the terminal-state CAS and suppress output. Once this worker
        // owns stdout, committing immediately before write is the narrowest practical boundary.
        match writer.lock() {
            Ok(mut out) => {
                if request.try_commit_response() {
                    out.write_all(response).and_then(|_| out.flush())
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(io::Error::other("stdout state poisoned")),
        }
    } else {
        Ok(())
    };
    remove_inflight_registration(inflight, request_key, request);
    write_result
}

fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Frame> {
    let mut collected = Vec::new();
    let mut too_large = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if collected.is_empty() && !too_large {
                return Ok(Frame::Eof);
            }
            if too_large {
                return Ok(Frame::TooLarge);
            }
            if collected.last() == Some(&b'\r') {
                collected.pop();
            }
            return Ok(Frame::Data(collected));
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let payload_len = newline.unwrap_or(available.len());
        if !too_large {
            if collected.len().saturating_add(payload_len) > MAX_MCP_REQUEST_BYTES {
                too_large = true;
            } else {
                collected.extend_from_slice(&available[..payload_len]);
            }
        }

        let consume_len = newline.map_or(available.len(), |position| position + 1);
        reader.consume(consume_len);
        if newline.is_some() {
            if too_large {
                return Ok(Frame::TooLarge);
            }
            if collected.last() == Some(&b'\r') {
                collected.pop();
            }
            return Ok(Frame::Data(collected));
        }
    }
}

fn handle_request<W: Write>(
    service: &dyn RepositoryService,
    request: &Value,
    cancellation: Option<&AtomicBool>,
    rate_limiter: &ToolRateLimiter,
    legacy_initialized: &AtomicBool,
    writer: &mut W,
) -> io::Result<()> {
    let Some(object) = request.as_object() else {
        return write_rpc_error(
            writer,
            Value::Null,
            &RpcError::new(-32600, "invalid request"),
        );
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return write_rpc_error(
            writer,
            object.get("id").cloned().unwrap_or(Value::Null),
            &RpcError::new(-32600, "invalid JSON-RPC version"),
        );
    }

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return write_rpc_error(
            writer,
            object.get("id").cloned().unwrap_or(Value::Null),
            &RpcError::new(-32600, "missing method"),
        );
    };
    let id = object.get("id").cloned();

    // Notifications never receive a response. The only legacy notification we need is the completed
    // initialize signal; unknown notifications can be ignored under JSON-RPC semantics.
    let Some(id) = id else {
        return Ok(());
    };
    if request_id_key(&id).is_none() {
        return write_rpc_error(
            writer,
            Value::Null,
            &RpcError::new(-32600, "invalid request id"),
        );
    }

    let null_params = Value::Null;
    let params = object.get("params").unwrap_or(&null_params);
    let result = match method {
        "server/discover" => match validate_bound_protocol(params, true, legacy_initialized) {
            Ok(ProtocolMode::Modern) => Ok(discover_result()),
            Ok(ProtocolMode::Legacy) => {
                Err(RpcError::new(-32602, "modern MCP metadata required"))
            }
            Err(error) => Err(error),
        },
        "initialize" => legacy_initialize(params, legacy_initialized),
        "ping" => match validate_bound_protocol(params, false, legacy_initialized) {
            Ok(ProtocolMode::Legacy) => Ok(json!({})),
            Ok(ProtocolMode::Modern) => Err(RpcError::new(-32601, "method not found")),
            Err(error) => Err(error),
        },
        "tools/list" => validate_bound_protocol(params, false, legacy_initialized)
            .and_then(|mode| list_tools(params, mode)),
        "tools/call" => validate_bound_protocol(params, false, legacy_initialized)
            .and_then(|mode| call_tool(service, params, cancellation, rate_limiter, mode)),
        _ => Err(RpcError::new(-32601, "method not found")),
    };

    match result {
        Ok(result) => write_rpc_result(writer, id, result),
        Err(error) => write_rpc_error(writer, id, &error),
    }
}

fn validate_protocol(params: &Value, modern_required: bool) -> Result<ProtocolMode, RpcError> {
    let meta = params.get("_meta");
    let protocol = meta
        .and_then(|value| value.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str);

    let Some(protocol) = protocol else {
        if modern_required {
            return Err(RpcError::new(-32602, "missing MCP request metadata"));
        }
        return Ok(ProtocolMode::Legacy);
    };
    if protocol != MODERN_MCP_VERSION {
        return Err(
            RpcError::new(-32022, "unsupported MCP protocol version").with_data(json!({
                "supported": [MODERN_MCP_VERSION, LEGACY_MCP_VERSION],
                "requested": protocol
            })),
        );
    }

    let has_capabilities = meta
        .and_then(|value| value.get("io.modelcontextprotocol/clientCapabilities"))
        .is_some_and(Value::is_object);
    if !has_capabilities {
        return Err(RpcError::new(-32602, "missing MCP client capabilities"));
    }
    if let Some(client_info) =
        meta.and_then(|value| value.get("io.modelcontextprotocol/clientInfo"))
    {
        let Some(client_info) = client_info.as_object() else {
            return Err(RpcError::new(-32602, "invalid MCP client info"));
        };
        if !client_info.get("name").is_some_and(Value::is_string)
            || !client_info.get("version").is_some_and(Value::is_string)
        {
            return Err(RpcError::new(-32602, "invalid MCP client info"));
        }
    }
    Ok(ProtocolMode::Modern)
}

fn validate_bound_protocol(
    params: &Value,
    modern_required: bool,
    legacy_initialized: &AtomicBool,
) -> Result<ProtocolMode, RpcError> {
    let requested = validate_protocol(params, modern_required)?;
    match requested {
        // Modern MCP is stateless: protocol version and capabilities come from this request's
        // _meta and must never be inferred from, or blocked by, earlier requests on this process.
        ProtocolMode::Modern => Ok(ProtocolMode::Modern),
        ProtocolMode::Legacy if legacy_initialized.load(AtomicOrdering::Acquire) => {
            Ok(ProtocolMode::Legacy)
        }
        ProtocolMode::Legacy => Err(RpcError::new(-32002, "legacy MCP initialize required")),
    }
}

fn validate_legacy_initialize(params: &Value) -> Result<(), RpcError> {
    let Some(object) = params.as_object() else {
        return Err(RpcError::new(-32602, "initialize params must be an object"));
    };
    let Some(protocol) = object.get("protocolVersion").and_then(Value::as_str) else {
        return Err(RpcError::new(-32602, "missing legacy MCP protocolVersion"));
    };
    if protocol.is_empty() {
        return Err(RpcError::new(-32602, "invalid legacy MCP protocolVersion"));
    }
    if !object.get("capabilities").is_some_and(Value::is_object) {
        return Err(RpcError::new(
            -32602,
            "missing legacy MCP client capabilities",
        ));
    }
    let Some(client_info) = object.get("clientInfo").and_then(Value::as_object) else {
        return Err(RpcError::new(-32602, "missing legacy MCP client info"));
    };
    if !client_info.get("name").is_some_and(Value::is_string)
        || !client_info.get("version").is_some_and(Value::is_string)
    {
        return Err(RpcError::new(-32602, "invalid legacy MCP client info"));
    }
    Ok(())
}

fn bind_legacy_initialize(legacy_initialized: &AtomicBool) {
    // Legacy compatibility keeps only the handshake state legacy requests actually require. It
    // never binds or downgrades modern requests, which are self-contained by protocol design.
    legacy_initialized.store(true, AtomicOrdering::Release);
}

fn legacy_initialize(params: &Value, legacy_initialized: &AtomicBool) -> Result<Value, RpcError> {
    validate_legacy_initialize(params)?;
    bind_legacy_initialize(legacy_initialized);
    Ok(legacy_initialize_result())
}

fn modern_server_meta() -> Value {
    json!({
        "io.modelcontextprotocol/serverInfo": {
            "name": "sippion",
            "version": crate::core::VERSION
        }
    })
}

fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": [MODERN_MCP_VERSION, LEGACY_MCP_VERSION],
        "capabilities": {"tools": {}},
        "_meta": {
            "io.modelcontextprotocol/serverInfo": {
                "name": "sippion",
                "version": crate::core::VERSION
            },
            "io.sippion/capabilityRegistry": capability_registry()
        },
        "instructions": SERVER_INSTRUCTIONS,
        "ttlMs": STATIC_CACHE_TTL_MS,
        "cacheScope": "public"
    })
}

fn legacy_initialize_result() -> Value {
    json!({
        "protocolVersion": LEGACY_MCP_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "sippion", "version": crate::core::VERSION},
        "instructions": SERVER_INSTRUCTIONS
    })
}

fn list_tools(params: &Value, mode: ProtocolMode) -> Result<Value, RpcError> {
    if params.get("cursor").is_some() {
        return Err(RpcError::new(-32602, "pagination cursor not supported"));
    }
    let tools = mcp_tool_definitions();
    Ok(match mode {
        ProtocolMode::Legacy => json!({"tools": tools}),
        ProtocolMode::Modern => json!({
            "resultType": "complete",
            "tools": tools,
            "_meta": modern_server_meta(),
            "ttlMs": STATIC_CACHE_TTL_MS,
            "cacheScope": "public"
        }),
    })
}

fn call_tool(
    service: &dyn RepositoryService,
    params: &Value,
    cancellation: Option<&AtomicBool>,
    rate_limiter: &ToolRateLimiter,
    mode: ProtocolMode,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(-32602, "missing tool name"))?;
    if name != "repo_context" {
        return Err(RpcError::new(-32602, "unknown tool"));
    }

    let arguments = match params.get("arguments") {
        Some(value) if !value.is_object() => {
            return Err(RpcError::new(-32602, "tool arguments must be an object"));
        }
        Some(value) => value.clone(),
        None => json!({}),
    };
    let input = match serde_json::from_value::<McpToolInput>(arguments) {
        Ok(input) => input,
        Err(_) => {
            return Ok(tool_result(
                mode,
                true,
                "invalid tool arguments; expected {\"q\":\"1-8 technical search terms\",\"session_id\":\"optional\",\"agent_id\":\"optional\"}",
            ));
        }
    };
    let query = match input.normalize() {
        Ok(query) => query,
        Err(error) => return Ok(tool_result(mode, true, input_error_message(error))),
    };
    let coordination = match input.coordination() {
        Ok(coordination) => coordination,
        Err(error) => return Ok(tool_result(mode, true, input_error_message(error))),
    };
    let actor_key = rate_actor_key(&coordination);
    if !rate_limiter.try_acquire(&actor_key) {
        return Ok(tool_result(
            mode,
            true,
            "Sippion tool rate limit reached (8 calls/60s per agent, 24 calls/60s process-wide); narrow the query or reduce parallel fan-out",
        ));
    }

    let text = service.context(&query, Some(&coordination), cancellation);
    match text {
        Ok(text) => Ok(tool_result(mode, false, &text)),
        Err(error) => Ok(tool_result(mode, true, error.user_message())),
    }
}

fn tool_result(mode: ProtocolMode, is_error: bool, text: &str) -> Value {
    match mode {
        ProtocolMode::Legacy => json!({
            "content": [{"type": "text", "text": text}],
            "isError": is_error
        }),
        ProtocolMode::Modern => json!({
            "resultType": "complete",
            "content": [{"type": "text", "text": text}],
            "isError": is_error,
            "_meta": modern_server_meta()
        }),
    }
}

const fn input_error_message(error: InputError) -> &'static str {
    match error {
        InputError::EmptyQuery => "q must not be empty",
        InputError::QueryTooLong => "q exceeds 512 UTF-8 bytes",
        InputError::TooFewTerms => {
            "q must contain 1-8 distinct technical terms; provide at least one likely identifier/term"
        }
        InputError::TooManyTerms => {
            "q must contain 1-8 distinct technical terms; remove prose or narrow the query"
        }
        InputError::InvalidSessionId => {
            "session_id must be 1-96 bytes and use only ASCII letters, digits, '.', '_', ':', '-' with an alphanumeric first character"
        }
        InputError::InvalidAgentId => {
            "agent_id must be 1-96 bytes and use only ASCII letters, digits, '.', '_', ':', '-' with an alphanumeric first character"
        }
    }
}

fn write_rpc_result<W: Write>(writer: &mut W, id: Value, result: Value) -> io::Result<()> {
    write_json_line(
        writer,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
}

fn write_rpc_error<W: Write>(writer: &mut W, id: Value, error: &RpcError) -> io::Result<()> {
    let mut payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": error.code, "message": error.message}
    });
    if let Some(data) = &error.data {
        payload["error"]["data"] = data.clone();
    }
    write_json_line(writer, &payload)
}

fn write_json_line<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
