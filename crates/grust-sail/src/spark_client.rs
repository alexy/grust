use tonic::transport::Channel;

use super::SparkConnectServiceClient;

const MEBIBYTE: usize = 1024 * 1024;

/// Largest Arrow IPC payload accepted from one bounded Spark Connect result.
///
/// Consumers that perform stricter domain-specific decoding should reuse this
/// value rather than defining a second transport budget.
pub const MAX_ARROW_IPC_PAYLOAD_BYTES: usize = 16 * MEBIBYTE;

/// Bounded room for the `ExecutePlanResponse` protobuf envelope and metadata.
pub const SPARK_CONNECT_PROTOBUF_HEADROOM_BYTES: usize = MEBIBYTE;

/// Tonic applies this limit to the whole decoded protobuf message, not only the
/// nested Arrow byte field.
pub const MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES: usize =
    MAX_ARROW_IPC_PAYLOAD_BYTES + SPARK_CONNECT_PROTOBUF_HEADROOM_BYTES;
const _: () = assert!(MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES < usize::MAX);

pub(super) fn with_decoding_limit(
    client: SparkConnectServiceClient<Channel>,
) -> SparkConnectServiceClient<Channel> {
    client.max_decoding_message_size(MAX_SPARK_CONNECT_DECODING_MESSAGE_BYTES)
}

#[cfg(test)]
#[path = "spark_client/tests.rs"]
mod tests;
