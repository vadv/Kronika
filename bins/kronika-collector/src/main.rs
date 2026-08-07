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
mod config;
mod logging;
mod os_sources;
mod producer_status;
mod rotation;
mod scheduler;
mod segments;
mod service_sections;

use anyhow::{Context, Result};
use config::Config;
use kronika_layout::{DataRoot, LayoutLimits, TemporaryKind, WriterOwner};
use kronika_source_os::{OsScope, ProcFs, detect_container};
use kronika_writer::{Interner, Journal, SectionBuffers};
use logging::{LogLevel, field, log_event};
use os_sources::{collect_os_sources, push_os_sources};
use producer_status::{ProducerStatusPublisher, retention_status};
use rotation::Rotation;
use scheduler::{DueSet, Scheduler};
use segments::{
    SegmentState, append_window_and_maybe_close, close_open_segment, encode_window,
    open_collector_journal,
};
use service_sections::{collect_due_instance, push_instance_metadata};
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
fn start_up(config: &Config) -> Result<(WriterOwner, ProducerStatusPublisher, Journal)> {
    std::fs::create_dir_all(&config.out_dir).context("create the output directory")?;
    let data_root = DataRoot::open(&config.out_dir).context("open the data root")?;
    let writer_owner = data_root
        .acquire_writer(LayoutLimits::default())
        .context("acquire exclusive writer ownership")?;
    let producer_status = ProducerStatusPublisher::start(
        &config.out_dir,
        std::process::id(),
        unix_now_us()?,
        retention_status(config.retention).context("map retention status")?,
    )
    .context("publish collector startup status")?;

    // The journal lives next to finished segments so windows survive restarts.
    let (journal, recovered) = prepare_collector_storage(
        &writer_owner,
        LayoutLimits::default(),
        config.journal_max_bytes,
    )?;
    if let Some(dest) = recovered {
        announce(&format!("wrote {} reason=recovered", dest.display()));
    }
    Ok((writer_owner, producer_status, journal))
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    let (writer_owner, mut producer_status, mut journal) = start_up(&config)?;

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
                    heartbeat_best_effort(&mut producer_status);
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
            match close_open_segment(&mut journal, &writer_owner, &mut segment, "age") {
                Ok(dest) => {
                    sched.mark_segment_opened();
                    announce(&format!("wrote {} reason=age", dest.display()));
                    written_this_tick.push(dest);
                }
                Err(err) => log_event(
                    LogLevel::Error,
                    "segment_write_failure",
                    &[field("reason", "age"), field("error", format!("{err:#}"))],
                ),
            }
            stop_if_persistence_unhealthy(&journal, &segment)?;
        }
        if due.is_empty() {
            run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
            heartbeat_best_effort(&mut producer_status);
            continue;
        }
        written_this_tick.extend(run_collection_cycle(
            &mut journal,
            &writer_owner,
            &config,
            &due,
            &mut segment,
            &mut sched,
        ));
        stop_if_persistence_unhealthy(&journal, &segment)?;
        run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
        heartbeat_best_effort(&mut producer_status);
    }
    if let Err(err) = unix_now_us().and_then(|at_us| {
        producer_status
            .stop(at_us)
            .context("publish collector terminal status")
    }) {
        // A missing terminal marker degrades the status file to a stale
        // heartbeat, which consumers already treat as an unhealthy producer.
        log_event(
            LogLevel::Warn,
            "producer_status_stop_failure",
            &[field("error", format!("{err:#}"))],
        );
    }
    Ok(())
}

/// One collection cycle: read the due sources, append a window, and write when
/// the segment is full. Failures log and leave the daemon running.
fn run_collection_cycle(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    due: &DueSet,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
) -> Vec<PathBuf> {
    let ts = match unix_now_us() {
        Ok(ts) => ts,
        Err(err) => {
            log_event(
                LogLevel::Error,
                "collection_failed",
                &[field("reason", "clock"), field("error", format!("{err:#}"))],
            );
            return Vec::new();
        }
    };
    let fs = ProcFs::from_env();
    let in_container = detect_container(&fs);
    let mut interner = Interner::new(kronika_format::DictLimits::default());
    let mut buffers = SectionBuffers::new();

    let instance = match collect_due_instance(due) {
        Ok(instance) => instance,
        Err(err) => {
            log_event(
                LogLevel::Error,
                "collection_failed",
                &[
                    field("collection", "instance_metadata"),
                    field("error", format!("{err:#}")),
                ],
            );
            None
        }
    };

    let os = collect_os_sources(
        &fs,
        &mut interner,
        OsScope::Host.as_u8(),
        ts,
        in_container,
        due,
    );

    let buffered = (|| -> Result<()> {
        if let Some(facts) = instance.as_ref() {
            push_instance_metadata(&mut buffers, &mut interner, facts, in_container, ts)?;
        }
        push_os_sources(&mut buffers, &os)
    })();
    if let Err(err) = buffered {
        log_event(
            LogLevel::Error,
            "window_buffer_failure",
            &[field("error", format!("{err:#}"))],
        );
        return Vec::new();
    }
    if buffers.is_empty() {
        return Vec::new();
    }

    let flushed = match encode_window(buffers, &interner) {
        Ok(flushed) => flushed,
        Err(err) => {
            log_event(
                LogLevel::Error,
                "window_encode_failure",
                &[field("error", format!("{err:#}"))],
            );
            return Vec::new();
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
        &interner,
    ) {
        Ok(finished) => {
            let mut dests = Vec::with_capacity(finished.len());
            for (dest, reason) in finished {
                sched.mark_segment_opened();
                announce(&format!("wrote {} reason={reason}", dest.display()));
                dests.push(dest);
            }
            dests
        }
        Err(err) => {
            log_event(
                LogLevel::Error,
                "window_append_failure",
                &[field("error", format!("{err:#}"))],
            );
            Vec::new()
        }
    }
}

/// Auxiliary status publication never gates collection: a stale status file is
/// itself the designed "producer unhealthy" signal.
fn heartbeat_best_effort(producer_status: &mut ProducerStatusPublisher) {
    if let Err(err) = unix_now_us().and_then(|at_us| {
        producer_status
            .heartbeat(at_us)
            .context("publish collector heartbeat")
    }) {
        log_event(
            LogLevel::Warn,
            "producer_status_heartbeat_failure",
            &[field("error", format!("{err:#}"))],
        );
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

fn stop_if_persistence_unhealthy(journal: &Journal, segment: &SegmentState) -> Result<()> {
    if segment.requires_restart() {
        anyhow::bail!(
            "a segment was published but active.wal was not reset; stop before appending and recover on restart"
        );
    }
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
