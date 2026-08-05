use std::path::PathBuf;

use super::*;

#[test]
fn default_config_leaves_warehouse_server_managed() {
    let config = SailConfig::default();

    assert_eq!(config.warehouse, SailWarehouse::ServerManaged);
    assert_eq!(config.warehouse_override().expect("warehouse policy"), None);
    config.validate().expect("valid default Sail config");
}

#[test]
fn local_warehouse_is_absolute_and_session_scoped() {
    let config = SailConfig {
        warehouse: SailWarehouse::LocalSessionScoped,
        ..SailConfig::default()
    };

    let warehouse = config
        .warehouse_override()
        .expect("valid local warehouse")
        .expect("local override");
    assert!(PathBuf::from(&warehouse).is_absolute());
    assert!(warehouse.contains(&config.session_id));
    assert!(warehouse.ends_with("warehouse"));

    let reopened = SailConfig {
        session_id: config.session_id,
        warehouse: SailWarehouse::LocalSessionScoped,
        ..SailConfig::default()
    };
    assert_eq!(
        reopened
            .warehouse_override()
            .expect("valid reopened local warehouse"),
        Some(warehouse)
    );
}

#[test]
fn explicit_warehouse_requires_an_absolute_path() {
    let config = SailConfig {
        warehouse: SailWarehouse::ExplicitPath(PathBuf::from("spark-warehouse")),
        ..SailConfig::default()
    };

    let error = config.validate().expect_err("relative warehouse rejected");
    assert!(
        error
            .to_string()
            .contains("warehouse override must be an absolute path")
    );
}

#[test]
fn explicit_warehouse_preserves_the_server_visible_path() {
    let config = SailConfig {
        warehouse: SailWarehouse::ExplicitPath(PathBuf::from("/srv/sail/grust")),
        ..SailConfig::default()
    };

    assert_eq!(
        config.warehouse_override().expect("valid warehouse"),
        Some("/srv/sail/grust".to_string())
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
