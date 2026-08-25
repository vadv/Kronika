use std::cell::Cell;
use std::path::Path;

use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_reader::{Listing, Reader};
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::pg_settings::PgSettings;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Journal, JournalConfig, SectionBuffers, write_segment};
use serde_json::Value;

use super::{
    MAX_CONTEXT_JSON_BYTES, MAX_CONTEXT_SECTION_PRESENCES, MAX_CONTEXT_SEGMENTS,
    MAX_CONTEXT_WARNINGS, produce, render_catalog_records, validate_bounds, validate_json_bytes,
};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::route::Window;

const SEGMENT_ID: i64 = 1_710_000_000_000_000;

#[test]
fn context_uses_one_finished_and_active_catalog_snapshot() {
    let mut fixture = Fixture::new();
    fixture.append_load(SEGMENT_ID + 10);
    fixture.finish_and_continue(SEGMENT_ID + 100);
    fixture.append_setting(SEGMENT_ID + 110);

    let context = fixture.context();
    assert_eq!(
        context.value["catalog"].as_array().expect("catalog values"),
        &context.records
    );
    let catalog = context.value["catalog"]
        .as_array()
        .expect("catalog records");
    let header = record(catalog, "catalog");
    assert_source(header, "os", true, true, true);
    assert_source(header, "postgresql", true, true, true);

    let finished = record(catalog, "finished_segment");
    assert_eq!(finished["id"], SEGMENT_ID.to_string());
    assert_eq!(finished["sections"][0]["logical_name"], "os_loadavg");
    assert_eq!(finished["sections"][0]["source_family"], "os");

    let active = record(catalog, "active_segment");
    assert_eq!(active["id"], (SEGMENT_ID + 100).to_string());
    assert!(active["cursor"]["wal_position"].as_str().is_some());
    assert_eq!(active["sections"][0]["logical_name"], "pg_settings");
    assert_eq!(active["sections"][0]["source_family"], "postgresql");
}

#[test]
fn missing_source_and_layouts_stay_absent() {
    let mut fixture = Fixture::new();
    fixture.append_load(SEGMENT_ID + 10);

    let context = fixture.context();
    let catalog = context.value["catalog"]
        .as_array()
        .expect("catalog records");
    let header = record(catalog, "catalog");
    assert_source(header, "os", true, true, true);
    assert_source(header, "postgresql", true, false, false);
    assert!(catalog.iter().all(|record| {
        record["sections"].as_array().is_none_or(|sections| {
            sections
                .iter()
                .all(|section| section["source_family"] != "postgresql")
        })
    }));
    assert!(
        !serde_json::to_string(&context.value)
            .expect("serialize context")
            .contains("completeness")
    );
}

#[test]
fn active_cursor_and_layout_rows_pin_one_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_setting(SEGMENT_ID + 10);
    let reader = Reader::open(fixture.root()).expect("context reader");
    let listing = reader
        .catalog_segments_cancellable(.., &|| false)
        .expect("pinned context listing");
    let first_position = listing.segments[0]
        .active_position()
        .expect("pinned active position");

    fixture.append_setting(SEGMENT_ID + 20);
    let pinned = render_catalog_records(
        listing,
        Window::default(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        false,
        &|| false,
    )
    .expect("render pinned context listing");
    let first_active = record(&pinned, "active_segment");
    assert_eq!(
        first_active["cursor"]["wal_position"],
        first_position.to_string()
    );
    assert_eq!(first_active["sections"][0]["rows"], "1");

    let second = fixture.context();
    let second_active = record(
        second.value["catalog"].as_array().expect("second catalog"),
        "active_segment",
    );
    let second_position = second_active["cursor"]["wal_position"]
        .as_str()
        .expect("second WAL position")
        .parse::<u64>()
        .expect("decimal WAL position");
    assert!(second_position > first_position);
    assert_eq!(second_active["sections"][0]["rows"], "2");
}

