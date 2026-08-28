// Synthetic lexical noise for retrieval evaluation. These names deliberately overlap
// broad repository vocabulary without defining the target implementation symbols.

pub fn session_token_audit_catalog() -> &'static str { "session token audit metadata" }
pub fn session_token_rotation_catalog() -> &'static str { "session token rotation metadata" }
pub fn session_token_expiry_catalog() -> &'static str { "session token expiry metadata" }
pub fn authentication_request_catalog() -> &'static str { "authentication request routing metadata" }
pub fn authentication_policy_catalog() -> &'static str { "authentication policy metadata" }
pub fn route_dispatch_catalog() -> &'static str { "route dispatch metadata" }
pub fn route_dispatcher_catalog() -> &'static str { "route dispatcher metadata" }
pub fn account_service_catalog() -> &'static str { "account service metadata" }
pub fn account_load_catalog() -> &'static str { "account load metadata" }
pub fn token_validator_catalog() -> &'static str { "token validator metadata" }
pub fn packet_header_catalog() -> &'static str { "packet header metadata" }
pub fn packet_normalize_catalog() -> &'static str { "packet normalize metadata" }
pub fn native_cache_catalog() -> &'static str { "native cache metadata" }
pub fn cache_probe_catalog() -> &'static str { "cache probe metadata" }

pub const GENERATED_AUTH_TERMS: &[&str] = &[
    "session token authentication request policy audit",
    "session token authentication request policy rotation",
    "session token authentication request policy expiry",
    "session token authentication request policy metadata",
    "route dispatcher dispatch routing metadata",
    "route dispatcher dispatch routing archive",
    "account service load account metadata",
    "account service load account archive",
    "token validator validate archive token metadata",
    "packet header normalize archive metadata",
    "native cache probe archive metadata",
    "repository context search ranking noise",
    "repository context search ranking generated",
    "repository context search ranking catalog",
    "repository context search ranking metadata",
];
