use grust_cocoindex::CocoIndexExport;
use grust_core::prelude::*;
use serde_json::json;

#[test]
fn public_api_exports_cocoindex_target_state() {
    let mut builder = Graph::builder();
    builder
        .node("Group", "meetup:rust-sf")
        .prop("name", "Rust SF")
        .finish();
    builder
        .node("Event", "event:123")
        .prop("title", "Async Rust Night")
        .prop("capacity", 80i64)
        .finish();
    builder
        .edge("HOSTED", "meetup:rust-sf", "event:123")
        .prop("source", "calendar")
        .finish();

    let export = builder.build().to_cocoindex_export().expect("export");
    let json = serde_json::to_value(export).expect("serialize export");

    assert_eq!(json["nodes"].as_array().expect("nodes").len(), 2);
    assert_eq!(
        json["relationships"]
            .as_array()
            .expect("relationships")
            .len(),
        1
    );
    assert_eq!(
        json["relationships"][0],
        json!({
            "rel_type": "HOSTED",
            "source": {"label": "Group", "key": {"id": "meetup:rust-sf"}},
            "target": {"label": "Event", "key": {"id": "event:123"}},
            "key": {"id": "meetup:rust-sf\u{1f}HOSTED\u{1f}event:123"},
            "properties": {"source": "calendar"}
        })
    );
}
