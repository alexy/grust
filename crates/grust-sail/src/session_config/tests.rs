use std::path::PathBuf;

use crate::SailWarehouse;

use super::*;

fn config() -> SailConfig {
    SailConfig {
        session_id: "00112233-4455-6677-8899-aabbccddeeff".to_string(),
        warehouse: SailWarehouse::ExplicitPath(PathBuf::from("/tmp/grust-sail/test/warehouse")),
        ..SailConfig::default()
    }
}

#[test]
fn server_managed_policy_builds_no_config_rpc() {
    let config = SailConfig::default();

    assert!(
        warehouse_configuration(&config)
            .expect("valid server-managed policy")
            .is_none()
    );
}

#[test]
fn set_then_get_transcript_binds_one_session() {
    let config = config();
    let configuration = warehouse_configuration(&config)
        .expect("valid warehouse policy")
        .expect("explicit warehouse configuration");
    assert_eq!(configuration.expected, "/tmp/grust-sail/test/warehouse");
    let set = configuration.set_request;
    assert_eq!(set.session_id, config.session_id);
    assert_eq!(set.client_observed_server_side_session_id, None);
    let OpType::Set(set_op) = set
        .operation
        .expect("operation")
        .op_type
        .expect("operation type")
    else {
        panic!("expected set operation");
    };
    assert_eq!(
        set_op.pairs,
        vec![KeyValue {
            key: WAREHOUSE_CONFIG_KEY.to_string(),
            value: Some("/tmp/grust-sail/test/warehouse".to_string()),
        }]
    );

    let get = get_request(&config, "server-session".to_string());
    assert_eq!(
        get.client_observed_server_side_session_id.as_deref(),
        Some("server-session")
    );
    assert!(matches!(
        get.operation.expect("operation").op_type,
        Some(OpType::Get(config_request::Get { keys }))
            if keys == vec![WAREHOUSE_CONFIG_KEY.to_string()]
    ));
}

#[test]
fn verification_requires_exact_session_and_warehouse() {
    let config = config();
    let valid = ConfigResponse {
        session_id: config.session_id.clone(),
        server_side_session_id: "server-session".to_string(),
        pairs: vec![KeyValue {
            key: WAREHOUSE_CONFIG_KEY.to_string(),
            value: Some("/tmp/grust-sail/test/warehouse".to_string()),
        }],
        warnings: vec![],
    };
    verify_warehouse(
        &valid,
        &config,
        "server-session",
        "/tmp/grust-sail/test/warehouse",
    )
    .expect("exact response accepted");

    let wrong_path = ConfigResponse {
        pairs: vec![KeyValue {
            key: WAREHOUSE_CONFIG_KEY.to_string(),
            value: Some("/tmp/other".to_string()),
        }],
        ..valid.clone()
    };
    assert!(
        verify_warehouse(
            &wrong_path,
            &config,
            "server-session",
            "/tmp/grust-sail/test/warehouse",
        )
        .is_err()
    );

    let wrong_session = ConfigResponse {
        server_side_session_id: "replacement-session".to_string(),
        ..valid
    };
    assert!(
        verify_warehouse(
            &wrong_session,
            &config,
            "server-session",
            "/tmp/grust-sail/test/warehouse",
        )
        .is_err()
    );
}
