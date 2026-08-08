//! Assertions against the files the collector published.

use super::{count, table_rows, type_id};
use crate::BddWorld;
use crate::collector::files_under;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use kronika_reader::{Reader, Segment};
use kronika_registry::Cell;
use std::path::Path;

/// `type_id` of `instance_metadata`.
const INSTANCE_METADATA: u32 = 1_021_001;
/// `type_id` of `os_cgroup_cpu`.
const OS_CGROUP_CPU: u32 = 1_201_001;

/// A `YYYY/MM/DD` prefix, as the layout writes it.
fn is_utc_calendar_path(relative: &Path) -> bool {
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect();
    let [year, month, day, _file] = parts.as_slice() else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Every segment the run published, oldest first, read the way web reads.
fn segments(world: &BddWorld) -> Result<Vec<Segment>> {
    let run = world.run.as_ref().context("a collector was started")?;
    let out_dir = run.out_dir();
    let reader = Reader::open(&out_dir)?;
    let listing = reader.segments(..)?;
    anyhow::ensure!(
        listing.warnings.is_empty(),
        "the scan set aside files under {}: {:?}",
        out_dir.display(),
        listing.warnings
    );
    anyhow::ensure!(
        !listing.segments.is_empty(),
        "no segment was published under {}",
        out_dir.display()
    );
    listing
        .segments
        .iter()
        .map(|unit| reader.open_segment(unit).map_err(Into::into))
        .collect()
}

/// One decoded cell as the text a scenario writes for it.
fn render(cell: &Cell) -> String {
    match cell {
        Cell::I16(value) => value.to_string(),
        Cell::I32(value) => value.to_string(),
        Cell::I64(value) | Cell::Ts(value) => value.to_string(),
        Cell::U32(value) => value.to_string(),
        Cell::U64(value) | Cell::StrId(value) => value.to_string(),
        Cell::F64(value) => value.to_string(),
        Cell::Bool(value) => value.to_string(),
        Cell::ListI32(values) => format!("{values:?}"),
        Cell::Null => "null".to_owned(),
    }
}

#[then("a segment exists under a YYYY/MM/DD directory")]
fn segment_on_calendar_path(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let files = files_under(&run.out_dir());
    anyhow::ensure!(
        files
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "zms")
                && is_utc_calendar_path(path)),
        "no segment on a UTC calendar path in {files:?}"
    );
    Ok(())
}

#[then("every published segment file ends in .zms")]
fn segments_are_zms(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    for path in files_under(&run.out_dir()) {
        if is_utc_calendar_path(&path) {
            anyhow::ensure!(
                path.extension().and_then(std::ffi::OsStr::to_str) == Some("zms"),
                "{} is on the calendar tree but is not a segment",
                path.display()
            );
        }
    }
    Ok(())
}

#[then("the raw journal is named active.wal")]
fn journal_is_active_wal(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    anyhow::ensure!(
        run.out_dir().join("active.wal").exists(),
        "active.wal is missing from {:?}",
        files_under(&run.out_dir())
    );
    Ok(())
}

#[then("no segment exists under a YYYY/MM/DD directory")]
fn no_segment_on_calendar_path(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let files = files_under(&run.out_dir());
    anyhow::ensure!(
        files
            .iter()
            .all(|path| path.extension().is_none_or(|ext| ext != "zms")),
        "unexpected segment under the data root: {files:?}"
    );
    Ok(())
}

#[then("every segment holds these sections")]
fn every_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "section", "min rows"])?;
    for segment in segments(world)? {
        for row in &wanted {
            let [id, name, min] = row.as_slice() else {
                anyhow::bail!("a section row needs a type_id, a name and a row floor, got {row:?}");
            };
            let id = type_id(id)?;
            let rows = segment.rows_of(id).with_context(|| {
                format!(
                    "{} carries no {name} ({id}); it has {:?}",
                    segment.path().display(),
                    segment.type_ids().collect::<Vec<_>>()
                )
            })?;
            anyhow::ensure!(
                rows >= count(min)?,
                "{} holds {rows} rows of {name} ({id}), fewer than {min}",
                segment.path().display()
            );
        }
    }
    Ok(())
}

#[then("no segment holds these sections")]
fn no_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let unwanted = table_rows(step, &["type_id", "section"])?;
    for segment in segments(world)? {
        for row in &unwanted {
            let [id, name] = row.as_slice() else {
                anyhow::bail!("a section row needs a type_id and a name, got {row:?}");
            };
            let id = type_id(id)?;
            anyhow::ensure!(
                segment.rows_of(id).is_none(),
                "{} carries {name} ({id}) with {:?} rows; the source was unreadable, so the \
                 section belongs nowhere in the segment",
                segment.path().display(),
                segment.rows_of(id)
            );
        }
    }
    Ok(())
}

#[then("some segment holds these sections")]
fn some_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "section", "min rows"])?;
    let segments = segments(world)?;
    for row in &wanted {
        let [id, name, min] = row.as_slice() else {
            anyhow::bail!("a section row needs a type_id, a name and a row floor, got {row:?}");
        };
        let id = type_id(id)?;
        let floor = count(min)?;
        anyhow::ensure!(
            segments
                .iter()
                .any(|segment| segment.rows_of(id).is_some_and(|rows| rows >= floor)),
            "none of the {} segments holds {min} rows of {name} ({id})",
            segments.len()
        );
    }
    Ok(())
}

