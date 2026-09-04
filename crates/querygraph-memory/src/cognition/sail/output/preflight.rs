//! Validate Arrow IPC size declarations before record-body allocation or decompression.

use arrow::ipc::{
    BodyCompressionMethod, CompressionType, Message, MessageHeader, RecordBatch, Type,
    root_as_message,
};

use crate::cognition::{CognitionError, budget};

const CONTINUATION_MARKER: u32 = u32::MAX;
const UTF8_COLUMNS: usize = 2;
const BUFFERS_PER_UTF8_COLUMN: usize = 3;
const COMPRESSION_PREFIX_BYTES: usize = size_of::<i64>();
const UNCOMPRESSED_BUFFER_MARKER: i64 = -1;

pub(super) fn validate_pair_streams(chunks: &[Vec<u8>]) -> Result<(), CognitionError> {
    let mut limits = DecodedLimits::default();
    for chunk in chunks {
        validate_pair_stream(chunk, &mut limits)?;
    }
    Ok(())
}

#[derive(Default)]
struct DecodedLimits {
    rows: usize,
    buffer_bytes: usize,
}

impl DecodedLimits {
    fn add_rows(&mut self, rows: usize) -> Result<(), CognitionError> {
        self.rows = self.rows.saturating_add(rows);
        budget::check_result_rows(self.rows)
    }

    fn add_buffer_bytes(&mut self, bytes: usize) -> Result<(), CognitionError> {
        self.buffer_bytes =
            self.buffer_bytes
                .checked_add(bytes)
                .ok_or(CognitionError::ResourceBudgetExceeded(
                    "decoded Arrow bytes",
                ))?;
        if self.buffer_bytes > budget::MAX_ARROW_BYTES {
            return Err(CognitionError::ResourceBudgetExceeded(
                "decoded Arrow bytes",
            ));
        }
        Ok(())
    }
}

fn validate_pair_stream(stream: &[u8], limits: &mut DecodedLimits) -> Result<(), CognitionError> {
    let mut cursor = 0;
    let mut saw_schema = false;
    while let Some(frame) = next_frame(stream, &mut cursor)? {
        match frame.message.header_type() {
            MessageHeader::Schema if !saw_schema => {
                validate_schema(frame.message, frame.body)?;
                saw_schema = true;
            }
            MessageHeader::RecordBatch if saw_schema => {
                let batch = frame
                    .message
                    .header_as_record_batch()
                    .ok_or_else(invalid_ipc)?;
                validate_record_batch(batch, frame.body, limits)?;
            }
            MessageHeader::Schema => return Err(invalid("Arrow IPC repeats its schema")),
            MessageHeader::RecordBatch => {
                return Err(invalid("Arrow IPC record batch precedes its schema"));
            }
            MessageHeader::DictionaryBatch => {
                return Err(invalid("Arrow IPC dictionaries are not valid pair output"));
            }
            _ => return Err(invalid("Arrow IPC contains an unsupported message")),
        }
    }
    if !saw_schema {
        return Err(invalid("Arrow IPC stream has no schema"));
    }
    Ok(())
}

struct Frame<'a> {
    message: Message<'a>,
    body: &'a [u8],
}

fn next_frame<'a>(
    stream: &'a [u8],
    cursor: &mut usize,
) -> Result<Option<Frame<'a>>, CognitionError> {
    if *cursor == stream.len() {
        return Ok(None);
    }

    let first = take_u32(stream, cursor)?;
    let metadata_length = if first == CONTINUATION_MARKER {
        take_u32(stream, cursor)?
    } else {
        first
    };
    if metadata_length == 0 {
        if *cursor != stream.len() {
            return Err(invalid("Arrow IPC contains bytes after its end marker"));
        }
        return Ok(None);
    }
    let metadata_length =
        usize::try_from(i32::try_from(metadata_length).map_err(|_| invalid_ipc())?)
            .map_err(|_| invalid_ipc())?;
    let metadata = take(stream, cursor, metadata_length)?;
    let message = root_as_message(metadata).map_err(|_| invalid_ipc())?;
    let body_length = usize::try_from(message.bodyLength()).map_err(|_| invalid_ipc())?;
    let body = take(stream, cursor, body_length)?;
    Ok(Some(Frame { message, body }))
}

fn take_u32(stream: &[u8], cursor: &mut usize) -> Result<u32, CognitionError> {
    let bytes: [u8; size_of::<u32>()] = take(stream, cursor, size_of::<u32>())?
        .try_into()
        .map_err(|_| invalid_ipc())?;
    Ok(u32::from_le_bytes(bytes))
}

fn take<'a>(
    stream: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], CognitionError> {
    let end = cursor.checked_add(length).ok_or_else(invalid_ipc)?;
    let bytes = stream.get(*cursor..end).ok_or_else(invalid_ipc)?;
    *cursor = end;
    Ok(bytes)
}

