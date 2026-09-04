use prost::Message as _;

use super::{
    MAX_ARROW_IPC_PAYLOAD_BYTES, MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES,
    SPARK_CONNECT_PROTOBUF_HEADROOM_BYTES,
};
use crate::sc::{ExecutePlanResponse, execute_plan_response};

#[test]
fn decoding_limit_includes_a_bounded_protobuf_envelope_reserve() {
    let response = ExecutePlanResponse {
        session_id: uuid_string(),
        server_side_session_id: uuid_string(),
        operation_id: uuid_string(),
        response_id: uuid_string(),
        metrics: None,
        observed_metrics: Vec::new(),
        schema: None,
        response_type: Some(execute_plan_response::ResponseType::ArrowBatch(
            execute_plan_response::ArrowBatch {
                row_count: i64::MAX,
                data: vec![0; MAX_ARROW_IPC_PAYLOAD_BYTES],
                start_offset: Some(i64::MAX),
                chunk_index: Some(i64::MAX),
                num_chunks_in_batch: Some(i64::MAX),
            },
        )),
    };

    assert!(response.encoded_len() > MAX_ARROW_IPC_PAYLOAD_BYTES);
    assert!(response.encoded_len() <= MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES);
    assert_eq!(
        MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES - MAX_ARROW_IPC_PAYLOAD_BYTES,
        SPARK_CONNECT_PROTOBUF_HEADROOM_BYTES
    );
}

fn uuid_string() -> String {
    "00112233-4455-6677-8899-aabbccddeeff".to_string()
}