#[then("every segment records these instance facts")]
fn instance_facts(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["column", "value"])?;
    for segment in segments(world)? {
        let rows = segment.rows(INSTANCE_METADATA)?;
        anyhow::ensure!(
            rows.len() == 1,
            "{} carries {} instance_metadata rows, expected exactly one",
            segment.path().display(),
            rows.len()
        );
        let row = rows.first().with_context(|| {
            format!(
                "{} carries no instance_metadata row",
                segment.path().display()
            )
        })?;
        for expected in &wanted {
            let [column, value] = expected.as_slice() else {
                anyhow::bail!("an instance-fact row needs a column and a value, got {expected:?}");
            };
            let cell = row
                .get(column)
                .with_context(|| format!("instance_metadata has no column {column}"))?;
            anyhow::ensure!(
                render(cell) == *value,
                "{} records {column}={}, not {value}",
                segment.path().display(),
                render(cell)
            );
        }
    }
    Ok(())
}

#[then(regex = r"^some segment records a cgroup CPU limit of (\d+) cores$")]
fn cgroup_cpu_limit(world: &mut BddWorld, cores: i64) -> Result<()> {
    let mut seen = Vec::new();
    for segment in segments(world)? {
        for row in segment.rows(OS_CGROUP_CPU)? {
            let (Some(&Cell::I64(quota)), Some(&Cell::I64(period))) =
                (row.get("quota_usec"), row.get("period_usec"))
            else {
                continue;
            };
            if period > 0 && quota > 0 {
                seen.push(quota / period);
                if quota == cores * period {
                    return Ok(());
                }
            }
        }
    }
    anyhow::bail!("no cgroup records a {cores}-core quota; the limits found were {seen:?}")
}

#[then(regex = r"^every segment covers at least (\d+) windows$")]
fn window_count(world: &mut BddWorld, least: u32) -> Result<()> {
    for segment in segments(world)? {
        anyhow::ensure!(
            segment.window_count() >= least,
            "{} covers {} windows, fewer than {least}",
            segment.path().display(),
            segment.window_count()
        );
    }
    Ok(())
}

#[then("every segment ends later than it starts")]
fn segment_time_span(world: &mut BddWorld) -> Result<()> {
    for segment in segments(world)? {
        anyhow::ensure!(
            segment.max_ts() > segment.min_ts(),
            "{} spans no time: min_ts {} equals max_ts {}",
            segment.path().display(),
            segment.min_ts(),
            segment.max_ts()
        );
    }
    Ok(())
}

#[then(regex = r"^its peak RSS stays under (\d+) MB$")]
fn peak_rss(world: &mut BddWorld, limit_mb: u64) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let peak = run
        .peak_rss_kib()
        .context("the run recorded no VmHWM; the process was gone before it was sampled")?;
    let limit_kib = limit_mb * 1024;
    anyhow::ensure!(
        peak <= limit_kib,
        "peak RSS was {peak} KiB, over the {limit_mb} MB limit ({limit_kib} KiB)"
    );
    Ok(())
}

#[then("some segment records these log events")]
fn log_events_recorded(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "column", "value"])?;
    let segments = segments(world)?;
    for expected in &wanted {
        let [id, column, value] = expected.as_slice() else {
            anyhow::bail!(
                "a log-event row needs a type_id, a column and a value, got {expected:?}"
            );
        };
        let id = type_id(id)?;
        anyhow::ensure!(
            segments
                .iter()
                .any(|segment| holds_value(segment, id, column, value).unwrap_or(false)),
            "none of the {} segments records {column}={value} in {id}; \
             the segments hold {:?}",
            segments.len(),
            seen_values(&segments, id, column)
        );
    }
    Ok(())
}

/// Every value the segments carry in one column, for a failure message that
/// says what is there instead of only what is not.
fn seen_values(segments: &[Segment], type_id: u32, column: &str) -> Vec<String> {
    let mut seen: Vec<String> = segments
        .iter()
        .filter_map(|segment| {
            let strings = segment.strings().ok()?;
            let rows = segment.rows(type_id).ok()?;
            Some(
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| match cell {
                        Cell::StrId(id) => {
                            strings.get(*id).map_or_else(|| render(cell), str::to_owned)
                        }
                        other => render(other),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

#[then("some segment records these log events exactly once")]
fn log_events_recorded_once(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "column", "value"])?;
    let segments = segments(world)?;
    for expected in &wanted {
        let [id, column, value] = expected.as_slice() else {
            anyhow::bail!(
                "a log-event row needs a type_id, a column and a value, got {expected:?}"
            );
        };
        let id = type_id(id)?;
        let mut seen = 0_usize;
        for segment in &segments {
            seen += matching_rows(segment, id, column, value)?;
        }
        anyhow::ensure!(
            seen == 1,
            "{id} records {column}={value} {seen} times across {} segments, not once",
            segments.len()
        );
    }
    Ok(())
}

/// How many rows of `type_id` carry `value` in `column`.
///
/// A dictionary column is compared against the text the segment interned, not
/// against the id it was interned under.
fn matching_rows(segment: &Segment, type_id: u32, column: &str, value: &str) -> Result<usize> {
    let strings = segment.strings()?;
    let render_cell = |cell: &Cell| match cell {
        Cell::StrId(id) => strings.get(*id).map_or_else(|| render(cell), str::to_owned),
        other => render(other),
    };
    Ok(segment
        .rows(type_id)?
        .iter()
        .filter(|row| {
            row.get(column)
                .is_some_and(|cell| render_cell(cell) == value)
        })
        .count())
}

fn holds_value(segment: &Segment, type_id: u32, column: &str, value: &str) -> Result<bool> {
    matching_rows(segment, type_id, column, value).map(|rows| rows > 0)
}
