use grust_core::GrustError;

use super::{ARROW_IPC_BYTES, ARROW_IPC_CHUNKS, ArrowIpcCollector, ArrowIpcLimits};

fn bounded(max_chunks: usize, max_bytes: usize) -> ArrowIpcCollector {
    ArrowIpcCollector::new(Some(ArrowIpcLimits {
        max_chunks,
        max_bytes,
    }))
}

#[test]
fn exact_chunk_and_byte_limits_are_accepted() {
    let mut collector = bounded(2, 5);

    collector.push(b"ab".to_vec()).unwrap();
    collector.push(b"cde".to_vec()).unwrap();

    assert_eq!(
        collector.into_chunks(),
        vec![b"ab".to_vec(), b"cde".to_vec()]
    );
}

#[test]
fn excessive_chunk_is_rejected_before_collection() {
    let mut collector = bounded(1, 16);
    collector.push(b"first".to_vec()).unwrap();

    let error = collector.push(b"second".to_vec()).unwrap_err();

    assert!(matches!(
        error,
        GrustError::ResourceLimitExceeded {
            resource: ARROW_IPC_CHUNKS,
            limit: 1,
            observed: 2,
        }
    ));
    assert_eq!(collector.into_chunks(), vec![b"first".to_vec()]);
}

#[test]
fn excessive_cumulative_bytes_are_rejected_before_collection() {
    let mut collector = bounded(3, 5);
    collector.push(b"abc".to_vec()).unwrap();

    let error = collector.push(b"def".to_vec()).unwrap_err();

    assert!(matches!(
        error,
        GrustError::ResourceLimitExceeded {
            resource: ARROW_IPC_BYTES,
            limit: 5,
            observed: 6,
        }
    ));
    assert_eq!(collector.into_chunks(), vec![b"abc".to_vec()]);
}

#[test]
fn cumulative_byte_overflow_is_rejected_even_at_the_maximum_limit() {
    let mut collector = bounded(1, usize::MAX);
    collector.total_bytes = usize::MAX;

    let error = collector.push(b"x".to_vec()).unwrap_err();

    assert!(matches!(
        error,
        GrustError::ResourceLimitExceeded {
            resource: ARROW_IPC_BYTES,
            limit: usize::MAX,
            observed: usize::MAX,
        }
    ));
    assert!(collector.into_chunks().is_empty());
}

#[test]
fn unbounded_collection_preserves_legacy_output_without_copying_batches() {
    let mut collector = ArrowIpcCollector::new(None);
    let batch = b"batch".to_vec();
    let batch_ptr = batch.as_ptr();

    collector.push(Vec::new()).unwrap();
    collector.push(batch).unwrap();

    let chunks = collector.into_chunks();
    assert_eq!(chunks, vec![Vec::new(), b"batch".to_vec()]);
    assert_eq!(chunks[1].as_ptr(), batch_ptr);
}
