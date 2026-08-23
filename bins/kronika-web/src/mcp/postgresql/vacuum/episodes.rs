use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::super::PostgresqlFailure;
use super::policy::Policies;
use super::reader::{EpisodeKey, Sample, malformed};

const MONOTONE_FIELDS: &[&str] = &[
    "index_vacuum_count",
    "heap_blks_scanned",
    "heap_blks_vacuumed",
];

pub(super) struct Episode {
    pub(super) key: EpisodeKey,
    rows: Vec<Sample>,
    phase_start: usize,
}

impl Episode {
    fn first(&self) -> Result<&Sample, PostgresqlFailure> {
        self.rows
            .first()
            .ok_or_else(|| malformed("a Vacuum episode has no first sample"))
    }

    fn last(&self) -> Result<&Sample, PostgresqlFailure> {
        self.rows
            .last()
            .ok_or_else(|| malformed("a Vacuum episode has no latest sample"))
    }

    fn phase_rows(&self) -> &[Sample] {
        self.rows.get(self.phase_start..).unwrap_or(&[])
    }
}

pub(super) fn build_episodes(
    rows: Vec<Sample>,
    adjacency_limit: Option<i64>,
) -> Result<(Vec<Episode>, Option<i64>), PostgresqlFailure> {
    let at_timestamp = rows.iter().map(|sample| sample.timestamp).max();
    let mut streams = BTreeMap::<EpisodeKey, Vec<Sample>>::new();
    for sample in rows {
        streams.entry(sample.key.clone()).or_default().push(sample);
    }
    let mut episodes = Vec::new();
    for (key, mut stream) in streams {
        stream.sort_by_key(|sample| (sample.timestamp, sample.segment_id, sample.ordinal));
        let mut current = Vec::new();
        for sample in stream {
            let continues = current
                .last()
                .map(|previous| continues(previous, &sample, adjacency_limit))
                .transpose()?
                .unwrap_or(false);
            if !continues && !current.is_empty() {
                episodes.push(finish_episode(key.clone(), std::mem::take(&mut current))?);
            }
            current.push(sample);
        }
        if !current.is_empty() {
            episodes.push(finish_episode(key, current)?);
        }
    }
    Ok((episodes, at_timestamp))
}

