//! Runs the collector for a bounded window and reports what it cost.
//!
//! This is the data source for the segment-size benchmarks: it publishes real
//! segments from the live host, then prints their size next to the collector's
//! peak resident set and CPU time. Every later stage of the project extends
//! the same run.
//!
//! Environment: `KRONIKA_DEMO_DIR` (default `demo-data`),
//! `KRONIKA_DEMO_DURATION_S` (default `60`), `KRONIKA_COLLECTOR_BIN` (default
//! `kronika-collector` next to this binary). Any `KRONIKA_*` variable the
//! collector reads passes through unchanged.

mod report;
mod sample;

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use report::Report;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often the demo reads the collector's footprint while it runs.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// How long to wait for a clean shutdown after `SIGTERM` before giving up.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
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

    let binary = collector_binary()?;
    let log_path = root.join("collector.log");
    let log = std::fs::File::create(&log_path).context("create collector.log")?;
    let mut child = Command::new(&binary)
        .env("KRONIKA_OUT_DIR", &segments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log.try_clone().context("share collector.log for stdout")?,
        ))
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    let pid = child.id();
    println!(
        "demo: collector pid {pid} for {duration_s}s, log {}",
        log_path.display()
    );

    let clock_ticks = rustix::param::clock_ticks_per_second();
    let started = Instant::now();
    let deadline = Duration::from_secs(duration_s);
    let mut peak_rss_bytes = 0_u64;
    let mut cpu_ticks = 0_u64;
    while started.elapsed() < deadline {
        std::thread::sleep(SAMPLE_INTERVAL);
        if let Some(status) = child.try_wait().context("poll the collector")? {
            anyhow::bail!(
                "the collector exited early with {status}; see {}",
                log_path.display()
            );
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
    };
    print!("{}", report.render());
    let report_path = root.join("report.json");
    std::fs::write(&report_path, report.to_json()).context("write report.json")?;
    println!("demo: report {}", report_path.display());
    Ok(())
}
