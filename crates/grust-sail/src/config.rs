use std::path::{Path, PathBuf};

use grust_core::{GrustError, Result};

/// Spark Connect session configuration for the Sail graph store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SailConfig {
    pub endpoint: String,
    pub user_id: String,
    pub session_id: String,
    pub batch_size: usize,
    /// Absolute path on the Sail server used for session-managed tables.
    pub warehouse_dir: PathBuf,
}

impl SailConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.endpoint.trim().is_empty() {
            return Err(invalid_config("endpoint must not be empty"));
        }
        if self.user_id.trim().is_empty() {
            return Err(invalid_config("user_id must not be empty"));
        }
        uuid::Uuid::parse_str(&self.session_id)
            .map_err(|_| invalid_config("session_id must be a UUID"))?;
        if self.batch_size == 0 {
            return Err(invalid_config("batch_size must be greater than zero"));
        }
        warehouse_path(&self.warehouse_dir)?;
        Ok(())
    }

    pub(crate) fn warehouse_value(&self) -> Result<&str> {
        warehouse_path(&self.warehouse_dir)
    }
}

impl Default for SailConfig {
    fn default() -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            user_id: "grust".to_string(),
            warehouse_dir: default_warehouse_dir(&session_id),
            session_id,
            batch_size: 1000,
        }
    }
}

fn default_warehouse_dir(session_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("grust-sail")
        .join(session_id)
        .join("warehouse")
}

fn warehouse_path(path: &Path) -> Result<&str> {
    if !path.is_absolute() {
        return Err(invalid_config("warehouse_dir must be an absolute path"));
    }
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| invalid_config("warehouse_dir must be valid UTF-8 without NUL bytes"))
}

fn invalid_config(message: &str) -> GrustError {
    GrustError::Backend(format!("invalid Sail configuration: {message}"))
}

#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
