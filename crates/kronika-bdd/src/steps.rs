//! Step definitions for `features/collector.feature`.

use crate::BddWorld;
use crate::collector::{Run, files_under};
use anyhow::Result;
use cucumber::{given, then, when};
use std::time::Duration;

/// A `YYYY/MM/DD` prefix, as the layout writes it.
fn is_utc_calendar_path(relative: &std::path::Path) -> bool {
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

#[given("a collector that writes on every tick")]
fn close_every_tick(world: &mut BddWorld) -> Result<()> {
    let mut env = world.env.clone();
    env.push(("KRONIKA_INTERVAL_S", "1".to_owned()));
    env.push(("KRONIKA_SEGMENT_MAX_BYTES", "0".to_owned()));
    world.run = Some(Run::spawn(&env)?);
    Ok(())
}

#[given("a collector that keeps one open segment")]
fn keep_one_segment(world: &mut BddWorld) -> Result<()> {
    let mut env = world.env.clone();
    env.push(("KRONIKA_INTERVAL_S", "1".to_owned()));
    // Caps far above anything a few ticks can reach, so nothing writes.
    env.push(("KRONIKA_SEGMENT_MAX_BYTES", "1073741824".to_owned()));
    env.push(("KRONIKA_SEGMENT_MAX_AGE_S", "86400".to_owned()));
    world.run = Some(Run::spawn(&env)?);
    Ok(())
}

#[given("a procfs fixture without meminfo")]
fn procfs_without_meminfo(world: &mut BddWorld) -> Result<()> {
    let fixture = tempfile::tempdir()?;
    // Enough of /proc for the collector to start and read something, with
    // meminfo deliberately absent.
    std::fs::write(
        fixture.path().join("stat"),
        "cpu  1 2 3 4 5 6 7 8 9 10\nctxt 1\nbtime 1700000000\nprocesses 1\n\
         procs_running 1\nprocs_blocked 0\n",
    )?;
    std::fs::write(
        fixture.path().join("loadavg"),
        "0.10 0.20 0.30 1/200 4242\n",
    )?;
    std::fs::create_dir_all(fixture.path().join("sys/kernel/random"))?;
    std::fs::write(fixture.path().join("sys/kernel/hostname"), "bdd-host\n")?;
    std::fs::write(fixture.path().join("sys/kernel/osrelease"), "6.1.0-bdd\n")?;
    std::fs::write(
        fixture.path().join("sys/kernel/random/boot_id"),
        "00000000-0000-4000-8000-000000000000\n",
    )?;
    world.env.push((
        "KRONIKA_PROC_ROOT",
        fixture.path().to_string_lossy().into_owned(),
    ));
    world.fixture = Some(fixture);
    Ok(())
}

#[when(regex = r"^it runs for (\d+) seconds$")]
fn run_for(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let run = world.run.as_mut().expect("a collector was started");
    run.run_for_and_stop(Duration::from_secs(seconds))
}

#[then("a segment exists under a YYYY/MM/DD directory")]
fn segment_on_calendar_path(world: &mut BddWorld) {
    let run = world.run.as_ref().expect("a collector was started");
    let files = files_under(&run.out_dir());
    assert!(
        files
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "zms")
                && is_utc_calendar_path(path)),
        "no segment on a UTC calendar path in {files:?}"
    );
}

#[then("every published segment file ends in .zms")]
fn segments_are_zms(world: &mut BddWorld) {
    let run = world.run.as_ref().expect("a collector was started");
    for path in files_under(&run.out_dir()) {
        if is_utc_calendar_path(&path) {
            assert_eq!(
                path.extension().and_then(std::ffi::OsStr::to_str),
                Some("zms"),
                "{} is on the calendar tree but is not a segment",
                path.display()
            );
        }
    }
}

#[then("the raw journal is named active.wal")]
fn journal_is_active_wal(world: &mut BddWorld) {
    let run = world.run.as_ref().expect("a collector was started");
    assert!(
        run.out_dir().join("active.wal").exists(),
        "active.wal is missing from {:?}",
        files_under(&run.out_dir())
    );
}

#[then("the log has a segment_write_finish line")]
fn log_has_write_line(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    assert!(
        log.lines()
            .any(|line| line.contains("segment_write_finish")),
        "no write line in:\n{log}"
    );
    Ok(())
}