#[test]
fn producer_cancellation_is_atomic() {
    let directory = tempfile::tempdir().expect("temporary context root");
    let checks = Cell::new(0_usize);
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() >= 5
    };
    let error = match produce(
        directory.path(),
        Window::default(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        false,
        &cancelled,
    ) {
        Ok(_) => panic!("Context production was not cancelled"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "cancelled");
    assert!(checks.get() >= 5);
}

#[test]
fn render_cancellation_discards_locally_built_catalog() {
    let checks = Cell::new(0_usize);
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() >= 2
    };
    let error = render_catalog_records(
        Listing {
            segments: Vec::new(),
            warnings: Vec::new(),
        },
        Window::default(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        false,
        &cancelled,
    )
    .expect_err("cancelled catalog render");

    assert_eq!(error.code(), "cancelled");
    assert_eq!(checks.get(), 2);
}

#[test]
fn context_bounds_accept_the_maximum_and_reject_the_next_value() {
    validate_bounds(
        MAX_CONTEXT_SEGMENTS,
        MAX_CONTEXT_SECTION_PRESENCES,
        MAX_CONTEXT_WARNINGS,
    )
    .expect("exact context bounds");
    validate_json_bytes(MAX_CONTEXT_JSON_BYTES).expect("exact context byte bound");
    assert_eq!(
        validate_json_bytes(MAX_CONTEXT_JSON_BYTES + 1)
            .expect_err("over-bound context bytes")
            .code(),
        "context_byte_limit_exceeded"
    );
    for (segments, sections, warnings, code) in [
        (
            MAX_CONTEXT_SEGMENTS + 1,
            MAX_CONTEXT_SECTION_PRESENCES,
            MAX_CONTEXT_WARNINGS,
            "segment_limit_exceeded",
        ),
        (
            MAX_CONTEXT_SEGMENTS,
            MAX_CONTEXT_SECTION_PRESENCES + 1,
            MAX_CONTEXT_WARNINGS,
            "layout_limit_exceeded",
        ),
        (
            MAX_CONTEXT_SEGMENTS,
            MAX_CONTEXT_SECTION_PRESENCES,
            MAX_CONTEXT_WARNINGS + 1,
            "warning_limit_exceeded",
        ),
    ] {
        assert_eq!(
            validate_bounds(segments, sections, warnings)
                .expect_err("over-bound context")
                .code(),
            code
        );
    }
}

#[test]
fn context_uses_public_surface_and_semantic_registries_only() {
    let directory = tempfile::tempdir().expect("temporary context root");
    let context = produce(
        directory.path(),
        Window::default(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        false,
        &|| false,
    )
    .expect("empty product context");

    let process = &context.value["surfaces"]["process"];
    assert_eq!(process["default_lens"], "tree");
    assert_eq!(
        ids(process["lenses"].as_array().expect("Process lenses")),
        ["generic", "cpu", "memory", "disk", "tree"]
    );
    let postgresql = context.value["surfaces"]["postgresql"]["surfaces"]
        .as_array()
        .expect("PostgreSQL surfaces");
    assert_eq!(
        ids(postgresql),
        [
            "pg_stat_activity",
            "pg_locks",
            "pg_stat_statements",
            "pg_store_plans",
            "pg_stat_database",
            "pg_stat_user_tables",
            "pg_stat_user_indexes",
        ]
    );
    let statements = postgresql
        .iter()
        .find(|surface| surface["id"] == "pg_stat_statements")
        .expect("Statement surface");
    assert_eq!(statements["default_lens"], "load");
    assert_eq!(
        ids(statements["lenses"].as_array().expect("Statement lenses")),
        ["load", "per_call", "io", "resources", "stability"]
    );
    let heatmap = context.value["surfaces"]["heatmap"]["surfaces"]
        .as_array()
        .expect("Heatmap surfaces");
    let processes = heatmap
        .iter()
        .find(|surface| surface["id"] == "processes")
        .expect("Process Heatmap");
    assert_eq!(processes["default_cut"], "cpu");
    assert_eq!(processes["default_group"], "command");
    assert_eq!(processes["default_columns"], 60);
    assert!(processes.get("section").is_none());
    assert!(processes.get("fields").is_none());

    let products = context.value["semantics"]["products"]
        .as_array()
        .expect("product semantics");
    let query_duration = semantic(products, "value_tone.query_duration_ms");
    assert_eq!(query_duration["unit"], "milliseconds");
    assert_eq!(
        query_duration["formula"],
        "(sample_timestamp - query_start) / 1000"
    );
    assert_eq!(
        query_duration["thresholds"][0]["value"].as_f64(),
        Some(5_000.0)
    );
    assert_eq!(query_duration["thresholds"][0]["tone"], "critical");
    let vacuum = semantic(products, "vacuum.phase_risk");
    assert_eq!(vacuum["policy"]["phases"]["truncating heap"], "dangerous");

    let findings = context.value["semantics"]["findings"]
        .as_array()
        .expect("finding semantics");
    let cpu = semantic(findings, "finding.os_cpu.cpu_busy");
    assert_eq!(cpu["unit"], "percent");
    assert_eq!(cpu["formula"], "100 * busy_ticks / total_ticks");
    assert_eq!(cpu["boundary"]["operator"], "gte");
    assert_eq!(cpu["boundary"]["numerator"], "80");
    let health = context.value["semantics"]["health"]
        .as_array()
        .expect("health semantics");
    assert_eq!(semantic(health, "health.os")["unit"], "percent");

    assert_public_context(&context.value);
}

fn assert_public_context(value: &Value) {
    const FORBIDDEN_KEYS: &[&str] = &[
        "inputSchema",
        "required",
        "additionalProperties",
        "limits",
        "historical_only",
        "transport",
        "authentication",
        "completeness",
        "tool",
    ];
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                assert!(
                    !FORBIDDEN_KEYS.contains(&key.as_str()),
                    "forbidden key {key}"
                );
                assert_public_context(nested);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_public_context),
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            for forbidden in [
                "proof",
                "evidence",
                "confidence",
                "causal",
                "kronika_get_",
                "exact 20",
                "investigation order",
            ] {
                assert!(!lower.contains(forbidden), "forbidden language in {text:?}");
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn record<'a>(catalog: &'a [Value], kind: &str) -> &'a Value {
    catalog
        .iter()
        .find(|record| record["record"] == kind)
        .unwrap_or_else(|| panic!("missing {kind} record"))
}

fn assert_source(
    header: &Value,
    name: &str,
    configured: bool,
    present: bool,
    metrics_present: bool,
) {
    let source = header["source_families"]
        .as_array()
        .expect("source families")
        .iter()
        .find(|source| source["name"] == name)
        .unwrap_or_else(|| panic!("missing {name} source"));
    assert_eq!(source["configured"], configured);
    assert_eq!(source["present"], present);
    assert_eq!(source["metrics_present"], metrics_present);
}

fn ids(values: &[Value]) -> Vec<&str> {
    values
        .iter()
        .map(|value| value["id"].as_str().expect("textual id"))
        .collect()
}

fn semantic<'a>(definitions: &'a [Value], id: &str) -> &'a Value {
    definitions
        .iter()
        .find(|definition| definition["id"] == id)
        .unwrap_or_else(|| panic!("missing {id} semantic"))
}

