use std::path::PathBuf;

use super::*;

#[test]
fn default_config_has_session_scoped_absolute_warehouse() {
    let config = SailConfig::default();

    assert!(config.warehouse_dir.is_absolute());
    assert!(config.warehouse_dir.ends_with("warehouse"));
    assert!(
        config
            .warehouse_dir
            .to_string_lossy()
            .contains(&config.session_id)
    );
    config.validate().expect("valid default Sail config");
}

#[test]
fn config_rejects_relative_warehouse() {
    let config = SailConfig {
        warehouse_dir: PathBuf::from("spark-warehouse"),
        ..SailConfig::default()
    };

    let error = config.validate().expect_err("relative warehouse rejected");
    assert!(
        error
            .to_string()
            .contains("warehouse_dir must be an absolute path")
    );
}

#[test]
fn config_rejects_invalid_session_and_zero_batch_size() {
    let invalid_session = SailConfig {
        session_id: "not-a-uuid".to_string(),
        ..SailConfig::default()
    };
    assert!(
        invalid_session
            .validate()
            .expect_err("invalid session rejected")
            .to_string()
            .contains("session_id must be a UUID")
    );

    let zero_batch = SailConfig {
        batch_size: 0,
        ..SailConfig::default()
    };
    assert!(
        zero_batch
            .validate()
            .expect_err("zero batch size rejected")
            .to_string()
            .contains("batch_size must be greater than zero")
    );
}