#[then(
    "that line names the segment path, the reason, the section count, the byte size, and the elapsed time"
)]
fn write_line_is_complete(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    let line = log
        .lines()
        .find(|line| line.contains("segment_write_finish"))
        .expect("a write line");
    for field in [
        "segment_path=",
        "reason=",
        "sections=",
        "segment_bytes=",
        "elapsed_ms=",
    ] {
        assert!(line.contains(field), "{field} missing from {line}");
    }
    Ok(())
}

#[then("the log reports os_meminfo as degraded")]
fn log_reports_degraded_meminfo(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    assert!(
        log.lines()
            .any(|line| line.contains("collection_degraded")
                && line.contains("collection=os_meminfo")),
        "no degraded meminfo line in:\n{log}"
    );
    Ok(())
}

#[then("the log still reports a segment written")]
fn log_still_writes(world: &mut BddWorld) -> Result<()> {
    log_has_write_line(world)
}

#[given("a data root whose journal is corrupt")]
fn corrupt_journal(world: &mut BddWorld) -> Result<()> {
    let root = tempfile::tempdir()?;
    let out_dir = root.path().join("segments");
    std::fs::create_dir_all(&out_dir)?;
    // A valid journal header magic followed by bytes that are not a frame.
    std::fs::write(out_dir.join("active.wal"), b"KRNJNL1\0not-a-valid-header")?;
    world.prepared_root = Some(root);
    Ok(())
}

#[when(regex = r"^the collector runs for (\d+) seconds$")]
fn collector_runs_for(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let root = world.prepared_root.take().expect("a prepared data root");
    let mut env = world.env.clone();
    env.push(("KRONIKA_INTERVAL_S", "1".to_owned()));
    env.push(("KRONIKA_SEGMENT_MAX_BYTES", "0".to_owned()));
    let mut run = Run::adopt(root, &env)?;
    run.run_for_and_stop(Duration::from_secs(seconds))?;
    world.run = Some(run);
    Ok(())
}

#[then("the log reports the journal as damaged")]
fn log_reports_damaged_journal(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    let line = log
        .lines()
        .find(|line| line.contains("journal_damaged"))
        .unwrap_or("");
    assert!(!line.is_empty(), "no journal_damaged line in:\n{log}");
    assert!(line.contains("path="), "{line} names no path");
    assert!(line.contains("reason="), "{line} names no reason");
    Ok(())
}

#[then("the corrupt bytes are kept next to the journal")]
fn damaged_bytes_are_kept(world: &mut BddWorld) {
    let run = world.run.as_ref().expect("a collector was started");
    let damaged = run.out_dir().join("active.wal.damaged");
    assert!(
        damaged.exists(),
        "active.wal.damaged is missing from {:?}",
        files_under(&run.out_dir())
    );
}

#[then("the log has no error line")]
fn log_has_no_error(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    let errors: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("level=error"))
        .collect();
    assert!(errors.is_empty(), "unexpected error lines: {errors:?}");
    Ok(())
}

#[when("its journal is cut short")]
fn cut_the_journal_short(world: &mut BddWorld) -> Result<()> {
    let run = world.run.take().expect("a collector was started");
    let journal = run.out_dir().join("active.wal");
    let len = std::fs::metadata(&journal)?.len();
    assert!(len > 512, "the journal is too small to cut: {len} bytes");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&journal)?
        .set_len(len - 200)?;
    world.prepared_root = Some(run.into_root());
    Ok(())
}

#[when(regex = r"^the collector runs again for (\d+) seconds$")]
fn collector_runs_again(world: &mut BddWorld, seconds: u64) -> Result<()> {
    collector_runs_for(world, seconds)
}

#[then("that segment was written from the salvaged windows")]
fn segment_came_from_salvage(world: &mut BddWorld) -> Result<()> {
    let log = world.run.as_ref().expect("a collector was started").log()?;
    let line = log
        .lines()
        .find(|line| line.contains("segment_write_finish") && line.contains("reason=recovered"))
        .unwrap_or("");
    assert!(
        !line.is_empty(),
        "no segment was written from the journal in:\n{log}"
    );
    let parts = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("journal_parts="))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(parts > 0, "{line} carries no salvaged window");
    Ok(())
}
