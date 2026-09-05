//! Explicit asynchronous cleanup of a caller-owned Spark Connect session.
use grust_core::{GrustError, Result};

use crate::{CLIENT_TYPE, SailConfig, SailGraphStore, sail_user_context, sc};

impl SailGraphStore {
    /// Consume this client and ask Sail to release its remote session state.
    ///
    /// This invalidates session temporary views and other clients sharing the
    /// configured session ID. It does not delete durable tables or warehouse
    /// files. Call only after this session's operations have finished. Dropping
    /// the Rust client alone cannot perform asynchronous server cleanup.
    ///
    /// An acknowledgement is not a proof that a previously interrupted query
    /// has stopped. Callers requiring a hard cleanup deadline must bound this
    /// future and treat a timeout or error as an uncertain release.
    pub async fn close(mut self) -> Result<()> {
        let response = self
            .client
            .release_session(request(&self.config))
            .await
            .map_err(|_| GrustError::Backend("release Sail session failed".to_string()))?
            .into_inner();
        verify_response(&response, &self.config.session_id)
    }
}

fn request(config: &SailConfig) -> sc::ReleaseSessionRequest {
    sc::ReleaseSessionRequest {
        session_id: config.session_id.clone(),
        user_context: Some(sail_user_context(config)),
        client_type: Some(CLIENT_TYPE.to_string()),
        allow_reconnect: false,
    }
}

fn verify_response(response: &sc::ReleaseSessionResponse, session_id: &str) -> Result<()> {
    if response.session_id != session_id || response.server_side_session_id.is_empty() {
        return Err(GrustError::Backend(
            "Sail session release acknowledgement is invalid".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_binds_the_owned_session_and_disallows_reconnect() {
        let config = SailConfig::default();
        let request = request(&config);
        assert_eq!(request.session_id, config.session_id);
        assert_eq!(request.user_context, Some(sail_user_context(&config)));
        assert!(!request.allow_reconnect);
        assert_eq!(request.client_type.as_deref(), Some(CLIENT_TYPE));
    }

    #[test]
    fn acknowledgement_must_identify_the_requested_session() {
        let mut response = sc::ReleaseSessionResponse {
            session_id: "owned".into(),
            server_side_session_id: "server-owned".into(),
        };
        assert!(verify_response(&response, "owned").is_ok());
        assert!(verify_response(&response, "another").is_err());
        response.server_side_session_id.clear();
        assert!(verify_response(&response, "owned").is_err());
    }
}
