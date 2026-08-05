use std::io::Cursor;

use arrow::array::StringArray;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use grust_sail::{SailConfig, SailGraphStore, SailWarehouse};

fn one_row_ipc() -> Vec<u8> {
    let schema = Schema::new(vec![Field::new("value", DataType::Utf8, false)]);
    let batch = RecordBatch::try_new(
        schema.clone().into(),
        vec![std::sync::Arc::new(StringArray::from_iter_values([
            "secret",
        ]))],
    )
    .expect("record batch");
    let mut bytes = Vec::new();
    let mut writer = StreamWriter::try_new(Cursor::new(&mut bytes), &schema).expect("IPC writer");
    writer.write(&batch).expect("write batch");
    writer.finish().expect("finish IPC");
    drop(writer);
    bytes
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn staged_arrow_view_can_be_dropped_idempotently() {
    let config = SailConfig {
        warehouse: SailWarehouse::LocalSessionScoped,
        ..SailConfig::default()
    };
    let store = SailGraphStore::connect(config)
        .await
        .expect("connect to Sail");
    let name = format!("cleanup_{}", uuid::Uuid::new_v4().simple());

    store
        .stage_arrow_ipc_view(&name, &one_row_ipc())
        .await
        .expect("stage view");
    assert_eq!(
        store
            .query_arrow_ipc(&format!("SELECT value FROM `{name}`"))
            .await
            .expect("query staged view")
            .len(),
        1
    );

    store
        .drop_arrow_ipc_view(&name)
        .await
        .expect("drop staged view");
    assert!(
        store
            .query_arrow_ipc(&format!("SELECT value FROM `{name}`"))
            .await
            .is_err()
    );
    store
        .drop_arrow_ipc_view(&name)
        .await
        .expect("missing view is accepted");
}
