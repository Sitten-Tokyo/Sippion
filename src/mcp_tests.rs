use super::*;
use std::io::Cursor;

struct PanicService;

impl RepositoryService for PanicService {
    fn context(
        &self,
        _query: &crate::core::NormalizedQuery,
        _coordination: Option<&CoordinationContext>,
        _cancellation: Option<&AtomicBool>,
    ) -> Result<String, crate::service::RepositoryServiceError> {
        panic!("repository service must not be called")
    }
}

struct StaticService;

impl RepositoryService for StaticService {
    fn context(
        &self,
        query: &crate::core::NormalizedQuery,
        coordination: Option<&CoordinationContext>,
        _cancellation: Option<&AtomicBool>,
    ) -> Result<String, crate::service::RepositoryServiceError> {
        let agent = coordination
            .and_then(|context| context.agent_id.as_deref())
            .unwrap_or("none");
        Ok(format!("service:{}:{agent}", query.terms.join(",")))
    }
}

#[test]
fn oversized_stdio_frame_is_drained_without_retaining_it() {
    let mut bytes = vec![b'x'; MAX_MCP_REQUEST_BYTES + 1];
    bytes.extend_from_slice(b"\n{}\n");
    let mut cursor = Cursor::new(bytes);
    assert!(matches!(read_frame(&mut cursor).unwrap(), Frame::TooLarge));
    assert!(matches!(read_frame(&mut cursor).unwrap(), Frame::Data(data) if data == b"{}"));
}

#[test]
fn modern_tools_list_has_required_cache_fields() {
    let params = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN_MCP_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    let result = list_tools(&params, ProtocolMode::Modern).unwrap();
    assert_eq!(result["resultType"], "complete");
    assert!(result["ttlMs"].is_number());
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "sippion"
    );
}

#[test]
fn modern_tool_result_advertises_server_info() {
    let result = tool_result(ProtocolMode::Modern, false, "ok");
    assert_eq!(result["resultType"], "complete");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["version"],
        crate::core::VERSION
    );
}

#[test]
fn context_guidance_switches_to_native_tools_after_narrowing() {
    let result = discover_result();
    let instructions = result["instructions"].as_str().expect("instructions");
    assert!(instructions.contains("Use native file reads only after narrowing candidates"));
    assert!(instructions.contains("repo_context"));
}

#[test]
fn legacy_initialize_negotiates_to_the_supported_legacy_revision() {
    let initialized = AtomicBool::new(false);
    let params = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}
    });
    let result = legacy_initialize(&params, &initialized).expect("legacy version negotiation");
    assert_eq!(result["protocolVersion"], LEGACY_MCP_VERSION);
    assert!(initialized.load(AtomicOrdering::Acquire));
}

#[test]
fn discovery_advertises_all_supported_protocol_versions() {
    let result = discover_result();
    assert_eq!(
        result["supportedVersions"],
        json!([MODERN_MCP_VERSION, LEGACY_MCP_VERSION])
    );
}

#[test]
fn legacy_requests_require_initialize_without_blocking_modern_requests() {
    let initialized = AtomicBool::new(false);
    let error = validate_bound_protocol(&json!({}), false, &initialized)
        .expect_err("legacy request before initialize must fail");
    assert_eq!(error.code, -32002);
    assert!(!initialized.load(AtomicOrdering::Acquire));

    let invalid = json!({
        "protocolVersion": LEGACY_MCP_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "test"}
    });
    assert!(legacy_initialize(&invalid, &initialized).is_err());
    assert!(!initialized.load(AtomicOrdering::Acquire));

    let valid = json!({
        "protocolVersion": LEGACY_MCP_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}
    });
    let result = legacy_initialize(&valid, &initialized).expect("valid initialize");
    assert_eq!(result["protocolVersion"], LEGACY_MCP_VERSION);
    assert!(initialized.load(AtomicOrdering::Acquire));
    assert_eq!(
        validate_bound_protocol(&json!({}), false, &initialized).expect("legacy request"),
        ProtocolMode::Legacy
    );

    let modern = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN_MCP_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    assert_eq!(
        validate_bound_protocol(&modern, false, &initialized).expect("modern request"),
        ProtocolMode::Modern
    );
}

