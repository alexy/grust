use super::*;

#[test]
fn drop_sql_accepts_only_safe_session_view_names() {
    assert_eq!(
        drop_sql("cognition_input_42").expect("safe view"),
        "DROP VIEW IF EXISTS `cognition_input_42`"
    );
    assert!(drop_sql("unsafe; DROP TABLE grust_nodes").is_err());
    assert!(drop_sql("Catalog.View").is_err());
}
