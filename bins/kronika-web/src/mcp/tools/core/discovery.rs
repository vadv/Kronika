use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Map, Value, json};

use super::{Failure, HOUR_US, MAX_ROWS, MAX_SEGMENTS, Payload};
use crate::api::{catalog_metric_source_bit, catalog_source_bit, catalog_warning_value};
use crate::config::{SOURCE_FAMILIES, SOURCE_OS, SOURCE_POSTGRESQL};
use crate::heatmap_product::{HeatmapConversion, HeatmapCut, HeatmapPolicy, HeatmapSurface};
use crate::mcp::{
    REQUEST_BODY_BYTES, RESPONSE_BODY_BYTES, STRUCTURED_CONTENT_BYTES, State, TEXT_SUMMARY_BYTES,
};

const MAX_CONTEXT_LAYOUTS: usize = 256;
const MAX_CONTEXT_LAYOUT_PRESENCES: usize = 512;
const MAX_WARNING_RECORDS: usize = 64;

pub(super) fn payload(state: &State, cancelled: &impl Fn() -> bool) -> Result<Payload, Failure> {
    let recorded = recorded_context(state, cancelled)?;
    let semantics = crate::product_semantics::all()
        .map_err(|error| Failure::bounded("semantics_unreadable", error.to_string()))?;
    let heatmap_surfaces = crate::heatmap_product::surfaces()
        .map_err(|error| Failure::bounded("heatmap_registry_unreadable", error.to_string()))?;
    let heatmap_policy = crate::heatmap_product::policy()
        .map_err(|error| Failure::bounded("heatmap_registry_unreadable", error.to_string()))?;
    Ok(Payload {
        anchor: super::anchor(None, None, None, None),
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
            "surfaces": tool_surfaces(heatmap_surfaces, heatmap_policy),
            "semantics": semantics,
        }),
        page: super::page(20, false, None, "complete"),
        warnings: recorded.warnings,
        summary:
            "Returned 20 tool definitions, current layouts, lenses, cuts, limits, and semantics."
                .to_owned(),
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
    let reader = Reader::open(&state.data_root).map_err(|error| unreadable(&error))?;
    let discovery = reader
        .catalog_discovery()
        .map_err(|error| unreadable(&error))?;
    check_cancelled(cancelled)?;
    let mut latest = None;
    for (_from, to) in discovery.ranges() {
        check_cancelled(cancelled)?;
        latest = Some(latest.map_or(to, |current: i64| current.max(to)));
    }
    let listing = if let Some(latest) = latest {
        discovery
            .segments(latest..=latest)
            .map_err(|error| unreadable(&error))?
    } else {
        discovery.segments(..).map_err(|error| unreadable(&error))?
    };
    check_cancelled(cancelled)?;
    if listing.segments.len() > MAX_SEGMENTS {
        return Err(Failure::bounded(
            "segment_limit_exceeded",
            "The selected instant overlaps more than 64 segments.",
        ));
    }
    if listing.warnings.len() > MAX_WARNING_RECORDS {
        return Err(Failure::bounded(
            "warning_limit_exceeded",
            "Store warnings exceed their bounded result limit.",
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
                    "The selected instant contains more than 512 segment-layout presences.",
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
            "The selected instant contains more than 256 physical layouts.",
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

fn unreadable(error: &kronika_reader::ReaderError) -> Failure {
    Failure {
        code: "unreadable",
        message: error.to_string(),
        parameter: None,
        retryable: true,
    }
}

fn tool_surfaces(
    heatmap_surfaces: &[HeatmapSurface],
    heatmap_policy: &HeatmapPolicy,
) -> Vec<Value> {
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
                surface.insert("cuts".to_owned(), heatmap_cuts(heatmap_surfaces));
                surface.insert("groups".to_owned(), heatmap_groups(heatmap_surfaces));
                surface.insert(
                    "defaults".to_owned(),
                    heatmap_defaults(heatmap_surfaces, heatmap_policy),
                );
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

fn heatmap_cuts(surfaces: &[HeatmapSurface]) -> Value {
    let by_surface = surfaces
        .iter()
        .map(|surface| {
            (
                surface.id.as_str(),
                surface
                    .cuts
                    .iter()
                    .map(|cut| cut.id.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!(by_surface)
}

fn heatmap_groups(surfaces: &[HeatmapSurface]) -> Value {
    let by_surface = surfaces
        .iter()
        .map(|surface| {
            (
                surface.id.as_str(),
                surface
                    .groups
                    .iter()
                    .map(|group| group.id.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!(by_surface)
}

fn heatmap_defaults(surfaces: &[HeatmapSurface], policy: &HeatmapPolicy) -> Value {
    let by_surface = surfaces
        .iter()
        .map(|surface| {
            (
                surface.id.as_str(),
                json!({
                    "cut": surface.default_cut,
                    "group": surface.default_group,
                    "columns": surface.default_columns,
                    "top": policy.default_top,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!(by_surface)
}

pub(super) fn heatmap_semantic(surface: &HeatmapSurface, cut: &HeatmapCut) -> Value {
    json!({
        "id": format!("heatmap.{}.{}", surface.id, cut.id),
        "origin": "accepted_presentation",
        "fields": cut.fields,
        "value_unit": cut.raw_unit.name(),
        "values_scaled": false,
        "conversion": heatmap_conversion(&cut.conversion),
    })
}

fn heatmap_conversion(conversion: &HeatmapConversion) -> Value {
    match conversion {
        HeatmapConversion::Identity => Value::Null,
        HeatmapConversion::FixedMultiply {
            factor,
            target_unit,
        } => json!({
            "status": "not_applied",
            "operation": "multiply",
            "factor": factor.to_string(),
            "target_unit": target_unit.name(),
            "origin": "exact_unit_conversion",
        }),
        HeatmapConversion::RecordedMultiply {
            locator,
            target_unit,
        } => json!({
            "status": "not_applied",
            "operation": "multiply",
            "factor": Value::Null,
            "target_unit": target_unit.name(),
            "origin": "recorded",
            "locator": locator,
        }),
        HeatmapConversion::RecordedDivide {
            locator,
            target_unit,
        } => json!({
            "status": "not_applied",
            "operation": "divide",
            "factor": Value::Null,
            "target_unit": target_unit.name(),
            "origin": "recorded",
            "locator": locator,
        }),
    }
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