#[test]
fn modern_requests_do_not_create_connection_protocol_state() {
    let initialized = AtomicBool::new(false);
    let modern = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN_MCP_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    assert_eq!(
        validate_bound_protocol(&modern, false, &initialized).expect("modern request"),
        ProtocolMode::Modern
    );
    assert!(!initialized.load(AtomicOrdering::Acquire));

    let legacy = json!({
        "protocolVersion": LEGACY_MCP_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}
    });
    legacy_initialize(&legacy, &initialized).expect("legacy compatibility can initialize later");
    assert!(initialized.load(AtomicOrdering::Acquire));
    assert_eq!(
        validate_bound_protocol(&modern, false, &initialized).expect("modern remains stateless"),
        ProtocolMode::Modern
    );
}

#[test]
fn tool_schema_validation_errors_are_model_visible_execution_errors() {
    let params = json!({
        "name": "repo_context",
        "arguments": {"unexpected": true}
    });
    let service = PanicService;
    let limiter = ToolRateLimiter::default();
    let result =
        call_tool(&service, &params, None, &limiter, ProtocolMode::Legacy).expect("tool result");
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("expected"))
    );
}

#[test]
fn invalid_query_shape_is_rejected_before_rate_budget_or_repository_scan() {
    let params = json!({
        "name": "repo_context",
        "arguments": {"q": "the"}
    });
    let service = PanicService;
    let limiter = ToolRateLimiter::default();
    let result =
        call_tool(&service, &params, None, &limiter, ProtocolMode::Legacy).expect("tool result");
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("1-8"))
    );
    assert_eq!(limiter.state.lock().expect("rate limiter").global.len(), 0);
}

#[test]
fn valid_tool_call_uses_repository_service_boundary() {
    let params = json!({
        "name": "repo_context",
        "arguments": {
            "q": "Authentication Middleware",
            "session_id": "task-1",
            "agent_id": "tests"
        }
    });
    let service = StaticService;
    let limiter = ToolRateLimiter::default();
    let result =
        call_tool(&service, &params, None, &limiter, ProtocolMode::Legacy).expect("tool result");
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["content"][0]["text"].as_str(),
        Some("service:authentication,middleware:tests")
    );
}

#[test]
fn json_rpc_tools_call_reaches_local_repository_service() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-main-service-{nonce}"));
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("service.rs"), "fn service_boundary_marker() {}\n").expect("source");

    let service = LocalRepositoryService::open_with_scan_budget(&root, MAX_SCAN_BYTES)
        .expect("open local repository service");
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_MCP_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {}
            },
            "name": "repo_context",
            "arguments": {"q": "service_boundary_marker"}
        }
    });
    let mut output = Vec::new();
    handle_request(
        &service,
        &request,
        None,
        &ToolRateLimiter::default(),
        &AtomicBool::new(false),
        &mut output,
    )
    .expect("JSON-RPC request");

    let response: Value = serde_json::from_slice(&output).expect("JSON-RPC response");
    assert_eq!(response["result"]["isError"], false);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("service_boundary_marker"))
    );

    drop(service);
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn structurally_invalid_tool_arguments_remain_protocol_errors() {
    let params = json!({
        "name": "repo_context",
        "arguments": "not-an-object"
    });
    let service = PanicService;
    let limiter = ToolRateLimiter::default();
    let error = call_tool(&service, &params, None, &limiter, ProtocolMode::Legacy)
        .expect_err("protocol error");
    assert_eq!(error.code, -32602);
}

#[test]
fn unsupported_modern_version_uses_spec_error_shape() {
    let params = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2099-01-01",
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    let error = validate_protocol(&params, false).expect_err("unsupported version");
    assert_eq!(error.code, -32022);
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data["requested"].as_str()),
        Some("2099-01-01")
    );
    assert_eq!(
        error.data.as_ref().map(|data| data["supported"].clone()),
        Some(json!([MODERN_MCP_VERSION, LEGACY_MCP_VERSION]))
    );
}

#[test]
fn modern_request_rejects_missing_client_capabilities() {
    let params = json!({
        "_meta": {"io.modelcontextprotocol/protocolVersion": MODERN_MCP_VERSION}
    });
    assert!(validate_protocol(&params, false).is_err());
}

