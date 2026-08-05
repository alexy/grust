use std::path::{Path, PathBuf};

use grust_core::{GrustError, Result};

/// Ownership policy for Sail's managed-table warehouse.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SailWarehouse {
    /// Leave `spark.sql.warehouse.dir` untouched and use the server's catalog
    /// and warehouse configuration.
    #[default]
    ServerManaged,
    /// Use a session-scoped directory on the client's local filesystem.
    ///
    /// The path is deterministic from `SailConfig::session_id` and is only
    /// suitable when Sail shares the client's filesystem namespace. Grust does
    /// not delete it; the caller owns cleanup.
    LocalSessionScoped,
    /// Set an absolute path that the Sail server resolves to stable storage.
    ExplicitPath(PathBuf),
}

/// Spark Connect session configuration for the Sail graph store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SailConfig {
    pub endpoint: String,
    pub user_id: String,
    pub session_id: String,
    pub batch_size: usize,
    /// How the Spark Connect session obtains its managed-table warehouse.
    pub warehouse: SailWarehouse,
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
        self.warehouse_override()?;
        Ok(())
    }

    pub(crate) fn warehouse_override(&self) -> Result<Option<String>> {
        let path = match &self.warehouse {
            SailWarehouse::ServerManaged => return Ok(None),
            SailWarehouse::LocalSessionScoped => local_session_path(&self.session_id),
            SailWarehouse::ExplicitPath(path) => path.clone(),
        };
        Ok(Some(warehouse_path(&path)?.to_string()))
    }
}

impl Default for SailConfig {
    fn default() -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            user_id: "grust".to_string(),
            warehouse: SailWarehouse::ServerManaged,
            session_id,
            batch_size: 1000,
        }
    }
}

fn local_session_path(session_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("grust-sail")
        .join(session_id)
        .join("warehouse")
}

fn warehouse_path(path: &Path) -> Result<&str> {
    if !path.is_absolute() {
        return Err(invalid_config(
            "warehouse override must be an absolute path",
        ));
    }
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or_else(|| invalid_config("warehouse override must be valid UTF-8 without NUL bytes"))
}

fn invalid_config(message: &str) -> GrustError {
    GrustError::Backend(format!("invalid Sail configuration: {message}"))
}

#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