struct Fixture {
    directory: tempfile::TempDir,
    writer: WriterOwner,
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
            writer,
            journal,
            address,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn context(&self) -> super::ProductContext {
        produce(
            self.root(),
            Window::default(),
            SOURCE_OS | SOURCE_POSTGRESQL,
            false,
            &|| false,
        )
        .expect("product context")
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
        self.append(buffers);
    }

    fn append_setting(&mut self, timestamp: i64) {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgSettings {
                ts: Ts(timestamp),
                datid: 16_384,
                datname: StrId(1),
                usesysid: 16_385,
                usename: StrId(2),
                name: StrId(3),
                setting: StrId(4),
                unit: Some(StrId(5)),
                source: StrId(6),
                sourcefile: None,
                sourceline: None,
                pending_restart: false,
                context: StrId(7),
                vartype: StrId(8),
                boot_val: None,
                reset_val: None,
            })
            .expect("PostgreSQL setting row fits");
        self.append(buffers);
    }

    fn append(&mut self, mut buffers: SectionBuffers) {
        let part = buffers
            .flush(&[])
            .expect("encode context fixture")
            .expect("nonempty context fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append context fixture");
    }

    fn finish_and_continue(&mut self, segment_id: i64) {
        write_segment(&self.journal, &self.writer, self.address).expect("finish context segment");
        self.journal.reset().expect("reset context journal");
        self.address = SegmentAddress::new(SegmentId::new(segment_id).expect("segment id"))
            .expect("segment address");
    }
}