fn continues(
    previous: &Sample,
    current: &Sample,
    adjacency_limit: Option<i64>,
) -> Result<bool, PostgresqlFailure> {
    let elapsed = current.timestamp.saturating_sub(previous.timestamp);
    if adjacency_limit.is_some_and(|limit| elapsed > limit) {
        return Ok(false);
    }
    for field in MONOTONE_FIELDS {
        if let (Some(before), Some(after)) = (previous.integer(field)?, current.integer(field)?)
            && after < before
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn finish_episode(key: EpisodeKey, rows: Vec<Sample>) -> Result<Episode, PostgresqlFailure> {
    let last = rows
        .last()
        .ok_or_else(|| malformed("a Vacuum episode has no latest sample"))?;
    let phase = last.phase()?;
    let cycle = last.integer("index_vacuum_count")?;
    let mut phase_start = rows.len().saturating_sub(1);
    while phase_start > 0 {
        let candidate = &rows[phase_start - 1];
        if candidate.phase()? != phase || candidate.integer("index_vacuum_count")? != cycle {
            break;
        }
        phase_start -= 1;
    }
    Ok(Episode {
        key,
        rows,
        phase_start,
    })
}

pub(super) fn sort_episodes(
    episodes: &mut [Episode],
    at_timestamp: Option<i64>,
    policies: &Policies,
) -> Result<(), PostgresqlFailure> {
    let mut failure = None;
    episodes.sort_by(|left, right| {
        if failure.is_some() {
            return Ordering::Equal;
        }
        match compare_episodes(left, right, at_timestamp, policies) {
            Ok(ordering) => ordering,
            Err(error) => {
                failure = Some(error);
                Ordering::Equal
            }
        }
    });
    failure.map_or(Ok(()), Err)
}

fn compare_episodes(
    left: &Episode,
    right: &Episode,
    at_timestamp: Option<i64>,
    policies: &Policies,
) -> Result<Ordering, PostgresqlFailure> {
    let left_last = left.last()?;
    let right_last = right.last()?;
    let left_at = at_timestamp == Some(left_last.timestamp);
    let right_at = at_timestamp == Some(right_last.timestamp);
    let active_order = right_at.cmp(&left_at);
    if active_order != Ordering::Equal {
        return Ok(active_order);
    }
    if left_at {
        let risk_order = policies
            .risk_position(policies.risk(left_last.phase()?))
            .cmp(&policies.risk_position(policies.risk(right_last.phase()?)));
        if risk_order != Ordering::Equal {
            return Ok(risk_order);
        }
        let span_order = phase_span(right)?.cmp(&phase_span(left)?);
        if span_order != Ordering::Equal {
            return Ok(span_order);
        }
        let cycle_order = right_last
            .integer("index_vacuum_count")?
            .unwrap_or(0)
            .cmp(&left_last.integer("index_vacuum_count")?.unwrap_or(0));
        if cycle_order != Ordering::Equal {
            return Ok(cycle_order);
        }
    }
    Ok(right_last
        .timestamp
        .cmp(&left_last.timestamp)
        .then_with(|| left.key.cmp(&right.key))
        .then_with(|| {
            left.first()
                .map(|sample| sample.timestamp)
                .unwrap_or(i64::MIN)
                .cmp(
                    &right
                        .first()
                        .map(|sample| sample.timestamp)
                        .unwrap_or(i64::MIN),
                )
        }))
}

fn phase_span(episode: &Episode) -> Result<i64, PostgresqlFailure> {
    let first = episode
        .phase_rows()
        .first()
        .ok_or_else(|| malformed("a Vacuum phase has no first sample"))?;
    Ok(episode.last()?.timestamp.saturating_sub(first.timestamp))
}

pub(super) fn episode_value(
    episode: &Episode,
    at_timestamp: Option<i64>,
    projected: &[String],
    policies: &Policies,
) -> Result<Value, PostgresqlFailure> {
    let first = episode.first()?;
    let last = episode.last()?;
    let phase_rows = episode.phase_rows();
    let phase_first = phase_rows
        .first()
        .ok_or_else(|| malformed("a Vacuum phase has no first sample"))?;
    let phase = last.phase()?;
    let at_sample = at_timestamp == Some(last.timestamp);
    let no_movement = no_movement(episode, policies)?;
    let progress = progress_series(&episode.rows)?;
    let locators = episode
        .rows
        .iter()
        .map(|sample| {
            json!({
                "segment_id": sample.segment_id.to_string(),
                "type_id": sample.type_id.to_string(),
                "row_ordinal": sample.ordinal.to_string(),
                "timestamp_us": sample.timestamp.to_string(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "identity": {
            "type_id": episode.key.type_id.to_string(),
            "pid": episode.key.pid,
            "datid": episode.key.datid,
            "relid": episode.key.relid,
        },
        "first_at_us": first.timestamp.to_string(),
        "last_at_us": last.timestamp.to_string(),
        "span_us": last.timestamp.saturating_sub(first.timestamp).to_string(),
        "sample_count": episode.rows.len(),
        "observation": {
            "kind": if at_sample { "at_sample" } else { "last_recorded" },
            "at_sample": at_sample,
            "timestamp_us": last.timestamp.to_string(),
        },
        "phase": {
            "name": phase,
            "risk": policies.risk(phase),
            "first_at_us": phase_first.timestamp.to_string(),
            "last_at_us": last.timestamp.to_string(),
            "span_us": last.timestamp.saturating_sub(phase_first.timestamp).to_string(),
            "sample_count": phase_rows.len(),
            "index_vacuum_count": last.value("index_vacuum_count").cloned().unwrap_or(Value::Null),
            "no_movement": no_movement,
        },
        "progress": progress,
        "latest_row": projected_row(last, projected)?,
        "sample_locators": locators,
    }))
}

fn projected_row(sample: &Sample, projected: &[String]) -> Result<Value, PostgresqlFailure> {
    let values = sample
        .row
        .get("values")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("a Vacuum row has no named values"))?;
    let mut selected = Map::new();
    let mut unavailable = Vec::new();
    for field in projected {
        if let Some(value) = values.get(field) {
            selected.insert(field.clone(), value.clone());
        } else {
            selected.insert(field.clone(), Value::Null);
            unavailable.push(field.clone());
        }
    }
    let mut row = sample.row.clone();
    row.insert("values".to_owned(), Value::Object(selected));
    row.insert("unavailable_fields".to_owned(), json!(unavailable));
    Ok(Value::Object(row))
}

fn no_movement(episode: &Episode, policies: &Policies) -> Result<Value, PostgresqlFailure> {
    let last = episode.last()?;
    let phase = last.phase()?;
    let Some(movement) = policies.movements.get(phase) else {
        return Ok(Value::Null);
    };
    let type_id = last.type_id.to_string();
    if movement
        .unavailable_type_ids
        .iter()
        .any(|unavailable| unavailable == &type_id)
    {
        return Ok(Value::Null);
    }
    let phase_rows = episode.phase_rows();
    let start = if movement.field == "phase" {
        0
    } else {
        let Some(reading) = last.integer(&movement.field)? else {
            return Ok(Value::Null);
        };
        let mut start = phase_rows.len().saturating_sub(1);
        while start > 0 && phase_rows[start - 1].integer(&movement.field)? == Some(reading) {
            start -= 1;
        }
        start
    };
    let still = &phase_rows[start..];
    if still.len() < policies.no_movement_samples {
        return Ok(Value::Null);
    }
    let first = still
        .first()
        .ok_or_else(|| malformed("a no-movement span has no first sample"))?;
    Ok(json!({
        "field": movement.field,
        "samples": still.len(),
        "span_us": last.timestamp.saturating_sub(first.timestamp).to_string(),
    }))
}

fn progress_series(rows: &[Sample]) -> Result<Value, PostgresqlFailure> {
    let mut points = Vec::new();
    for sample in rows {
        let (Some(scanned), Some(total)) = (
            sample.integer("heap_blks_scanned")?,
            sample.integer("heap_blks_total")?,
        ) else {
            continue;
        };
        if total <= 0 {
            continue;
        }
        let scanned_f64 = scanned
            .to_string()
            .parse::<f64>()
            .map_err(|_error| malformed("a Vacuum progress value cannot be represented"))?;
        let total_f64 = total
            .to_string()
            .parse::<f64>()
            .map_err(|_error| malformed("a Vacuum total cannot be represented"))?;
        let percent = (100.0 * scanned_f64 / total_f64).clamp(0.0, 100.0);
        points.push(json!({
            "timestamp_us": sample.timestamp.to_string(),
            "heap_blks_scanned": sample.value("heap_blks_scanned").cloned().unwrap_or(Value::Null),
            "heap_blks_total": sample.value("heap_blks_total").cloned().unwrap_or(Value::Null),
            "percent": percent,
        }));
    }
    Ok(json!({"heap_scan": points}))
}
