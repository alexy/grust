use grust_core::{GraphAdminStore, GraphStore};
use grust_sail::{SailConfig, SailGraphStore};

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn typed_table_rejects_null_structural_identity() {
    let store = SailGraphStore::connect(SailConfig::default())
        .await
        .expect("connect to Sail");
    let schema = grust_core::GraphSchema::builder()
        .node("ConstraintProbe", vec![])
        .build();
    store
        .apply_schema(&schema)
        .await
        .expect("create constrained typed table");

    let error = store
        .query_arrow_ipc("INSERT INTO grust_node_constraintprobe VALUES (NULL)")
        .await
        .expect_err("Delta check constraint rejects null node id");
    assert!(
        error.to_string().contains("grust_node_id_not_null"),
        "unexpected constraint error: {error}"
    );

    store.clear().await.expect("clear probe tables");
}
