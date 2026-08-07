//! Running the collector binary and reading back what it produced.

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for a clean exit after `SIGTERM`.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// The collector binary under test.
///
/// `KRONIKA_COLLECTOR_BIN` is set by the BDD image; a developer running the
/// suite from a checkout gets the debug build next to the test binary.
fn binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_COLLECTOR_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the BDD binary")?;
    let dir = here
        .parent()
        .context("the BDD binary has no parent directory")?;
    Ok(dir.join("kronika-collector"))
}

/// One collector run: its data root, its log, and the process while it lives.
#[derive(Debug)]
pub(crate) struct Run {
    root: tempfile::TempDir,
    log_path: PathBuf,
    child: Option<Child>,
}

impl Run {
    /// Spawn the collector over a fresh data root with `env` applied on top.
    pub(crate) fn spawn(env: &[(&str, String)]) -> Result<Self> {
        Self::adopt(
            tempfile::tempdir().context("create a data root for the scenario")?,
            env,
        )
    }

    /// Spawn the collector over a data root the scenario already populated.
    pub(crate) fn adopt(root: tempfile::TempDir, env: &[(&str, String)]) -> Result<Self> {
        let out_dir = root.path().join("segments");
        std::fs::create_dir_all(&out_dir).context("create the segments directory")?;
        let log_path = root.path().join("collector.log");
        let log = std::fs::File::create(&log_path).context("create collector.log")?;
        let mut command = Command::new(binary()?);
        command.env("KRONIKA_OUT_DIR", &out_dir);
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                log.try_clone().context("share collector.log for stdout")?,
            ))
            .stderr(Stdio::from(log))
            .spawn()
            .context("spawn the collector")?;
        Ok(Self {
            root,
            log_path,
            child: Some(child),
        })
    }

    /// The collector's data root.
    pub(crate) fn out_dir(&self) -> PathBuf {
        self.root.path().join("segments")
    }

    /// Let the collector run, then stop it the way an operator would.
    pub(crate) fn run_for_and_stop(&mut self, duration: Duration) -> Result<()> {
        std::thread::sleep(duration);
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        kill(
            Pid::from_raw(i32::try_from(child.id()).context("collector pid exceeds i32")?),
            Signal::SIGTERM,
        )
        .context("signal the collector to stop")?;
        let started = Instant::now();
        loop {
            if child.try_wait().context("reap the collector")?.is_some() {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < STOP_TIMEOUT,
                "the collector did not exit within {STOP_TIMEOUT:?} of SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        self.child = None;
        Ok(())
    }

    /// Everything the collector wrote to stdout and stderr.
    pub(crate) fn log(&self) -> Result<String> {
        std::fs::read_to_string(&self.log_path).context("read collector.log")
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // A scenario that failed mid-run leaves the collector alive; the
            // temporary data root goes with it either way.
            drop(child.kill());
            drop(child.wait());
        }
    }
}

/// Every file under `root`, relative to it.
pub(crate) fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_path_buf());
            }
        }
    }
    found
}
