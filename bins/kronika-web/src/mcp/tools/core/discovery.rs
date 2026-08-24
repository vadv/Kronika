use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Map, Value, json};

use super::{Failure, HOUR_US, MAX_ROWS, MAX_SEGMENTS, Payload};
use crate::api::{catalog_metric_source_bit, catalog_source_bit, catalog_warning_value};
use crate::config::{SOURCE_FAMILIES, SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::{
    REQUEST_BODY_BYTES, RESPONSE_BODY_BYTES, STRUCTURED_CONTENT_BYTES, State, TEXT_SUMMARY_BYTES,
};

const MAX_CONTEXT_LAYOUTS: usize = 256;
const MAX_CONTEXT_LAYOUT_PRESENCES: usize = 512;
const MAX_WARNING_RECORDS: usize = 64;

pub(super) struct HeatmapCut {
    pub(super) surface: &'static str,
    pub(super) cut: &'static str,
    pub(super) section: &'static str,
    pub(super) fields: &'static [&'static str],
    pub(super) labels: &'static [&'static str],
    raw_unit: HeatmapUnit,
    scale: HeatmapScale,
}

#[derive(Clone, Copy)]
enum HeatmapUnit {
    Blocks,
    Bytes,
    ClockTicks,
    Count,
    Kibibytes,
    Microseconds,
    Milliseconds,
    Nanoseconds,
    Seconds,
}

impl HeatmapUnit {
    const fn name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Bytes => "bytes",
            Self::ClockTicks => "clock_ticks",
            Self::Count => "count",
            Self::Kibibytes => "kibibytes",
            Self::Microseconds => "microseconds",
            Self::Milliseconds => "milliseconds",
            Self::Nanoseconds => "nanoseconds",
            Self::Seconds => "seconds",
        }
    }
}

#[derive(Clone, Copy)]
enum HeatmapScale {
    Identity,
    FixedMultiply {
        factor: u64,
        target: HeatmapUnit,
    },
    RecordedMultiply {
        locator: &'static str,
        target: HeatmapUnit,
    },
    RecordedDivide {
        locator: &'static str,
        target: HeatmapUnit,
    },
}

impl HeatmapCut {
    const fn new(
        surface: &'static str,
        cut: &'static str,
        section: &'static str,
        fields: &'static [&'static str],
        labels: &'static [&'static str],
        raw_unit: HeatmapUnit,
        scale: HeatmapScale,
    ) -> Self {
        Self {
            surface,
            cut,
            section,
            fields,
            labels,
            raw_unit,
            scale,
        }
    }

    pub(super) fn semantic(&self) -> Value {
        json!({
            "id": format!("heatmap.{}.{}", self.surface, self.cut),
            "origin": "accepted_presentation",
            "fields": self.fields,
            "value_unit": self.raw_unit.name(),
            "values_scaled": false,
            "conversion": self.scale.semantic(),
        })
    }
}

impl HeatmapScale {
    fn semantic(self) -> Value {
        match self {
            Self::Identity => Value::Null,
            Self::FixedMultiply { factor, target } => json!({
                "status": "not_applied",
                "operation": "multiply",
                "factor": factor.to_string(),
                "target_unit": target.name(),
                "origin": "exact_unit_conversion",
            }),
            Self::RecordedMultiply { locator, target } => json!({
                "status": "not_applied",
                "operation": "multiply",
                "factor": Value::Null,
                "target_unit": target.name(),
                "origin": "recorded",
                "locator": locator,
            }),
            Self::RecordedDivide { locator, target } => json!({
                "status": "not_applied",
                "operation": "divide",
                "factor": Value::Null,
                "target_unit": target.name(),
                "origin": "recorded",
                "locator": locator,
            }),
        }
    }
}

