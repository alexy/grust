//! Coordinator-owned Sail session, borrowed by fresh observation processes.
use std::ops::Deref;
use std::process::Command;

use grust_sail::{SailConfig, SailGraphStore, SailWarehouse};

pub const SESSION_ENV: &str = "GRUST_LSQB_SAIL_OWNED_SESSION";

pub struct Session {
    store: SailGraphStore,
    config: SailConfig,
    owner: bool,
}

impl Session {
    pub fn owned(store: SailGraphStore, config: SailConfig) -> Self {
        Self {
            store,
            config,
            owner: true,
        }
    }

    pub async fn borrow() -> Result<Self, String> {
        let config = SailConfig {
            endpoint: std::env::var("SAIL_ENDPOINT").map_err(|_| "Sail worker endpoint missing")?,
            session_id: std::env::var(SESSION_ENV)
                .map_err(|_| "Sail coordinator session missing")?,
            warehouse: SailWarehouse::ServerManaged,
            ..SailConfig::default()
        };
        let store = SailGraphStore::connect(config.clone())
            .await
            .map_err(|_| "attach to owned Sail session failed")?;
        Ok(Self {
            store,
            config,
            owner: false,
        })
    }

    pub fn configure_worker(&self, command: &mut Command) {
        // Explicit child environment only: never mutate the Tokio process env.
        command.env(SESSION_ENV, &self.config.session_id);
    }

    pub async fn close(self) -> Result<(), String> {
        if self.owner {
            tokio::time::timeout(std::time::Duration::from_secs(30), self.store.close())
                .await
                .map_err(|_| "Sail coordinator session release deadline")?
                .map_err(|_| "release Sail coordinator session failed".to_string())?;
        }
        Ok(())
    }
}

impl Deref for Session {
    type Target = SailGraphStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}