#[test]
fn sequential_tool_calls_are_rate_limited_per_actor_and_globally() {
    let limiter = ToolRateLimiter::default();
    let now = Instant::now();
    for _ in 0..MAX_ACTOR_TOOL_CALLS_PER_WINDOW {
        assert!(limiter.try_acquire_at("session/a", now));
    }
    assert!(!limiter.try_acquire_at("session/a", now));
    assert!(limiter.try_acquire_at("session/b", now));
    assert!(limiter.try_acquire_at("session/a", now + TOOL_RATE_WINDOW));

    let global = ToolRateLimiter::default();
    for index in 0..MAX_GLOBAL_TOOL_CALLS_PER_WINDOW {
        assert!(global.try_acquire_at(&format!("actor-{index}"), now));
    }
    assert!(!global.try_acquire_at("overflow", now));
}

#[test]
fn cancellation_notification_marks_matching_inflight_request() {
    let request = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-1".to_string(),
        Arc::clone(&request),
    )])));
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": "req-1"}
    });

    assert!(handle_cancellation_notification(&notification, &inflight));
    assert!(request.cancellation().load(AtomicOrdering::Acquire));
    assert_eq!(
        request.terminal_state.load(AtomicOrdering::Acquire),
        INFLIGHT_CANCELLED
    );
}

#[test]
fn cancelled_async_response_is_never_written() {
    let request = Arc::new(InflightRequest::new());
    assert!(request.try_cancel());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-2".to_string(),
        Arc::clone(&request),
    )])));
    let writer = Arc::new(Mutex::new(Vec::<u8>::new()));

    finish_async_response(
        &writer,
        &inflight,
        "s:req-2",
        &request,
        true,
        b"should-not-be-written",
    )
    .expect("cancelled response completion");

    assert!(writer.lock().expect("writer").is_empty());
    assert!(!inflight.lock().expect("inflight").contains_key("s:req-2"));
}

#[test]
fn async_response_write_error_is_propagated_and_registration_is_removed() {
    struct PartialFailWriter {
        writes: usize,
    }

    impl Write for PartialFailWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes = self.writes.saturating_add(1);
            if self.writes == 1 && !bytes.is_empty() {
                Ok(1)
            } else {
                Err(io::Error::other("simulated stdout failure"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let request = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-write-fail".to_string(),
        Arc::clone(&request),
    )])));
    let writer = Arc::new(Mutex::new(PartialFailWriter { writes: 0 }));

    let error = finish_async_response(
        &writer,
        &inflight,
        "s:req-write-fail",
        &request,
        true,
        b"response",
    )
    .expect_err("partial stdout failure must be surfaced");

    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(
        !inflight
            .lock()
            .expect("inflight")
            .contains_key("s:req-write-fail")
    );
}

#[test]
fn cancellation_while_waiting_for_stdout_suppresses_response() {
    let request = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-race".to_string(),
        Arc::clone(&request),
    )])));
    let writer = Arc::new(Mutex::new(Vec::<u8>::new()));

    let held_writer = writer.lock().expect("writer");
    let worker_writer = Arc::clone(&writer);
    let worker_inflight = Arc::clone(&inflight);
    let worker_request = Arc::clone(&request);
    let worker = std::thread::spawn(move || {
        finish_async_response(
            &worker_writer,
            &worker_inflight,
            "s:req-race",
            &worker_request,
            true,
            b"must-be-suppressed",
        )
        .expect("cancelled response completion");
    });

    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": "req-race"}
    });
    assert!(handle_cancellation_notification(&notification, &inflight));
    assert!(request.cancellation().load(AtomicOrdering::Acquire));
    drop(held_writer);

    worker.join().expect("response worker");
    assert!(writer.lock().expect("writer").is_empty());
    assert!(
        !inflight
            .lock()
            .expect("inflight")
            .contains_key("s:req-race")
    );
}

