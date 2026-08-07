//! Assertions against the files the collector published.

use super::{count, table_rows, type_id};
use crate::BddWorld;
use crate::collector::files_under;
use crate::segment::{Segment, segments_under};
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
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

/// Every segment the run published, oldest first.
fn segments(world: &BddWorld) -> Result<Vec<Segment>> {
    let run = world.run.as_ref().context("a collector was started")?;
    segments_under(&run.out_dir())
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
                    segment.path.display(),
                    segment
                        .catalog
                        .entries
                        .iter()
                        .map(|e| e.type_id)
                        .collect::<Vec<_>>()
                )
            })?;
            anyhow::ensure!(
                rows >= count(min)?,
                "{} holds {rows} rows of {name} ({id}), fewer than {min}",
                segment.path.display()
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
                segment.path.display(),
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
        let rows = segment.decode(INSTANCE_METADATA)?;
        anyhow::ensure!(
            rows.len() == 1,
            "{} carries {} instance_metadata rows, expected exactly one",
            segment.path.display(),
            rows.len()
        );
        let row = rows.first().with_context(|| {
            format!(
                "{} carries no instance_metadata row",
                segment.path.display()
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
                segment.path.display(),
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
        for row in segment.decode(OS_CGROUP_CPU)? {
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
            segment.catalog.window_count >= least,
            "{} covers {} windows, fewer than {least}",
            segment.path.display(),
            segment.catalog.window_count
        );
    }
    Ok(())
}

#[then("every segment ends later than it starts")]
fn segment_time_span(world: &mut BddWorld) -> Result<()> {
    for segment in segments(world)? {
        anyhow::ensure!(
            segment.catalog.max_ts > segment.catalog.min_ts,
            "{} spans no time: min_ts {} equals max_ts {}",
            segment.path.display(),
            segment.catalog.min_ts,
            segment.catalog.max_ts
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
