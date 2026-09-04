use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow::ipc::{CompressionType, MessageHeader, root_as_message};
use arrow::record_batch::RecordBatch;

use super::*;

#[test]
fn compressed_expansion_budget_accepts_the_boundary_and_rejects_one_more_byte() {
    let empty = compressed_pair_stream(0);
    let mut empty_limits = DecodedLimits::default();
    validate_pair_stream(&empty, &mut empty_limits).expect("empty compressed pair");
    let fixed_decoded_bytes = empty_limits.buffer_bytes;
    drop(empty);

    let boundary_values = budget::MAX_ARROW_BYTES - fixed_decoded_bytes;
    let boundary = compressed_pair_stream(boundary_values);
    assert!(boundary.len() < budget::MAX_ARROW_BYTES);

    let mut limits = DecodedLimits::default();
    validate_pair_stream(&boundary, &mut limits).expect("inclusive decoded-byte boundary");
    assert_eq!(limits.buffer_bytes, budget::MAX_ARROW_BYTES);
    drop(boundary);

    let excessive = compressed_pair_stream(boundary_values + 1);
    assert!(excessive.len() < budget::MAX_ARROW_BYTES);
    let error = validate_pair_stream(&excessive, &mut DecodedLimits::default())
        .expect_err("compressed expansion must be rejected before decoding");
    assert!(matches!(
        error,
        CognitionError::ResourceBudgetExceeded("decoded Arrow bytes")
    ));
}

#[test]
fn a_declared_body_larger_than_the_available_stream_is_rejected() {
    let mut stream = pair_stream("left", "right", None);
    let (body_start, body_length) = first_record_body(&stream);
    assert!(body_length > 0);
    stream.truncate(body_start + body_length - 1);

    let error = validate_pair_stream(&stream, &mut DecodedLimits::default())
        .expect_err("truncated declared body");
    assert!(matches!(
        error,
        CognitionError::Sail("Sail returned invalid Arrow IPC")
    ));
}

#[test]
fn a_small_compressed_pair_stream_passes_preflight_and_decoding() {
    let stream = pair_stream("left", "right", Some(CompressionType::ZSTD));
    validate_pair_stream(&stream, &mut DecodedLimits::default()).expect("safe preflight");

    let rows = super::super::decode_pair_chunks(vec![stream]).expect("compressed pair decode");
    assert_eq!(rows, vec![("left".into(), "right".into())]);
}

#[test]
fn declared_rows_are_bounded_before_record_batch_decoding() {
    let rows = budget::MAX_RESULT_ROWS + 1;
    let left = Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
        "left", rows,
    )));
    let right = Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
        "right", rows,
    )));
    let stream = pair_stream_from_columns(left, right, None);

    let error = validate_pair_stream(&stream, &mut DecodedLimits::default())
        .expect_err("declared row count must be bounded before decoding");
    assert!(matches!(
        error,
        CognitionError::ResourceBudgetExceeded("Sail result rows")
    ));
}

fn compressed_pair_stream(total_value_bytes: usize) -> Vec<u8> {
    let left_bytes = total_value_bytes / 2;
    let right_bytes = total_value_bytes - left_bytes;
    let left = "x".repeat(left_bytes);
    let right = "y".repeat(right_bytes);
    pair_stream(&left, &right, Some(CompressionType::LZ4_FRAME))
}

fn pair_stream(left: &str, right: &str, compression: Option<CompressionType>) -> Vec<u8> {
    pair_stream_from_columns(
        Arc::new(StringArray::from(vec![left])),
        Arc::new(StringArray::from(vec![right])),
        compression,
    )
}

fn pair_stream_from_columns(
    left: Arc<StringArray>,
    right: Arc<StringArray>,
    compression: Option<CompressionType>,
) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("left", DataType::Utf8, false),
        Field::new("right", DataType::Utf8, false),
    ]));
    let columns: Vec<ArrayRef> = vec![left, right];
    let batch = RecordBatch::try_new(Arc::clone(&schema), columns).expect("record batch");
    let options = IpcWriteOptions::default()
        .try_with_compression(compression)
        .expect("supported compression");
    let mut stream = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new_with_options(&mut stream, &schema, options).expect("IPC writer");
        writer.write(&batch).expect("write IPC batch");
        writer.finish().expect("finish IPC stream");
    }
    stream
}

fn first_record_body(stream: &[u8]) -> (usize, usize) {
    let mut cursor = 0;
    loop {
        let first = read_u32(stream, &mut cursor);
        let metadata_length = if first == CONTINUATION_MARKER {
            read_u32(stream, &mut cursor)
        } else {
            first
        } as usize;
        assert_ne!(metadata_length, 0, "record batch message");
        let metadata_end = cursor + metadata_length;
        let message = root_as_message(&stream[cursor..metadata_end]).expect("message metadata");
        cursor = metadata_end;
        let body_length = usize::try_from(message.bodyLength()).expect("body length");
        if message.header_type() == MessageHeader::RecordBatch {
            return (cursor, body_length);
        }
        cursor += body_length;
    }
}

fn read_u32(stream: &[u8], cursor: &mut usize) -> u32 {
    let end = *cursor + size_of::<u32>();
    let value = u32::from_le_bytes(stream[*cursor..end].try_into().expect("u32"));
    *cursor = end;
    value
}
