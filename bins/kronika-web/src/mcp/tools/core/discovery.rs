use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::{Failure, HOUR_US, MAX_ROWS, MAX_SEGMENTS, Payload};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::{
    REQUEST_BODY_BYTES, RESPONSE_BODY_BYTES, STRUCTURED_CONTENT_BYTES, State, TEXT_SUMMARY_BYTES,
};

pub(super) struct HeatmapCut {
    pub(super) surface: &'static str,
    pub(super) cut: &'static str,
    pub(super) section: &'static str,
    pub(super) fields: &'static [&'static str],
    pub(super) labels: &'static [&'static str],
    unit: &'static str,
    scale_by: Option<&'static str>,
}

impl HeatmapCut {
    const fn new(
        surface: &'static str,
        cut: &'static str,
        section: &'static str,
        fields: &'static [&'static str],
        labels: &'static [&'static str],
        unit: &'static str,
        scale_by: Option<&'static str>,
    ) -> Self {
        Self {
            surface,
            cut,
            section,
            fields,
            labels,
            unit,
            scale_by,
        }
    }

    pub(super) fn semantic(&self) -> Value {
        json!({
            "id": format!("heatmap.{}.{}", self.surface, self.cut),
            "origin": "accepted_presentation",
            "fields": self.fields,
            "unit": self.unit,
            "scale_by": self.scale_by,
        })
    }
}