const CUTS: &[HeatmapCut] = &[
    HeatmapCut::new(
        "processes",
        "cpu",
        "os_process",
        &["utime", "stime"],
        &[],
        HeatmapUnit::ClockTicks,
        HeatmapScale::RecordedDivide {
            locator: "instance_metadata.clock_ticks_per_sec",
            target: HeatmapUnit::Seconds,
        },
    ),
    HeatmapCut::new(
        "processes",
        "rss",
        "os_process",
        &["rmem_kb"],
        &[],
        HeatmapUnit::Kibibytes,
        HeatmapScale::FixedMultiply {
            factor: 1_024,
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "processes",
        "io_read",
        "os_process",
        &["read_bytes"],
        &[],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "processes",
        "io_write",
        "os_process",
        &["write_bytes"],
        &[],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "processes",
        "majflt",
        "os_process",
        &["majflt"],
        &[],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "processes",
        "run_delay",
        "os_process",
        &["rundelay_ns"],
        &[],
        HeatmapUnit::Nanoseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "statements",
        "exec_time",
        "pg_stat_statements",
        &["total_exec_time"],
        &["datname", "usename"],
        HeatmapUnit::Milliseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "statements",
        "calls",
        "pg_stat_statements",
        &["calls"],
        &["datname", "usename"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "statements",
        "rows",
        "pg_stat_statements",
        &["rows"],
        &["datname", "usename"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "statements",
        "shared_read",
        "pg_stat_statements",
        &["shared_blks_read"],
        &["datname", "usename"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "statements",
        "shared_dirtied",
        "pg_stat_statements",
        &["shared_blks_dirtied"],
        &["datname", "usename"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "statements",
        "temp_written",
        "pg_stat_statements",
        &["temp_blks_written"],
        &["datname", "usename"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "statements",
        "wal_bytes",
        "pg_stat_statements",
        &["wal_bytes"],
        &["datname", "usename"],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "plans",
        "exec_time",
        "pg_store_plans",
        &["total_time"],
        &["datname", "usename"],
        HeatmapUnit::Milliseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "plans",
        "calls",
        "pg_store_plans",
        &["calls"],
        &["datname", "usename"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "plans",
        "rows",
        "pg_store_plans",
        &["rows"],
        &["datname", "usename"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "plans",
        "shared_read",
        "pg_store_plans",
        &["shared_blks_read"],
        &["datname", "usename"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "plans",
        "temp_written",
        "pg_store_plans",
        &["temp_blks_written"],
        &["datname", "usename"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "databases",
        "commits",
        "pg_stat_database",
        &["xact_commit"],
        &["datname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "databases",
        "rollbacks",
        "pg_stat_database",
        &["xact_rollback"],
        &["datname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "databases",
        "db_read",
        "pg_stat_database",
        &["blks_read"],
        &["datname"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "databases",
        "temp_bytes",
        "pg_stat_database",
        &["temp_bytes"],
        &["datname"],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "databases",
        "deadlocks",
        "pg_stat_database",
        &["deadlocks"],
        &["datname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "tables",
        "writes",
        "pg_stat_user_tables",
        &["n_tup_ins", "n_tup_upd", "n_tup_del"],
        &["datname", "schemaname", "relname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "tables",
        "seq_read",
        "pg_stat_user_tables",
        &["seq_tup_read"],
        &["datname", "schemaname", "relname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "tables",
        "heap_read",
        "pg_stat_user_tables",
        &["heap_blks_read"],
        &["datname", "schemaname", "relname"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "tables",
        "dead_tuples",
        "pg_stat_user_tables",
        &["n_dead_tup"],
        &["datname", "schemaname", "relname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "tables",
        "autovacuum_time",
        "pg_stat_user_tables",
        &["total_autovacuum_time"],
        &["datname", "schemaname", "relname"],
        HeatmapUnit::Milliseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_scan",
        "pg_stat_user_indexes",
        &["idx_scan"],
        &["datname", "schemaname", "relname", "indexrelname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_tup_read",
        "pg_stat_user_indexes",
        &["idx_tup_read"],
        &["datname", "schemaname", "relname", "indexrelname"],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "indexes",
        "idx_blks_read",
        "pg_stat_user_indexes",
        &["idx_blks_read"],
        &["datname", "schemaname", "relname", "indexrelname"],
        HeatmapUnit::Blocks,
        HeatmapScale::RecordedMultiply {
            locator: "pg_settings.block_size",
            target: HeatmapUnit::Bytes,
        },
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_cpu",
        "os_cgroup_cpu",
        &["usage_usec"],
        &[],
        HeatmapUnit::Microseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_throttled",
        "os_cgroup_cpu",
        &["throttled_usec"],
        &[],
        HeatmapUnit::Microseconds,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_read",
        "os_cgroup_io",
        &["rbytes"],
        &[],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_write",
        "os_cgroup_io",
        &["wbytes"],
        &[],
        HeatmapUnit::Bytes,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_rios",
        "os_cgroup_io",
        &["rios"],
        &[],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
    HeatmapCut::new(
        "cgroups",
        "cg_wios",
        "os_cgroup_io",
        &["wios"],
        &[],
        HeatmapUnit::Count,
        HeatmapScale::Identity,
    ),
];

pub(super) fn heatmap_cut(surface: &str, cut: &str) -> Option<&'static HeatmapCut> {
    CUTS.iter()
        .find(|candidate| candidate.surface == surface && candidate.cut == cut)
}

pub(super) fn payload(state: &State, cancelled: &impl Fn() -> bool) -> Result<Payload, Failure> {
    let recorded = recorded_context(state, cancelled)?;
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
                "recorded": recorded.value,
                "limits": global_limits(),
            },
            "surfaces": surfaces(),
            "semantics": semantics,
        }),
        page: super::page(20, false, None, "complete"),
        warnings: recorded.warnings,
        summary: "Kronika returned the exact 20 read-only historical tools, latest recorded layouts, accepted lenses and cuts, limits, and semantics.".to_owned(),
    })
}

struct RecordedContext {
    value: Value,
    warnings: Vec<Value>,
}

fn recorded_context(
    state: &State,
    cancelled: &impl Fn() -> bool,
) -> Result<RecordedContext, Failure> {
    check_cancelled(cancelled)?;
    let reader = Reader::open(&state.data_root).map_err(unreadable)?;
    let discovery = reader.catalog_discovery().map_err(unreadable)?;
    check_cancelled(cancelled)?;
    let mut latest = None;
    for (_from, to) in discovery.ranges() {
        check_cancelled(cancelled)?;
        latest = Some(latest.map_or(to, |current: i64| current.max(to)));
    }
    let listing = if let Some(latest) = latest {
        discovery.segments(latest..=latest).map_err(unreadable)?
    } else {
        discovery.segments(..).map_err(unreadable)?
    };
    check_cancelled(cancelled)?;
    if listing.segments.len() > MAX_SEGMENTS {
        return Err(Failure::bounded(
            "segment_limit_exceeded",
            "The latest recorded instant overlaps more than 64 segments.",
        ));
    }
    if listing.warnings.len() > MAX_WARNING_RECORDS {
        return Err(Failure::bounded(
            "warning_limit_exceeded",
            "The recorded store warnings exceed their bounded result limit.",
        ));
    }
    let value = latest.map_or_else(
        || Ok(empty_recorded_context()),
        |latest| recorded_value(latest, &listing.segments, cancelled),
    )?;
    let warnings = listing.warnings.iter().map(catalog_warning_value).collect();
    Ok(RecordedContext { value, warnings })
}

fn recorded_value(
    latest: i64,
    segments: &[SegmentRef],
    cancelled: &impl Fn() -> bool,
) -> Result<Value, Failure> {
    let mut layouts = BTreeMap::<u32, BTreeSet<i64>>::new();
    let mut layout_presences = 0_usize;
    let mut present_sources = 0_u32;
    let mut metric_sources = 0_u32;
    for segment in segments {
        check_cancelled(cancelled)?;
        for section in segment.sections() {
            check_cancelled(cancelled)?;
            layout_presences = layout_presences.saturating_add(1);
            if layout_presences > MAX_CONTEXT_LAYOUT_PRESENCES {
                return Err(Failure::bounded(
                    "layout_limit_exceeded",
                    "The latest recorded instant contains more than 512 segment-layout presences.",
                ));
            }
            layouts
                .entry(section.type_id)
                .or_default()
                .insert(segment.id());
            present_sources |= catalog_source_bit(section.type_id).unwrap_or(0);
            metric_sources |= catalog_metric_source_bit(section.type_id).unwrap_or(0);
        }
    }
    if layouts.len() > MAX_CONTEXT_LAYOUTS {
        return Err(Failure::bounded(
            "layout_limit_exceeded",
            "The latest recorded instant contains more than 256 physical layouts.",
        ));
    }
    Ok(json!({
        "as_of_us": latest.to_string(),
        "source_families": recorded_sources(present_sources, metric_sources),
        "layouts": layouts
            .into_iter()
            .map(|(type_id, segment_ids)| layout_value(type_id, segment_ids))
            .collect::<Vec<_>>(),
        "segments": segments.iter().map(segment_value).collect::<Vec<_>>(),
    }))
}

fn empty_recorded_context() -> Value {
    json!({
        "as_of_us": null,
        "source_families": recorded_sources(0, 0),
        "layouts": [],
        "segments": [],
    })
}

fn recorded_sources(present: u32, metrics: u32) -> Vec<Value> {
    [("os", SOURCE_OS), ("postgresql", SOURCE_POSTGRESQL)]
        .into_iter()
        .map(|(name, bit)| {
            json!({
                "name": name,
                "present": present & bit != 0,
                "metrics_present": metrics & bit != 0,
            })
        })
        .collect()
}

fn layout_value(type_id: u32, segment_ids: BTreeSet<i64>) -> Value {
    json!({
        "logical_name": logical_section_name(type_id),
        "physical_name": section_name(type_id),
        "type_id": type_id.to_string(),
        "implementation": section_implementation(type_id),
        "source_family": source_name(catalog_source_bit(type_id)),
        "segment_ids": segment_ids
            .into_iter()
            .map(|segment_id| segment_id.to_string())
            .collect::<Vec<_>>(),
    })
}

fn source_name(bit: Option<u32>) -> Option<&'static str> {
    let bit = bit?;
    SOURCE_FAMILIES
        .iter()
        .find(|family| family.bit == bit)
        .map(|family| family.name)
}

fn segment_value(segment: &SegmentRef) -> Value {
    json!({
        "segment_id": segment.id().to_string(),
        "kind": match segment.kind() {
            SegmentKind::Finished => "finished",
            SegmentKind::Active => "active",
        },
        "min_ts_us": segment.min_ts().to_string(),
        "max_ts_us": segment.max_ts().to_string(),
        "active_wal_position": segment.active_position().map(|value| value.to_string()),
    })
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), Failure> {
    if cancelled() {
        Err(Failure {
            code: "cancelled",
            message: "The historical read was cancelled.".to_owned(),
            parameter: None,
            retryable: true,
        })
    } else {
        Ok(())
    }
}

fn unreadable(error: kronika_reader::ReaderError) -> Failure {
    Failure {
        code: "unreadable",
        message: error.to_string(),
        parameter: None,
        retryable: true,
    }
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
        "context_layouts": MAX_CONTEXT_LAYOUTS,
        "context_layout_presences": MAX_CONTEXT_LAYOUT_PRESENCES,
        "store_warnings": MAX_WARNING_RECORDS,
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
