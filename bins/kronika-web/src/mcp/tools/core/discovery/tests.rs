use std::sync::Arc;

use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_writer::{Journal, JournalConfig, SectionBuffers};
use serde_json::Value;

use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::State;

const SEGMENT_ID: i64 = 1_710_000_000_000_000;

#[test]
fn context_discovers_every_descriptor_once() {
    let directory = tempfile::tempdir().expect("temporary empty context root");
    let state = state(directory.path(), SOURCE_OS | SOURCE_POSTGRESQL, false);

    let payload = super::payload(&state, &|| false).expect("MCP context");
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
    let directory = tempfile::tempdir().expect("temporary empty context root");
    let state = state(directory.path(), SOURCE_OS, true);
    let payload = super::payload(&state, &|| false).expect("MCP context");
    let process = surface(&payload.data, "kronika_find_processes");
    let heatmap = surface(&payload.data, "kronika_rank_heatmap");

    assert_eq!(
        process["lenses"],
        serde_json::json!(["generic", "cpu", "memory", "disk", "tree"])
    );
    assert!(
        heatmap["cuts"]["processes"]
            .as_array()
            .is_some_and(|cuts| cuts.iter().any(|cut| cut == "cpu"))
    );
    assert_eq!(
        heatmap["groups"]["processes"],
        serde_json::json!(["identity", "command"])
    );
    assert_eq!(heatmap["defaults"]["processes"]["cut"], "cpu");
    assert_eq!(heatmap["defaults"]["processes"]["group"], "command");
    assert_eq!(heatmap["defaults"]["processes"]["columns"], 60);
    assert_eq!(heatmap["defaults"]["processes"]["top"], 25);
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
fn context_reports_latest_recorded_layout_and_active_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_load(SEGMENT_ID + 10);

    let payload = super::payload(&fixture.state(), &|| false).expect("recorded MCP context");
    let recorded = &payload.data["context"]["recorded"];
    assert_eq!(recorded["as_of_us"], (SEGMENT_ID + 10).to_string());
    assert_eq!(recorded["source_families"][0]["name"], "os");
    assert_eq!(recorded["source_families"][0]["present"], true);
    assert_eq!(recorded["source_families"][0]["metrics_present"], true);
    assert_eq!(recorded["source_families"][1]["present"], false);

    let layout = recorded["layouts"]
        .as_array()
        .expect("recorded layouts")
        .iter()
        .find(|layout| layout["logical_name"] == "os_loadavg")
        .expect("Loadavg layout");
    assert_eq!(layout["physical_name"], "os_loadavg");
    assert_eq!(layout["source_family"], "os");
    assert_eq!(
        layout["segment_ids"],
        serde_json::json!([SEGMENT_ID.to_string()])
    );

    let segment = &recorded["segments"][0];
    assert_eq!(segment["segment_id"], SEGMENT_ID.to_string());
    assert_eq!(segment["kind"], "active");
    assert!(segment["active_wal_position"].as_str().is_some());
}

#[test]
fn heatmap_lookup_and_discovery_share_one_registry() {
    let selected = crate::heatmap_product::resolve("tables", Some("writes"), None, None)
        .expect("accepted Heatmap cut");
    let cut = selected.cut;

    assert_eq!(cut.section, "pg_stat_user_tables");
    assert_eq!(cut.fields, ["n_tup_ins", "n_tup_upd", "n_tup_del"]);
    assert!(crate::heatmap_product::resolve("tables", Some("cpu"), None, None).is_err());
    let semantic = super::heatmap_semantic(selected.surface, cut);
    assert_eq!(semantic["origin"], "accepted_presentation");
    assert_eq!(semantic["value_unit"], "count");
    assert_eq!(semantic["values_scaled"], false);

    let blocks =
        crate::heatmap_product::resolve("statements", Some("shared_read"), Some("identity"), None)
            .expect("block cut");
    let blocks_semantic = super::heatmap_semantic(blocks.surface, blocks.cut);
    assert_eq!(blocks_semantic["value_unit"], "blocks");
    assert_eq!(
        blocks_semantic["conversion"],
        serde_json::json!({
            "status": "not_applied",
            "operation": "multiply",
            "factor": null,
            "target_unit": "bytes",
            "origin": "recorded",
            "locator": "pg_settings.block_size",
        })
    );

    let ticks =
        crate::heatmap_product::resolve("processes", None, None, None).expect("default clock cut");
    let ticks_semantic = super::heatmap_semantic(ticks.surface, ticks.cut);
    assert_eq!(ticks_semantic["value_unit"], "clock_ticks");
    assert_eq!(
        ticks_semantic["conversion"]["locator"],
        "instance_metadata.clock_ticks_per_sec"
    );
    assert_eq!(ticks.group.id, "command");
    assert_eq!(ticks.columns, 60);
}

fn surface<'a>(context: &'a Value, name: &str) -> &'a Value {
    context["surfaces"]
        .as_array()
        .expect("tool surfaces")
        .iter()
        .find(|surface| surface["tool"] == name)
        .expect("named tool surface")
}

fn state(root: &std::path::Path, sources: u32, synthetic_demo: bool) -> State {
    State {
        data_root: root.to_owned(),
        sources,
        synthetic_demo,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary context data root");
        let root = DataRoot::open(directory.path()).expect("open context data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire context writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open context journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
        }
    }

    fn state(&self) -> State {
        state(self.directory.path(), SOURCE_OS, false)
    }

    fn append_load(&mut self, timestamp: i64) {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(OsLoadavg {
                ts: Ts(timestamp),
                load1: 1.5,
                load5: 1.0,
                load15: 0.5,
                running: 2,
                total: 345,
                scope: 0,
            })
            .expect("Loadavg row fits");
        let part = buffers
            .flush(&[])
            .expect("encode context fixture")
            .expect("nonempty context fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append context fixture");
    }
}
