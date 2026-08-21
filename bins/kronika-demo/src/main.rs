//! Runs the collector for a bounded window and reports what it cost.
//!
//! This is the data source for the segment-size benchmarks: it publishes real
//! segments from the live host, then prints their size next to the collector's
//! peak resident set and CPU time. Every later stage of the project extends
//! the same run.
//!
//! The report names every section the run produced and how many rows of it
//! reached a segment, so pointing `KRONIKA_PG_LOG` or `KRONIKA_PGBOUNCER_LOG`
//! at a live log shows those events in the same summary.
//!
//! Environment: `KRONIKA_DEMO_DIR` (default `demo-data`),
//! `KRONIKA_DEMO_DURATION_S` (default `60`; `0` runs until `SIGTERM` or
//! `SIGINT` instead of an already-elapsed deadline), `KRONIKA_COLLECTOR_BIN`
//! (default `kronika-collector` next to this binary). Any `KRONIKA_*`
//! variable the collector reads passes through unchanged.
//!
//! Setting `KRONIKA_DEMO_WORKLOAD_DSN` also drives a `PostgreSQL` workload
//! (schemas, tables, steady DML, lock-wait chains) alongside the collector;
//! see `workload` for its configuration.
#![allow(
    clippy::multiple_crate_versions,
    reason = "the registry's arrow/parquet stack and the workload's rand/tokio-postgres \
              dependencies pull duplicate transitive versions outside our control"
)]

mod report;
mod sample;
mod sections;
mod shutdown;
mod workload;

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use report::Report;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use workload::{Workload, WorkloadConfig};

/// How often the demo reads the collector's footprint while it runs.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// How long to wait for a clean shutdown after `SIGTERM` before giving up.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn collector_log_to_stderr(raw: Option<&str>) -> Result<bool> {
    match raw {
        None | Some("file") => Ok(false),
        Some("stderr") => Ok(true),
        Some(value) => anyhow::bail!("KRONIKA_DEMO_COLLECTOR_LOG={value:?} is not file or stderr"),
    }
}

/// The collector binary: the operator's choice, else the one next to us.
fn collector_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_COLLECTOR_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the demo binary")?;
    let dir = here
        .parent()
        .context("the demo binary has no parent directory")?;
    Ok(dir.join("kronika-collector"))
}

fn spawn_collector(
    binary: &Path,
    segments: &Path,
    root: &Path,
) -> Result<(std::process::Child, String)> {
    let log_path = root.join("collector.log");
    let log_to_stderr =
        collector_log_to_stderr(std::env::var("KRONIKA_DEMO_COLLECTOR_LOG").ok().as_deref())?;
    let log_description = if log_to_stderr {
        "container stderr".to_owned()
    } else {
        log_path.display().to_string()
    };
    let mut command = Command::new(binary);
    command
        .env("KRONIKA_OUT_DIR", segments)
        .stdin(Stdio::null());
    if log_to_stderr {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        let log = std::fs::File::create(&log_path).context("create collector.log")?;
        command
            .stdout(Stdio::from(
                log.try_clone().context("share collector.log for stdout")?,
            ))
            .stderr(Stdio::from(log));
    }
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    Ok((child, log_description))
}

/// Total bytes and count of the `.zms` files under `root`.
fn measure_segments(root: &Path) -> Result<(usize, u64)> {
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry.file_type().context("stat a data-root entry")?;
            if file_type.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "zms") {
                count += 1;
                bytes += entry.metadata().context("stat a segment")?.len();
            }
        }
    }
    Ok((count, bytes))
}

fn main() -> Result<()> {
    let root = PathBuf::from(env_or("KRONIKA_DEMO_DIR", "demo-data"));
    let segments = root.join("segments");
    let duration_s: u64 = env_or("KRONIKA_DEMO_DURATION_S", "60")
        .parse()
        .context("KRONIKA_DEMO_DURATION_S is not a u64")?;
    std::fs::create_dir_all(&segments).context("create the demo data root")?;

    let stop = shutdown::watch().context("watch for shutdown signals")?;
    let workload = WorkloadConfig::from_env()
        .context("read the workload configuration")?
        .map(Workload::start)
        .transpose()
        .context("start the demo workload")?;

    let binary = collector_binary()?;
    let (mut child, log_description) = spawn_collector(&binary, &segments, &root)?;
    let pid = child.id();
    println!("demo: collector pid {pid} for {duration_s}s, log {log_description}");

    let clock_ticks = rustix::param::clock_ticks_per_second();
    let started = Instant::now();
    // `0` means "run until stopped" rather than an already-elapsed deadline.
    let deadline = (duration_s != 0).then(|| Duration::from_secs(duration_s));
    let mut peak_rss_bytes = 0_u64;
    let mut cpu_ticks = 0_u64;
    loop {
        if deadline.is_some_and(|deadline| started.elapsed() >= deadline) {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
        if let Some(status) = child.try_wait().context("poll the collector")? {
            anyhow::bail!("the collector exited early with {status}; see {log_description}");
        }
        if let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/status"))
            && let Some(rss) = sample::peak_rss_bytes(&text)
        {
            peak_rss_bytes = peak_rss_bytes.max(rss);
        }
        if let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && let Some(ticks) = sample::cpu_ticks(&text)
        {
            cpu_ticks = cpu_ticks.max(ticks);
        }
    }

    if let Some(workload) = workload {
        workload.stop();
    }

    // SIGTERM, not kill(): the collector writes its open segment on the way out
    // and the measured size would be short without it.
    kill(
        Pid::from_raw(i32::try_from(pid).context("collector pid exceeds i32")?),
        Signal::SIGTERM,
    )
    .context("signal the collector to stop")?;
    let stop_started = Instant::now();
    loop {
        if child.try_wait().context("reap the collector")?.is_some() {
            break;
        }
        anyhow::ensure!(
            stop_started.elapsed() < STOP_TIMEOUT,
            "the collector did not exit within {STOP_TIMEOUT:?} of SIGTERM"
        );
        std::thread::sleep(SAMPLE_INTERVAL);
    }

    let (count, bytes) = measure_segments(&segments)?;
    let journal_bytes = std::fs::metadata(segments.join("active.wal"))
        .map(|meta| meta.len())
        .unwrap_or_default();
    let report = Report {
        duration_s,
        segments: count,
        segment_bytes: bytes,
        journal_bytes,
        peak_rss_bytes,
        cpu_ms: cpu_ticks.saturating_mul(1_000) / clock_ticks.max(1),
        sections: sections::section_rows(&segments)?,
    };
    print!("{}", report.render());
    let report_path = root.join("report.json");
    std::fs::write(&report_path, report.to_json()).context("write report.json")?;
    println!("demo: report {}", report_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::collector_log_to_stderr;

    #[test]
    fn collector_log_destination_is_explicit() {
        assert!(!collector_log_to_stderr(None).expect("default file"));
        assert!(!collector_log_to_stderr(Some("file")).expect("file"));
        assert!(collector_log_to_stderr(Some("stderr")).expect("stderr"));
        assert!(collector_log_to_stderr(Some("stdout")).is_err());
    }
}
