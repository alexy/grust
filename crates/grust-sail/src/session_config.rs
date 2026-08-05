use grust_core::{GrustError, Result};

use crate::config::SailConfig;
use crate::sc::spark_connect_service_client::SparkConnectServiceClient;
use crate::sc::{
    ConfigRequest, ConfigResponse, KeyValue, config_request, config_request::operation::OpType,
};
use crate::{CLIENT_TYPE, sail_user_context};

pub(crate) const WAREHOUSE_CONFIG_KEY: &str = "spark.sql.warehouse.dir";

struct WarehouseConfiguration {
    expected: String,
    set_request: ConfigRequest,
}

pub(crate) async fn configure_warehouse(
    client: &mut SparkConnectServiceClient<tonic::transport::Channel>,
    config: &SailConfig,
) -> Result<()> {
    let Some(configuration) = warehouse_configuration(config)? else {
        return Ok(());
    };
    let set = client
        .config(configuration.set_request)
        .await
        .map_err(|error| rpc_error("set", error))?
        .into_inner();
    let server_session_id = verify_session(&set, config, None)?;

    let get = client
        .config(get_request(config, server_session_id.clone()))
        .await
        .map_err(|error| rpc_error("get", error))?
        .into_inner();
    verify_warehouse(&get, config, &server_session_id, &configuration.expected)
}

fn warehouse_configuration(config: &SailConfig) -> Result<Option<WarehouseConfiguration>> {
    let Some(expected) = config.warehouse_override()? else {
        return Ok(None);
    };
    Ok(Some(WarehouseConfiguration {
        set_request: set_request(config, &expected),
        expected,
    }))
}

fn set_request(config: &SailConfig, warehouse: &str) -> ConfigRequest {
    request(
        config,
        None,
        OpType::Set(config_request::Set {
            pairs: vec![KeyValue {
                key: WAREHOUSE_CONFIG_KEY.to_string(),
                value: Some(warehouse.to_string()),
            }],
            silent: Some(false),
        }),
    )
}

fn get_request(config: &SailConfig, server_session_id: String) -> ConfigRequest {
    request(
        config,
        Some(server_session_id),
        OpType::Get(config_request::Get {
            keys: vec![WAREHOUSE_CONFIG_KEY.to_string()],
        }),
    )
}

fn request(
    config: &SailConfig,
    server_session_id: Option<String>,
    op_type: OpType,
) -> ConfigRequest {
    ConfigRequest {
        session_id: config.session_id.clone(),
        client_observed_server_side_session_id: server_session_id,
        user_context: Some(sail_user_context(config)),
        operation: Some(config_request::Operation {
            op_type: Some(op_type),
        }),
        client_type: Some(CLIENT_TYPE.to_string()),
    }
}

fn verify_warehouse(
    response: &ConfigResponse,
    config: &SailConfig,
    server_session_id: &str,
    expected: &str,
) -> Result<()> {
    verify_session(response, config, Some(server_session_id))?;
    match response.pairs.as_slice() {
        [KeyValue { key, value }]
            if key == WAREHOUSE_CONFIG_KEY && value.as_deref() == Some(expected) =>
        {
            Ok(())
        }
        _ => Err(GrustError::Backend(format!(
            "Sail did not retain the configured {WAREHOUSE_CONFIG_KEY} value"
        ))),
    }
}

fn verify_session(
    response: &ConfigResponse,
    config: &SailConfig,
    expected_server_session_id: Option<&str>,
) -> Result<String> {
    if response.session_id != config.session_id {
        return Err(GrustError::Backend(
            "Sail Config RPC returned a different client session".to_string(),
        ));
    }
    if response.server_side_session_id.is_empty() {
        return Err(GrustError::Backend(
            "Sail Config RPC returned an empty server session identifier".to_string(),
        ));
    }
    if expected_server_session_id.is_some_and(|id| id != response.server_side_session_id) {
        return Err(GrustError::Backend(
            "Sail Config RPC changed server session during configuration".to_string(),
        ));
    }
    Ok(response.server_side_session_id.clone())
}

fn rpc_error(operation: &str, error: tonic::Status) -> GrustError {
    GrustError::Backend(format!(
        "Sail Config RPC failed to {operation} {WAREHOUSE_CONFIG_KEY}: {error}"
    ))
}

#[cfg(test)]
#[path = "session_config/tests.rs"]
mod tests;
