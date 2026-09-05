//! Decode portable pushdown rows, including integer row-presence markers.
use std::io::Cursor;

use arrow::array::{Array, Int32Array, Int64Array, StringArray};
use arrow::ipc::reader::StreamReader;
use grust_core::{GrustError, Result};

pub(super) fn parse_text_rows_from_arrow(data: &[u8]) -> Result<Vec<Vec<Option<String>>>> {
    let reader = StreamReader::try_new(Cursor::new(data), None)
        .map_err(|e| GrustError::Backend(format!("Arrow IPC read failed: {e}")))?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| GrustError::Backend(format!("Arrow batch error: {e}")))?;
        for (i, column) in batch.columns().iter().enumerate() {
            if !column.as_any().is::<StringArray>()
                && !column.as_any().is::<Int32Array>()
                && !column.as_any().is::<Int64Array>()
            {
                return Err(GrustError::Schema(format!(
                    "pushdown result column {i} is not text or an integer"
                )));
            }
        }
        for row in 0..batch.num_rows() {
            out.push(
                batch
                    .columns()
                    .iter()
                    .map(|column| {
                        if column.is_null(row) {
                            return None;
                        }
                        let array = column.as_any();
                        // Plans without selected bindings use SELECT 1 to preserve
                        // match multiplicity for the Rust projection (e.g. count(*)).
                        // Sail returns that marker as Int32, not Arrow Utf8.
                        Some(if let Some(values) = array.downcast_ref::<StringArray>() {
                            values.value(row).to_owned()
                        } else if let Some(values) = array.downcast_ref::<Int32Array>() {
                            values.value(row).to_string()
                        } else {
                            array
                                .downcast_ref::<Int64Array>()
                                .expect("validated integer column")
                                .value(row)
                                .to_string()
                        })
                    })
                    .collect(),
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Float64Array, RecordBatch};
    use arrow::ipc::writer::StreamWriter;
    use std::sync::Arc;

    fn stream(columns: Vec<(&str, ArrayRef)>, batches: usize) -> Vec<u8> {
        let batch = RecordBatch::try_from_iter(columns).unwrap();
        let mut bytes = Vec::new();
        let mut writer = StreamWriter::try_new(&mut bytes, &batch.schema()).unwrap();
        for _ in 0..batches {
            writer.write(&batch).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        bytes
    }

    #[test]
    fn integer_markers_preserve_match_multiplicity_across_batches() {
        let bytes = stream(vec![("1", Arc::new(Int32Array::from(vec![1, 1, 1])))], 2);
        assert_eq!(
            parse_text_rows_from_arrow(&bytes).unwrap(),
            vec![vec![Some("1".into())]; 6]
        );
        let params = grust_cypher::CypherParameters::new();
        let plan = grust_cypher::pushdown::plan_read(
            "MATCH (:A)-[:R]->(:B) RETURN count(*) AS count",
            &params,
            &grust_cypher::pushdown::NoTypeHints,
        )
        .unwrap()
        .unwrap();
        let dialect = grust_cypher::pushdown::SparkDialect;
        assert!(plan.to_sql(&dialect).starts_with("SELECT 1 FROM "));
        let result = plan
            .project_text_rows(
                &dialect,
                parse_text_rows_from_arrow(&bytes).unwrap(),
                &params,
            )
            .unwrap();
        assert_eq!(result.rows, vec![vec![grust_core::Value::Int(6)]]);
    }

    #[test]
    fn text_and_integer_columns_preserve_order_and_nulls() {
        let bytes = stream(
            vec![
                (
                    "text",
                    Arc::new(StringArray::from(vec![Some("node"), None])),
                ),
                ("marker", Arc::new(Int32Array::from(vec![None, Some(1)]))),
                (
                    "wide",
                    Arc::new(Int64Array::from(vec![Some(i64::MAX), None])),
                ),
            ],
            1,
        );
        assert_eq!(
            parse_text_rows_from_arrow(&bytes).unwrap(),
            vec![
                vec![Some("node".into()), None, Some(i64::MAX.to_string())],
                vec![None, Some("1".into()), None],
            ]
        );
    }

    #[test]
    fn unsupported_types_are_not_silently_coerced() {
        let bytes = stream(vec![("float", Arc::new(Float64Array::from(vec![1.5])))], 1);
        assert!(parse_text_rows_from_arrow(&bytes).is_err());
    }

    #[test]
    fn empty_marker_batch_has_no_matches() {
        let bytes = stream(
            vec![("1", Arc::new(Int32Array::from(Vec::<i32>::new())))],
            1,
        );
        assert!(parse_text_rows_from_arrow(&bytes).unwrap().is_empty());
    }

    #[test]
    fn sail_recursive_paths_are_gated_before_sql_execution() {
        for bounds in ["0..0", "1..3"] {
            let query = format!("MATCH (a:A)-[:R*{bounds}]->(b:B) RETURN count(*) AS count");
            let plan = grust_cypher::pushdown::plan_read(
                &query,
                &Default::default(),
                &grust_cypher::pushdown::NoTypeHints,
            )
            .unwrap()
            .unwrap();
            assert!(!plan.supported_by(&grust_cypher::pushdown::SparkDialect));
            assert!(plan.supported_by(&grust_cypher::pushdown::SqliteDialect));
        }
    }
}