const CUTS: &[HeatmapCut] = &[
    HeatmapCut::new(
        "processes",
        "cpu",
        "os_process",
        &["utime", "stime"],
        &[],
        "seconds",
        Some("clock_ticks"),
    ),
    HeatmapCut::new(
        "processes",
        "rss",
        "os_process",
        &["rmem_kb"],
        &[],
        "bytes",
        Some("kib"),
    ),
    HeatmapCut::new(
        "processes",
        "io_read",
        "os_process",
        &["read_bytes"],
        &[],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "processes",
        "io_write",
        "os_process",
        &["write_bytes"],
        &[],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "processes",
        "majflt",
        "os_process",
        &["majflt"],
        &[],
        "count",
        None,
    ),
    HeatmapCut::new(
        "processes",
        "run_delay",
        "os_process",
        &["rundelay_ns"],
        &[],
        "nanoseconds",
        None,
    ),
    HeatmapCut::new(
        "statements",
        "exec_time",
        "pg_stat_statements",
        &["total_exec_time"],
        &["datname", "usename"],
        "milliseconds",
        None,
    ),
    HeatmapCut::new(
        "statements",
        "calls",
        "pg_stat_statements",
        &["calls"],
        &["datname", "usename"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "statements",
        "rows",
        "pg_stat_statements",
        &["rows"],
        &["datname", "usename"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "statements",
        "shared_read",
        "pg_stat_statements",
        &["shared_blks_read"],
        &["datname", "usename"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "statements",
        "shared_dirtied",
        "pg_stat_statements",
        &["shared_blks_dirtied"],
        &["datname", "usename"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "statements",
        "temp_written",
        "pg_stat_statements",
        &["temp_blks_written"],
        &["datname", "usename"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "statements",
        "wal_bytes",
        "pg_stat_statements",
        &["wal_bytes"],
        &["datname", "usename"],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "plans",
        "exec_time",
        "pg_store_plans",
        &["total_time"],
        &["datname", "usename"],
        "milliseconds",
        None,
    ),
    HeatmapCut::new(
        "plans",
        "calls",
        "pg_store_plans",
        &["calls"],
        &["datname", "usename"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "plans",
        "rows",
        "pg_store_plans",
        &["rows"],
        &["datname", "usename"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "plans",
        "shared_read",
        "pg_store_plans",
        &["shared_blks_read"],
        &["datname", "usename"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "plans",
        "temp_written",
        "pg_store_plans",
        &["temp_blks_written"],
        &["datname", "usename"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "databases",
        "commits",
        "pg_stat_database",
        &["xact_commit"],
        &["datname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "databases",
        "rollbacks",
        "pg_stat_database",
        &["xact_rollback"],
        &["datname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "databases",
        "db_read",
        "pg_stat_database",
        &["blks_read"],
        &["datname"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "databases",
        "temp_bytes",
        "pg_stat_database",
        &["temp_bytes"],
        &["datname"],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "databases",
        "deadlocks",
        "pg_stat_database",
        &["deadlocks"],
        &["datname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "tables",
        "writes",
        "pg_stat_user_tables",
        &["n_tup_ins", "n_tup_upd", "n_tup_del"],
        &["datname", "schemaname", "relname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "tables",
        "seq_read",
        "pg_stat_user_tables",
        &["seq_tup_read"],
        &["datname", "schemaname", "relname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "tables",
        "heap_read",
        "pg_stat_user_tables",
        &["heap_blks_read"],
        &["datname", "schemaname", "relname"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "tables",
        "dead_tuples",
        "pg_stat_user_tables",
        &["n_dead_tup"],
        &["datname", "schemaname", "relname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "tables",
        "autovacuum_time",
        "pg_stat_user_tables",
        &["total_autovacuum_time"],
        &["datname", "schemaname", "relname"],
        "milliseconds",
        None,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_scan",
        "pg_stat_user_indexes",
        &["idx_scan"],
        &["datname", "schemaname", "relname", "indexrelname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_tup_read",
        "pg_stat_user_indexes",
        &["idx_tup_read"],
        &["datname", "schemaname", "relname", "indexrelname"],
        "count",
        None,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_blks_read",
        "pg_stat_user_indexes",
        &["idx_blks_read"],
        &["datname", "schemaname", "relname", "indexrelname"],
        "bytes",
        Some("block_size"),
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_cpu",
        "os_cgroup_cpu",
        &["usage_usec"],
        &[],
        "microseconds",
        None,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_throttled",
        "os_cgroup_cpu",
        &["throttled_usec"],
        &[],
        "microseconds",
        None,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_read",
        "os_cgroup_io",
        &["rbytes"],
        &[],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_write",
        "os_cgroup_io",
        &["wbytes"],
        &[],
        "bytes",
        None,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_rios",
        "os_cgroup_io",
        &["rios"],
        &[],
        "count",
        None,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_wios",
        "os_cgroup_io",
        &["wios"],
        &[],
        "count",
        None,
    ),
];

pub(super) fn heatmap_cut(surface: &str, cut: &str) -> Option<&'static HeatmapCut> {
    CUTS.iter()
        .find(|candidate| candidate.surface == surface && candidate.cut == cut)
}

pub(super) fn payload(state: &State) -> Result<Payload, Failure> {
    let semantics = crate::product_semantics::all()
        .map_err(|error| Failure::bounded("semantics_unreadable", error.to_string()))?;
    Ok(Payload {
        anchor: super::anchor(None, None, None),
        data: json!({
            "context": {
                "historical_only": true,
                "transport": "stateless_streamable_http",
                "authentication": "web_boundary",
                "synthetic_demo": state.synthetic_demo,
                "configured_sources": source_values(state.sources),
                "limits": global_limits(),
            },
            "surfaces": surfaces(),
            "semantics": semantics,
        }),
        page: super::page(20, false, None, "complete"),
        warnings: Vec::new(),
        summary: "Kronika returned the exact 20 read-only historical tools, accepted lenses and cuts, limits, and semantics.".to_owned(),
    })
}

fn surfaces() -> Vec<Value> {
    crate::mcp::catalog::all()
        .iter()
        .map(|tool| {
            let mut surface = Map::new();
            surface.insert("tool".to_owned(), json!(tool.name.as_ref()));
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object);
            if let Some(lenses) = property_enum(properties, "lens") {
                surface.insert("lenses".to_owned(), lenses);
            }
            if tool.name == "kronika_rank_heatmap" {
                surface.insert("cuts".to_owned(), heatmap_cuts());
            }
            let limits = property_limits(properties);
            if !limits.is_empty() {
                surface.insert("limits".to_owned(), Value::Object(limits));
            }
            Value::Object(surface)
        })
        .collect()
}

fn property_enum(properties: Option<&Map<String, Value>>, name: &str) -> Option<Value> {
    properties?
        .get(name)?
        .get("enum")
        .filter(|value| value.is_array())
        .cloned()
}

fn property_limits(properties: Option<&Map<String, Value>>) -> Map<String, Value> {
    let Some(properties) = properties else {
        return Map::new();
    };
    properties
        .iter()
        .filter_map(|(name, schema)| {
            let constraints = ["minimum", "maximum", "default", "maxItems", "maxLength"]
                .into_iter()
                .filter_map(|constraint| {
                    schema
                        .get(constraint)
                        .cloned()
                        .map(|value| (constraint.to_owned(), value))
                })
                .collect::<Map<_, _>>();
            (!constraints.is_empty()).then(|| (name.clone(), Value::Object(constraints)))
        })
        .collect()
}

fn heatmap_cuts() -> Value {
    let mut by_surface = BTreeMap::<&str, Vec<&str>>::new();
    for spec in CUTS {
        by_surface.entry(spec.surface).or_default().push(spec.cut);
    }
    json!(by_surface)
}

fn global_limits() -> Value {
    json!({
        "request_body_bytes": REQUEST_BODY_BYTES,
        "response_body_bytes": RESPONSE_BODY_BYTES,
        "structured_content_default_bytes": super::super::DEFAULT_DATA_BYTES,
        "structured_content_bytes": STRUCTURED_CONTENT_BYTES,
        "text_summary_bytes": TEXT_SUMMARY_BYTES,
        "rows_per_page": 500,
        "fields": 32,
        "filters": 16,
        "logical_sections": 16,
        "history_identities": 16,
        "history_samples": 10_000,
        "timeline_records": 1_000,
        "large_text_chunk_bytes": 32 * 1_024,
        "calendar_window_us": HOUR_US.to_string(),
        "segments": MAX_SEGMENTS,
        "physical_row_visits": MAX_ROWS,
        "decoded_cells": super::super::DECODED_CELLS,
        "queue_wait_ms": super::super::QUEUE_WAIT.as_millis(),
        "context_deadline_seconds": super::super::CONTEXT_DEADLINE.as_secs(),
        "scan_deadline_seconds": super::super::SCAN_DEADLINE.as_secs(),
        "concurrent_heavy_scans": 2,
    })
}

fn source_values(configured: u32) -> Vec<Value> {
    [("os", SOURCE_OS), ("postgresql", SOURCE_POSTGRESQL)]
        .into_iter()
        .map(|(name, bit)| json!({"name": name, "configured": configured & bit != 0}))
        .collect()
}

#[cfg(test)]
mod tests;
