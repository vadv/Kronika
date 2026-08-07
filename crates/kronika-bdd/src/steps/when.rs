//! Driving the run: letting it collect, damaging its journal, restarting it.

use crate::BddWorld;
use crate::collector::Run;
use anyhow::{Context as _, Result};
use cucumber::when;
use std::time::Duration;

#[when(regex = r"^it runs for (\d+) seconds$")]
fn run_for(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let run = world.run.as_mut().context("a collector was started")?;
    run.run_for_and_stop(Duration::from_secs(seconds))
}

#[when(regex = r"^the last (\d+) bytes of the journal are cut off$")]
fn cut_the_journal(world: &mut BddWorld, bytes: u64) -> Result<()> {
    let run = world.run.take().context("a collector was started")?;
    let journal = run.out_dir().join("active.wal");
    let len = std::fs::metadata(&journal)
        .with_context(|| format!("stat {}", journal.display()))?
        .len();
    anyhow::ensure!(
        len > bytes,
        "the journal holds {len} bytes, too few to cut {bytes}"
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&journal)?
        .set_len(len - bytes)?;
    world.prepared_root = Some(run.into_root());
    Ok(())
}

#[when(regex = r"^it starts again over the same data root and runs for (\d+) seconds$")]
fn restart_over_the_same_root(world: &mut BddWorld, seconds: u64) -> Result<()> {
    let root = world
        .prepared_root
        .take()
        .context("a data root was prepared")?;
    let mut run = Run::spawn(root, &world.env)?;
    run.run_for_and_stop(Duration::from_secs(seconds))?;
    world.run = Some(run);
    Ok(())
}