fn validate_schema(message: Message<'_>, body: &[u8]) -> Result<(), CognitionError> {
    if !body.is_empty() {
        return Err(invalid("Arrow IPC schema has an unexpected body"));
    }
    let schema = message.header_as_schema().ok_or_else(invalid_ipc)?;
    let fields = schema.fields().ok_or_else(invalid_ipc)?;
    if fields.len() != UTF8_COLUMNS {
        return Err(invalid("Sail result must contain exactly two columns"));
    }
    for field in fields {
        if field.type_type() != Type::Utf8
            || field.dictionary().is_some()
            || field
                .children()
                .is_some_and(|children| !children.is_empty())
        {
            return Err(invalid("Sail result columns must be plain UTF-8"));
        }
    }
    Ok(())
}

fn validate_record_batch(
    batch: RecordBatch<'_>,
    body: &[u8],
    limits: &mut DecodedLimits,
) -> Result<(), CognitionError> {
    let rows = usize::try_from(batch.length()).map_err(|_| invalid_ipc())?;
    limits.add_rows(rows)?;
    validate_nodes(batch, rows)?;

    if batch
        .variadicBufferCounts()
        .is_some_and(|counts| !counts.is_empty())
    {
        return Err(invalid("Arrow IPC pair output has variadic buffers"));
    }
    let buffers = batch.buffers().ok_or_else(invalid_ipc)?;
    if buffers.len() != UTF8_COLUMNS * BUFFERS_PER_UTF8_COLUMN {
        return Err(invalid(
            "Arrow IPC pair output has an invalid buffer layout",
        ));
    }

    let compressed = validate_compression(batch)?;
    let mut decoded_lengths = [0; UTF8_COLUMNS * BUFFERS_PER_UTF8_COLUMN];
    for (index, buffer) in buffers.iter().enumerate() {
        let offset = usize::try_from(buffer.offset()).map_err(|_| invalid_ipc())?;
        let encoded_length = usize::try_from(buffer.length()).map_err(|_| invalid_ipc())?;
        let encoded = body
            .get(offset..offset.checked_add(encoded_length).ok_or_else(invalid_ipc)?)
            .ok_or_else(invalid_ipc)?;
        let decoded_length = decoded_buffer_length(encoded, compressed)?;
        limits.add_buffer_bytes(decoded_length)?;
        decoded_lengths[index] = decoded_length;
    }
    validate_utf8_layout(batch, rows, decoded_lengths)
}

fn validate_nodes(batch: RecordBatch<'_>, rows: usize) -> Result<(), CognitionError> {
    let nodes = batch.nodes().ok_or_else(invalid_ipc)?;
    if nodes.len() != UTF8_COLUMNS {
        return Err(invalid("Arrow IPC pair output has an invalid field layout"));
    }
    for node in nodes {
        let length = usize::try_from(node.length()).map_err(|_| invalid_ipc())?;
        let null_count = usize::try_from(node.null_count()).map_err(|_| invalid_ipc())?;
        if length != rows || null_count > rows {
            return Err(invalid("Arrow IPC pair output has invalid field lengths"));
        }
    }
    Ok(())
}

fn validate_compression(batch: RecordBatch<'_>) -> Result<bool, CognitionError> {
    let Some(compression) = batch.compression() else {
        return Ok(false);
    };
    if compression.method() != BodyCompressionMethod::BUFFER
        || !matches!(
            compression.codec(),
            CompressionType::LZ4_FRAME | CompressionType::ZSTD
        )
    {
        return Err(invalid("Arrow IPC uses unsupported compression"));
    }
    Ok(true)
}

fn decoded_buffer_length(encoded: &[u8], compressed: bool) -> Result<usize, CognitionError> {
    if !compressed || encoded.is_empty() {
        return Ok(encoded.len());
    }
    let prefix = encoded
        .get(..COMPRESSION_PREFIX_BYTES)
        .ok_or_else(invalid_ipc)?;
    let declared = i64::from_le_bytes(prefix.try_into().map_err(|_| invalid_ipc())?);
    match declared {
        UNCOMPRESSED_BUFFER_MARKER => Ok(encoded.len() - COMPRESSION_PREFIX_BYTES),
        0.. => usize::try_from(declared).map_err(|_| invalid_ipc()),
        _ => Err(invalid_ipc()),
    }
}

fn validate_utf8_layout(
    batch: RecordBatch<'_>,
    rows: usize,
    decoded_lengths: [usize; UTF8_COLUMNS * BUFFERS_PER_UTF8_COLUMN],
) -> Result<(), CognitionError> {
    let offset_bytes = rows
        .checked_add(1)
        .and_then(|count| count.checked_mul(size_of::<i32>()))
        .ok_or_else(invalid_ipc)?;
    let validity_bytes = rows.div_ceil(8);
    let nodes = batch.nodes().ok_or_else(invalid_ipc)?;
    for column in 0..UTF8_COLUMNS {
        let base = column * BUFFERS_PER_UTF8_COLUMN;
        if decoded_lengths[base + 1] < offset_bytes {
            return Err(invalid("Arrow IPC UTF-8 offsets are truncated"));
        }
        if nodes.get(column).null_count() > 0 && decoded_lengths[base] < validity_bytes {
            return Err(invalid("Arrow IPC validity bitmap is truncated"));
        }
    }
    Ok(())
}

fn invalid_ipc() -> CognitionError {
    invalid("Sail returned invalid Arrow IPC")
}

fn invalid(message: &'static str) -> CognitionError {
    CognitionError::Sail(message)
}

#[cfg(test)]
mod tests;
