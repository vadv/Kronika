use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::State;

#[test]
fn context_discovers_every_descriptor_once() {
    let state = State {
        data_root: PathBuf::from("unused-context-root"),
        sources: SOURCE_OS | SOURCE_POSTGRESQL,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };

    let payload = super::payload(&state).expect("MCP context");
    let surfaces = payload.data["surfaces"].as_array().expect("tool surfaces");
    let names = surfaces
        .iter()
        .map(|surface| surface["tool"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    let catalog = crate::mcp::catalog::all()
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert_eq!(names, catalog);
    assert_eq!(names.len(), 20);
    assert_eq!(payload.page["returned"], 20);
    assert_eq!(payload.page["stop_reason"], "complete");
}

#[test]
fn context_exposes_schema_lenses_cuts_and_hard_limits() {
    let state = State {
        data_root: PathBuf::from("unused-context-root"),
        sources: SOURCE_OS,
        synthetic_demo: true,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let payload = super::payload(&state).expect("MCP context");
    let process = surface(&payload.data, "kronika_find_processes");
    let heatmap = surface(&payload.data, "kronika_rank_heatmap");

    assert_eq!(
        process["lenses"],
        serde_json::json!(["identity", "cpu", "memory", "disk", "tree"])
    );
    assert!(
        heatmap["cuts"]["processes"]
            .as_array()
            .is_some_and(|cuts| cuts.iter().any(|cut| cut == "cpu"))
    );
    assert_eq!(
        payload.data["context"]["limits"]["physical_row_visits"],
        super::super::MAX_ROWS
    );
    assert_eq!(
        payload.data["context"]["limits"]["decoded_cells"],
        super::super::super::DECODED_CELLS
    );
    assert_eq!(
        payload.data["context"]["configured_sources"][0]["configured"],
        true
    );
    assert_eq!(
        payload.data["context"]["configured_sources"][1]["configured"],
        false
    );
}

#[test]
fn heatmap_lookup_and_discovery_share_one_registry() {
    let cut = super::heatmap_cut("tables", "writes").expect("accepted Heatmap cut");

    assert_eq!(cut.section, "pg_stat_user_tables");
    assert_eq!(cut.fields, ["n_tup_ins", "n_tup_upd", "n_tup_del"]);
    assert!(super::heatmap_cut("tables", "cpu").is_none());
    assert_eq!(cut.semantic()["origin"], "accepted_presentation");
}

fn surface<'a>(context: &'a Value, name: &str) -> &'a Value {
    context["surfaces"]
        .as_array()
        .expect("tool surfaces")
        .iter()
        .find(|surface| surface["tool"] == name)
        .expect("named tool surface")
}
