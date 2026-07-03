pub use grust_core::*;

pub mod lakecat;
pub use lakecat::{
    LakeCatCatalogEvent, LakeCatCatalogGraph, LakeCatTableRef, lakecat_catalog_event_graph,
    lakecat_catalog_graph_from_json_value, lakecat_event_id, lakecat_namespace_id,
    lakecat_warehouse_id,
};

#[cfg(feature = "cocoindex")]
pub use grust_cocoindex::*;

#[cfg(feature = "cypher")]
pub use grust_cypher::{
    CypherCatalogSnapshot, CypherConstraintRegistry, CypherCreateMode, CypherDdlApplicationReport,
    CypherDdlStatement, CypherGeneratedNodeId, CypherMutationOptions, CypherMutationReport,
    CypherMutationResult, CypherMutationTableResult, CypherNodeIdPolicy, CypherNullAssignment,
    CypherParameters, CypherRelationshipIdPolicy, CypherResultTable, CypherSchemaApplication,
    CypherSchemaManager, CypherSession, CypherWrittenEdgeIdentity, CypherWrittenNodeIdentity,
    GraphIndexDefinition, GraphIndexElement, GraphTypeDefinition, NamedGraphCatalog,
    NamedGraphConstraint, NamedGraphIndex, NamedGraphType, SessionCommand,
    apply_cypher_ddl_to_schema, apply_cypher_native_constraints, cypher_catalog_procedure,
    cypher_constraints, cypher_ddl, cypher_mutation_plan, cypher_mutation_plan_with_options,
    cypher_mutation_plan_with_return_options, ensure_catalog_graph_selection,
    ensure_query_uses_graph, execute_cypher_mutation_returning_with_options_on_store,
    query_graph_selection, run_read_query_on_named_graph, sail_cypher_constraints, sail_cypher_ddl,
    sail_cypher_mutation_plan, sail_cypher_mutation_plan_with_options,
    sail_cypher_mutation_plan_with_return_options,
};

#[cfg(feature = "falkor")]
pub use grust_falkor::*;

#[cfg(feature = "lancedb")]
pub use grust_lancedb::*;

#[cfg(feature = "memory")]
pub use grust_memory::*;

#[cfg(feature = "postgres")]
pub use grust_postgres::*;

#[cfg(feature = "postgres-pgq")]
pub use grust_postgres_pgq::*;

#[cfg(feature = "pggraph")]
pub use grust_pggraph::*;

// Sail-native surface only. The Cypher language types and cypher_*/sail_cypher_*
// entrypoints are re-exported once by the `cypher` block above (the `sail`
// feature enables `cypher`).
#[cfg(feature = "sail")]
pub use grust_sail::{
    SailConfig, SailDegreePairRow, SailDegreeRow, SailGraphPatternDirection, SailGraphStore,
    SailGraphTypedTable, SailGraphTypedTableKind, SailTripletRow, sail_degree_pairs_sql,
    sail_degrees_sql, sail_graph_schema_typed_tables, sail_in_degrees_sql, sail_out_degrees_sql,
    sail_triplets_sql, sail_triplets_sql_for_direction, sail_typed_edge_columns,
    sail_typed_edge_table_missing_fields, sail_typed_node_columns,
    sail_typed_node_table_missing_fields,
};

#[cfg(feature = "surreal")]
pub use grust_surreal::*;

#[cfg(feature = "turso")]
pub use grust_turso::*;

pub mod prelude {
    pub use crate::lakecat::{
        LakeCatCatalogEvent, LakeCatCatalogGraph, LakeCatTableRef, lakecat_catalog_event_graph,
        lakecat_catalog_graph_from_json_value, lakecat_event_id, lakecat_namespace_id,
        lakecat_warehouse_id,
    };
    pub use grust_core::prelude::*;

    #[cfg(feature = "cocoindex")]
    pub use grust_cocoindex::*;

    #[cfg(feature = "cypher")]
    pub use grust_cypher::{
        CypherCatalogSnapshot, CypherConstraintRegistry, CypherCreateMode,
        CypherDdlApplicationReport, CypherDdlStatement, CypherGeneratedNodeId,
        CypherMutationOptions, CypherMutationReport, CypherMutationResult,
        CypherMutationTableResult, CypherNodeIdPolicy, CypherNullAssignment, CypherParameters,
        CypherRelationshipIdPolicy, CypherResultTable, CypherSchemaApplication,
        CypherSchemaManager, CypherSession, CypherWrittenEdgeIdentity, CypherWrittenNodeIdentity,
        GraphIndexDefinition, GraphIndexElement, GraphTypeDefinition, NamedGraphCatalog,
        NamedGraphConstraint, NamedGraphIndex, NamedGraphType, SessionCommand,
        apply_cypher_ddl_to_schema, apply_cypher_native_constraints, cypher_catalog_procedure,
        cypher_constraints, cypher_ddl, cypher_mutation_plan, cypher_mutation_plan_with_options,
        cypher_mutation_plan_with_return_options, ensure_catalog_graph_selection,
        ensure_query_uses_graph, execute_cypher_mutation_returning_with_options_on_store,
        query_graph_selection, run_read_query_on_named_graph, sail_cypher_constraints,
        sail_cypher_ddl, sail_cypher_mutation_plan, sail_cypher_mutation_plan_with_options,
        sail_cypher_mutation_plan_with_return_options,
    };

    #[cfg(feature = "falkor")]
    pub use grust_falkor::*;

    #[cfg(feature = "lancedb")]
    pub use grust_lancedb::*;

    #[cfg(feature = "memory")]
    pub use grust_memory::*;

    #[cfg(feature = "postgres")]
    pub use grust_postgres::*;

    #[cfg(feature = "postgres-pgq")]
    pub use grust_postgres_pgq::*;

    #[cfg(feature = "pggraph")]
    pub use grust_pggraph::*;

    // Sail-native surface only; the Cypher language surface comes from the
    // `cypher` block above (the `sail` feature enables `cypher`).
    #[cfg(feature = "sail")]
    pub use grust_sail::{
        SailConfig, SailDegreePairRow, SailDegreeRow, SailGraphPatternDirection, SailGraphStore,
        SailGraphTypedTable, SailGraphTypedTableKind, SailTripletRow, sail_degree_pairs_sql,
        sail_degrees_sql, sail_graph_schema_typed_tables, sail_in_degrees_sql,
        sail_out_degrees_sql, sail_triplets_sql, sail_triplets_sql_for_direction,
        sail_typed_edge_columns, sail_typed_edge_table_missing_fields, sail_typed_node_columns,
        sail_typed_node_table_missing_fields,
    };

    #[cfg(feature = "surreal")]
    pub use grust_surreal::*;

    #[cfg(feature = "turso")]
    pub use grust_turso::*;
}
