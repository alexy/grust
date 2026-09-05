#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendCatalogEntry {
    pub id: &'static str,
    pub adapter: &'static str,
    pub feature: Option<&'static str>,
    pub query_capability: QueryCapability,
    pub default_execution: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryCapability {
    PortableQuery,
    NativeAggregate,
    MaterializeThenReference,
    ExportOnly,
}

pub const BACKENDS: [BackendCatalogEntry; 14] = [
    BackendCatalogEntry {
        id: "memory",
        adapter: "grust-memory",
        feature: None,
        query_capability: QueryCapability::PortableQuery,
        default_execution: Some("in-process-reference"),
    },
    BackendCatalogEntry {
        id: "turso",
        adapter: "grust-turso",
        feature: None,
        query_capability: QueryCapability::PortableQuery,
        default_execution: Some("backend-row-source+rust-projection"),
    },
    BackendCatalogEntry {
        id: "postgres",
        adapter: "grust-postgres-core",
        feature: None,
        query_capability: QueryCapability::PortableQuery,
        default_execution: Some("backend-row-source+rust-projection"),
    },
    BackendCatalogEntry {
        id: "ladybug",
        adapter: "grust-ladybug",
        feature: Some("ladybug"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "falkor",
        adapter: "grust-falkor",
        feature: Some("falkor"),
        query_capability: QueryCapability::NativeAggregate,
        default_execution: Some("backend-native-aggregate"),
    },
    BackendCatalogEntry {
        id: "surreal",
        adapter: "grust-surreal",
        feature: Some("surreal"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "lancedb",
        adapter: "grust-lancedb",
        feature: Some("lancedb"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "sail",
        adapter: "grust-sail",
        feature: Some("sail"),
        query_capability: QueryCapability::PortableQuery,
        default_execution: Some("backend-row-source+rust-projection"),
    },
    BackendCatalogEntry {
        id: "pggraph",
        adapter: "grust-pggraph",
        feature: Some("pggraph"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "postgres-pgq",
        adapter: "grust-postgres-pgq",
        feature: Some("postgres-pgq"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "helix",
        adapter: "grust-helix",
        feature: Some("helix"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "cocoindex",
        adapter: "grust-cocoindex",
        feature: None,
        query_capability: QueryCapability::ExportOnly,
        default_execution: None,
    },
    BackendCatalogEntry {
        id: "helix-sdk",
        adapter: "grust-helix",
        feature: Some("helix"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
    BackendCatalogEntry {
        id: "surreal-sdk",
        adapter: "grust-surreal",
        feature: Some("surreal"),
        query_capability: QueryCapability::MaterializeThenReference,
        default_execution: Some("backend-materialize+rust-reference"),
    },
];

pub fn backend(id: &str) -> Result<&'static BackendCatalogEntry, String> {
    BACKENDS.iter().find(|entry| entry.id == id).ok_or_else(|| {
        format!(
            "unknown backend {id:?}; use {}",
            BACKENDS
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

pub fn compiled(feature: Option<&str>) -> bool {
    match feature {
        None => true,
        Some("falkor") => cfg!(feature = "falkor"),
        Some("helix") => cfg!(feature = "helix"),
        Some("ladybug") => cfg!(feature = "ladybug"),
        Some("lancedb") => cfg!(feature = "lancedb"),
        Some("pggraph") => cfg!(feature = "pggraph"),
        Some("postgres-pgq") => cfg!(feature = "postgres-pgq"),
        Some("sail") => cfg!(feature = "sail"),
        Some("surreal") => cfg!(feature = "surreal"),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_every_backend_once() {
        let mut ids = BACKENDS.iter().map(|entry| entry.id).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BACKENDS.len());
        assert!(backend("ladybug").is_ok());
        assert!(backend("helix").is_ok());
        assert!(backend("cocoindex").is_ok());
    }

    #[test]
    fn export_adapter_is_not_a_query_backend() {
        assert_eq!(
            backend("cocoindex").unwrap().query_capability,
            QueryCapability::ExportOnly
        );
    }

    #[test]
    fn sdk_lanes_preserve_http_identities_and_capabilities() {
        for (http, sdk) in [("helix", "helix-sdk"), ("surreal", "surreal-sdk")] {
            let http = backend(http).unwrap();
            let sdk = backend(sdk).unwrap();
            assert_ne!(http.id, sdk.id);
            assert_eq!(http.adapter, sdk.adapter);
            assert_eq!(http.feature, sdk.feature);
            assert_eq!(
                sdk.query_capability,
                QueryCapability::MaterializeThenReference
            );
            assert_eq!(http.default_execution, sdk.default_execution);
        }
    }
}
