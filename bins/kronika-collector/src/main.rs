//! Linux system collector daemon.
//!
//! Configuration is environment-only; the one required variable is
//! `KRONIKA_OUT_DIR`. The process snapshots the OS sources on their own
//! intervals, appends each synchronized window to `<out>/active.wal`, and
//! publishes immutable `<out>/YYYY/MM/DD/<segment-id>.zms` segments by size,
//! age, journal pressure, or `SIGUSR2`.
//!
//! `SIGTERM` and `SIGINT` stop the loop without discarding the journal.
//! Startup recovery writes valid frames left by the preceding process. A failed
//! source is logged and retried on its next interval; bad startup
//! configuration, journal-open failures, and any persistence failure that
//! poisons the journal terminate the process.
#![allow(
    clippy::multiple_crate_versions,
    reason = "the registry's arrow/parquet stack pulls duplicate transitive versions outside our control"
)]

mod buffering;
mod capacity;
mod config;
mod logging;
mod os_sources;
mod rotation;
mod scheduler;
mod segments;
mod service_sections;

use anyhow::{Context, Result};
use config::Config;
use kronika_layout::{DataRoot, LayoutLimits, TemporaryKind, WriterOwner};
use kronika_source_os::{OsScope, ProcFs, detect_container};
use kronika_writer::{Journal, SectionBuffers};
use logging::{LogLevel, field, log_event};
use os_sources::{collect_os_sources, push_os_sources};
use rotation::Rotation;
use scheduler::{DueSet, Scheduler};
use segments::{
    SegmentState, append_window_and_maybe_close, close_open_segment, encode_window,
    open_collector_journal,
};
use service_sections::{collect_instance, push_instance_metadata};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::signal::unix::{SignalKind, signal};

/// How long to sleep before the next wake, or `None` to wait on signals only.
fn timer_sleep_delay(
    now: Instant,
    tick_secs: u64,
    segment_max_age_secs: u64,
    sched: &Scheduler,
    segment: &SegmentState,
    rotation: Option<&Rotation>,
) -> Option<Duration> {
    let mut delay = (tick_secs != 0).then(|| Duration::from_secs(tick_secs));
    if let Some(delay) = delay.as_mut() {
        if let Some(next_due) = sched.next_elapsed_due_in(now) {
            *delay = (*delay).min(next_due);
        }
        if let Some(next_age) =
            segment.time_until_age(now, Duration::from_secs(segment_max_age_secs))
        {
            *delay = (*delay).min(next_age);
        }
    }
    // Rotation runs its own timer, so an otherwise signal-only loop still wakes
    // for the periodic size re-check.
    if let Some(rotation) = rotation {
        let next_rotation = rotation.time_until_tick(now);
        delay = Some(delay.map_or(next_rotation, |delay| delay.min(next_rotation)));
    }
    delay
}

/// Delete temporaries a previous process left behind.
///
/// A temporary is a segment that was never published, so nothing reads it and
/// nothing is lost by removing it.
fn remove_writer_temporaries(owner: &WriterOwner, limits: LayoutLimits) -> Result<()> {
    let snapshot = owner
        .root()
        .scan(limits)
        .context("scan for stale writer temporaries")?;
    for temporary in &snapshot.temporaries {
        if temporary.kind != TemporaryKind::Zms {
            continue;
        }
        if let Err(error) = owner.remove_temporary(temporary) {
            log_event(
                LogLevel::Warn,
                "writer_temporary_remove_failed",
                &[field("error", format!("{error}"))],
            );
        }
    }
    Ok(())
}

fn prepare_collector_storage(
    owner: &WriterOwner,
    limits: LayoutLimits,
    journal_max_bytes: u64,
) -> Result<(Journal, Option<PathBuf>)> {
    remove_writer_temporaries(owner, limits)?;
    open_collector_journal(owner, journal_max_bytes)
}

/// Take exclusive ownership of the data root and recover what the previous
/// process left behind.
fn start_up(config: &Config) -> Result<(WriterOwner, Journal)> {
    std::fs::create_dir_all(&config.out_dir).context("create the output directory")?;
    let data_root = DataRoot::open(&config.out_dir).context("open the data root")?;
    let writer_owner = data_root
        .acquire_writer(LayoutLimits::default())
        .context("acquire exclusive writer ownership")?;
    // The journal lives next to finished segments so windows survive restarts.
    let (journal, recovered) = prepare_collector_storage(
        &writer_owner,
        LayoutLimits::default(),
        config.journal_max_bytes,
    )?;
    if let Some(dest) = recovered {
        announce(&format!("wrote {} reason=recovered", dest.display()));
    }
    Ok((writer_owner, journal))
}