#[test]
fn blocked_response_writer_does_not_hold_inflight_lock() {
    use std::sync::mpsc;

    struct BlockingWriter {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.started
                .send(())
                .map_err(|_| io::Error::other("test start channel closed"))?;
            self.release
                .recv()
                .map_err(|_| io::Error::other("test release channel closed"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let request = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-blocked".to_string(),
        Arc::clone(&request),
    )])));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writer = Arc::new(Mutex::new(BlockingWriter {
        started: started_tx,
        release: release_rx,
    }));

    let worker_writer = Arc::clone(&writer);
    let worker_inflight = Arc::clone(&inflight);
    let worker_request = Arc::clone(&request);
    let worker = std::thread::spawn(move || {
        finish_async_response(
            &worker_writer,
            &worker_inflight,
            "s:req-blocked",
            &worker_request,
            true,
            b"response",
        )
        .expect("blocked response completion");
    });

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer reached blocking write");
    assert!(
        inflight.try_lock().is_ok(),
        "in-flight lock must be released before response I/O"
    );
    assert!(
        inflight
            .lock()
            .expect("inflight")
            .contains_key("s:req-blocked"),
        "response-pending work must remain counted toward the in-flight cap"
    );

    release_tx.send(()).expect("release writer");
    worker.join().expect("response worker");
    assert!(
        !inflight
            .lock()
            .expect("inflight")
            .contains_key("s:req-blocked"),
        "completed response must release its in-flight registration"
    );
}

#[test]
fn inflight_guard_removes_registration_during_unwind() {
    let request = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-panic".to_string(),
        Arc::clone(&request),
    )])));

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
        let inflight = Arc::clone(&inflight);
        let request = Arc::clone(&request);
        move || {
            let _guard = InflightGuard::new(inflight, "s:req-panic".to_string(), request);
            panic!("simulated worker panic");
        }
    }));

    assert!(unwind.is_err());
    assert!(
        !inflight
            .lock()
            .expect("inflight")
            .contains_key("s:req-panic")
    );
}

#[test]
fn inflight_guard_does_not_remove_reused_request_id() {
    let original = Arc::new(InflightRequest::new());
    let replacement = Arc::new(InflightRequest::new());
    let inflight = Arc::new(Mutex::new(HashMap::from([(
        "s:req-reused".to_string(),
        Arc::clone(&original),
    )])));
    let guard = InflightGuard::new(Arc::clone(&inflight), "s:req-reused".to_string(), original);

    inflight
        .lock()
        .expect("inflight")
        .insert("s:req-reused".to_string(), Arc::clone(&replacement));
    drop(guard);

    let active = inflight.lock().expect("inflight");
    assert!(
        active
            .get("s:req-reused")
            .is_some_and(|registered| Arc::ptr_eq(registered, &replacement))
    );
}

#[test]
fn tool_call_ids_are_typed_for_inflight_tracking() {
    assert_eq!(request_id_key(&json!(7)).as_deref(), Some("n:7"));
    assert_eq!(request_id_key(&json!("7")).as_deref(), Some("s:7"));
    assert_ne!(request_id_key(&json!(7)), request_id_key(&json!("7")));
}

#[test]
fn fractional_request_ids_are_supported() {
    assert!(request_id_key(&json!(1.5)).is_some());
    assert!(request_id_key(&json!(-0.25)).is_some());
    assert_ne!(request_id_key(&json!(1.5)), request_id_key(&json!(-0.25)));
}

#[test]
fn fractional_request_id_is_preserved_in_response() {
    let service = PanicService;
    let limiter = ToolRateLimiter::default();
    let initialized = AtomicBool::new(false);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1.5,
        "method": "ping"
    });
    let mut output = Vec::new();

    handle_request(
        &service,
        &request,
        None,
        &limiter,
        &initialized,
        &mut output,
    )
    .expect("write response");

    let response: Value = serde_json::from_slice(&output).expect("valid JSON response");
    assert_eq!(response["id"], json!(1.5));
    assert_ne!(response["error"]["message"], "invalid request id");
}

#[test]
fn integer_request_ids_accept_signed_and_unsigned_ranges() {
    assert_eq!(request_id_key(&json!(-7)).as_deref(), Some("n:-7"));
    assert_eq!(
        request_id_key(&json!(u64::MAX)).as_deref(),
        Some("n:18446744073709551615")
    );
}
