use crate::auth::validate_session_token;

pub fn authenticate_request(token: &str) -> bool {
    validate_session_token(token)
}
