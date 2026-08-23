//! Bounded episode projection over exact recorded `PostgreSQL` Vacuum samples.

mod cadence;
mod episodes;
mod policy;
mod reader;

use serde_json::{Map, Value, json};

use self::cadence::recorded_cadence;
use self::episodes::{build_episodes, episode_value, sort_episodes};
use self::policy::{Policies, adjacency_limit};
#[cfg(test)]
use self::reader::{EpisodeKey, Sample};
use self::reader::{admit_samples, collect_hour, decode_hour};
use super::{
    HOUR_US, MAX_FIELDS, MAX_ROWS, PostgresqlFailure, PostgresqlPayload, State, failure, fields,
    input, page_size,
};

const SECTION: &str = "pg_stat_progress_vacuum";
const MAX_EPISODES: usize = MAX_ROWS;
const VACUUM_FIELDS: &[&str] = &[
    "ts",
    "pid",
    "datid",
    "datname",
    "relid",
    "schemaname",
    "relname",
    "is_autovacuum",
    "phase",
    "heap_blks_total",
    "heap_blks_scanned",
    "heap_blks_vacuumed",
    "index_vacuum_count",
    "max_dead_tuples",
    "num_dead_tuples",
    "max_dead_tuple_bytes",
    "dead_tuple_bytes",
    "num_dead_item_ids",
    "indexes_total",
    "indexes_processed",
    "delay_time",
];

pub(super) fn execute(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    if args.contains_key("find") {
        return Err(input(
            "find",
            "find is not supported by the shared Rust field registry for Vacuum",
        ));
    }
    if args.contains_key("cursor") {
        return Err(input(
            "cursor",
            "Vacuum episodes require one complete bounded native sample set",
        ));
    }
    let from = super::timestamp(args, "from_us")?;
    let to = super::timestamp(args, "to_us")?;
    if from > to || from.div_euclid(HOUR_US) != to.div_euclid(HOUR_US) {
        return Err(input(
            "to_us",
            "Vacuum intervals must be ordered and contained in one UTC hour",
        ));
    }
    let projected = projected_fields(args)?;
    let admitted_episodes = page_size(args)?;
    let policies = Policies::load()?;
    let collected = collect_hour(state, from, to, cancelled)?;
    let decoded = decode_hour(collected.records)?;
    admit_samples(&decoded.rows)?;
    let cadence = recorded_cadence(state, to, cancelled)?;
    let adjacency_limit = cadence
        .seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| adjacency_limit(seconds, policies.adjacency_factor))
        .transpose()?;
    let (mut episodes, at_timestamp) = build_episodes(decoded.rows, adjacency_limit)?;
    if episodes.len() > MAX_EPISODES || episodes.len() > admitted_episodes {
        return Err(failure(
            "whole_set_bound_exceeded",
            format!(
                "the complete Vacuum result has {} episodes, above the admitted {}",
                episodes.len(),
                admitted_episodes.min(MAX_EPISODES)
            ),
            Some("page_size"),
        ));
    }
    sort_episodes(&mut episodes, at_timestamp, &policies)?;
    let episode_values = episodes
        .iter()
        .map(|episode| episode_value(episode, at_timestamp, &projected, &policies))
        .collect::<Result<Vec<_>, _>>()?;
    let mut semantics = policies.definitions;
    semantics.extend(decoded.layouts.into_iter().map(|layout| {
        json!({
            "origin": "recorded",
            "source": "kronika_registry",
            "layout": layout,
        })
    }));
    if let Some(provenance) = cadence.provenance {
        semantics.push(provenance);
    }
    let returned = episode_values.len();
    let mut warnings = decoded.warnings;
    warnings.extend(cadence.warnings);
    Ok(PostgresqlPayload {
        anchor: json!({
            "hour_start_us": from.div_euclid(HOUR_US).saturating_mul(HOUR_US).to_string(),
            "requested_at_us": to.to_string(),
            "selected_at_us": at_timestamp.map(|timestamp| timestamp.to_string()),
            "segment_id": Value::Null,
            "active_wal_position": Value::Null,
        }),
        data: json!({
            "episodes": episode_values,
            "semantics": semantics,
        }),
        page: json!({
            "returned": returned,
            "truncated": false,
            "next_cursor": Value::Null,
            "stop_reason": "complete",
        }),
        warnings,
        summary: format!(
            "Returned {returned} complete recorded Vacuum episode summaries without causal classification."
        ),
    })
}

fn projected_fields(args: &Map<String, Value>) -> Result<Vec<String>, PostgresqlFailure> {
    let projected = fields(args, VACUUM_FIELDS)?;
    if projected.len() > MAX_FIELDS {
        return Err(input("fields", "fields may contain at most 32 names"));
    }
    if let Some(unknown) = projected
        .iter()
        .find(|field| !VACUUM_FIELDS.contains(&field.as_str()))
    {
        return Err(input(
            "fields",
            format!("Vacuum has no recorded field {unknown:?}"),
        ));
    }
    Ok(projected)
}

#[cfg(test)]
#[path = "vacuum/tests.rs"]
mod tests;