fn main() -> Result<()> {
    if capacity::is_helper_invocation() {
        return capacity::run_helper();
    }
    run_collector()
}

#[tokio::main]
async fn run_collector() -> Result<()> {
    let config = Config::from_env()?;
    let (writer_owner, mut journal) = start_up(&config)?;

    let mut sigusr2 = signal(SignalKind::user_defined2()).context("install the SIGUSR2 handler")?;
    let mut sigterm = signal(SignalKind::terminate()).context("install the SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install the SIGINT handler")?;
    let mut sched = Scheduler::new(config.intervals);
    let mut segment = SegmentState::default();
    let mut rotation = Rotation::new(
        config.retention,
        &writer_owner,
        LayoutLimits::default(),
        Instant::now(),
    )?;
    // With the timer disabled collection is signal-driven only.
    let mut first_timer_tick = config.tick_secs > 0;

    announce("ready");

    loop {
        let sleep = if first_timer_tick {
            first_timer_tick = false;
            Some(Duration::ZERO)
        } else {
            timer_sleep_delay(
                Instant::now(),
                config.tick_secs,
                config.segment_max_age_secs,
                &sched,
                &segment,
                rotation.as_ref(),
            )
        };
        let forced = tokio::select! {
            Some(()) = sigusr2.recv() => true,
            () = async {
                match sleep {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // With the collection timer disabled the only timed wake is
                // rotation's; collection stays strictly signal-driven.
                if config.tick_secs == 0 {
                    run_rotation(&mut rotation, &writer_owner, &journal, &[]);
                    continue;
                }
                false
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        };
        let due = sched.plan(Instant::now(), forced);
        let mut written_this_tick: Vec<PathBuf> = Vec::new();
        // The age valve runs before collection: a tick whose sources fail or
        // return no rows must still close an expired segment.
        let age = Duration::from_secs(config.segment_max_age_secs);
        if segment.age_expired(Instant::now(), age) {
            let dest = close_open_segment(&mut journal, &writer_owner, &mut segment, "age")?;
            sched.mark_segment_opened();
            announce(&format!("wrote {} reason=age", dest.display()));
            written_this_tick.push(dest);
            stop_if_persistence_unhealthy(&journal)?;
        }
        if due.is_empty() {
            run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
            continue;
        }
        written_this_tick.extend(run_collection_cycle(
            &mut journal,
            &writer_owner,
            &config,
            &due,
            &mut segment,
            &mut sched,
        )?);
        stop_if_persistence_unhealthy(&journal)?;
        run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
    }
    Ok(())
}

/// One collection cycle: read the due sources, append a window, and write when
/// the segment is full. Source, encoding, and recoverable append failures skip
/// the window; segment-close failures terminate the daemon.
fn run_collection_cycle(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    due: &DueSet,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
) -> Result<Vec<PathBuf>> {
    let first = collect_and_append_window(journal, writer_owner, config, due, segment, sched)?;
    let mut written = first.written;
    if first.recollect {
        let recollection_due = sched.recollection_due(due, Instant::now());
        let second = collect_and_append_window(
            journal,
            writer_owner,
            config,
            &recollection_due,
            segment,
            sched,
        )?;
        anyhow::ensure!(
            !second.recollect,
            "a fresh segment unexpectedly requested another pre-append close"
        );
        written.extend(second.written);
    }
    Ok(written)
}

#[derive(Default)]
struct CollectionOutcome {
    written: Vec<PathBuf>,
    recollect: bool,
}

fn collect_and_append_window(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    due: &DueSet,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
) -> Result<CollectionOutcome> {
    let Some(ts) = collection_timestamp() else {
        return Ok(CollectionOutcome::default());
    };
    let fs = ProcFs::from_env();
    let in_container = detect_container(&fs);
    let mut buffers = SectionBuffers::new();

    let instance = if segment.is_empty() {
        match collect_instance() {
            Ok(instance) => Some(instance),
            Err(_logged) => return Ok(CollectionOutcome::default()),
        }
    } else {
        None
    };

    if let Some(facts) = instance.as_ref()
        && let Err(err) = push_instance_metadata(
            &mut buffers,
            segment.interner_mut(),
            facts,
            in_container,
            ts,
        )
    {
        log_event(
            LogLevel::Error,
            "window_buffer_failure",
            &[field("error", format!("{err:#}"))],
        );
        return Ok(CollectionOutcome::default());
    }

    let os = collect_os_sources(
        &fs,
        segment.interner_mut(),
        OsScope::Host.as_u8(),
        ts,
        in_container,
        due,
    );

    if let Err(err) = push_os_sources(&mut buffers, &os) {
        log_event(
            LogLevel::Error,
            "window_buffer_failure",
            &[field("error", format!("{err:#}"))],
        );
        return Ok(CollectionOutcome::default());
    }
    if buffers.is_empty() {
        return Ok(CollectionOutcome::default());
    }

    let flushed = match encode_window(buffers, segment.interner()) {
        Ok(flushed) => flushed,
        Err(err) => {
            log_event(
                LogLevel::Error,
                "window_encode_failure",
                &[field("error", format!("{err:#}"))],
            );
            return Ok(CollectionOutcome::default());
        }
    };
    match append_window_and_maybe_close(
        journal,
        writer_owner,
        config,
        segment,
        ts,
        due.forced(),
        &flushed,
    ) {
        Ok(finished) => {
            let recollect = finished
                .iter()
                .any(|(_, reason)| matches!(*reason, "format-limit" | "journal-full"));
            let mut dests = Vec::with_capacity(finished.len());
            for (dest, reason) in finished {
                sched.mark_segment_opened();
                announce(&format!("wrote {} reason={reason}", dest.display()));
                dests.push(dest);
            }
            Ok(CollectionOutcome {
                written: dests,
                recollect,
            })
        }
        Err(failure) => {
            let (close_failed, err) = failure.into_parts();
            log_event(
                LogLevel::Error,
                "window_append_failure",
                &[field("error", format!("{err:#}"))],
            );
            if close_failed {
                Err(err.context("close the segment for the collection window"))
            } else {
                Ok(CollectionOutcome::default())
            }
        }
    }
}

fn collection_timestamp() -> Option<i64> {
    match unix_now_us() {
        Ok(ts) => Some(ts),
        Err(err) => {
            log_event(
                LogLevel::Error,
                "collection_failed",
                &[field("reason", "clock"), field("error", format!("{err:#}"))],
            );
            None
        }
    }
}

fn unix_now_us() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?;
    i64::try_from(duration.as_micros()).context("system time exceeds i64 microseconds")
}

/// Feeds the tick's publications to rotation and lets it enforce the target.
///
/// A no-op when rotation is disabled. Publications grow the incremental size
/// counter; the enforcement itself is scan-free unless the tree is over target.
fn run_rotation(
    rotation: &mut Option<Rotation>,
    writer_owner: &WriterOwner,
    journal: &Journal,
    finished: &[PathBuf],
) {
    let Some(rotation) = rotation.as_mut() else {
        return;
    };
    for dest in finished {
        match std::fs::metadata(dest) {
            Ok(metadata) => rotation.record_publication(metadata.len()),
            // An uncounted publication under-counts the tree until the next
            // enforcement scan re-seeds the counter.
            Err(err) => log_event(
                LogLevel::Warn,
                "rotation_publication_stat_failure",
                &[
                    field("path", dest.display().to_string()),
                    field("error", format!("{err:#}")),
                ],
            ),
        }
    }
    let journal_bytes = u64::try_from(journal.bytes()).unwrap_or(u64::MAX);
    rotation.maybe_enforce(
        writer_owner,
        journal_bytes,
        !finished.is_empty(),
        Instant::now(),
    );
}

fn stop_if_persistence_unhealthy(journal: &Journal) -> Result<()> {
    if journal.is_poisoned() {
        anyhow::bail!(
            "active.wal entered an indeterminate persistence state; stop and recover it on restart"
        );
    }
    Ok(())
}

fn announce(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .ok();
}

#[cfg(test)]
mod tests;
