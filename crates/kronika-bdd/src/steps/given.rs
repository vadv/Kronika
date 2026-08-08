//! Building the world a scenario runs against: settings, fixtures, data roots.

use super::table_rows;
use crate::BddWorld;
use crate::collector::{Run, copy_tree};
use crate::services::{PgBouncer, Postgres};
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::given;
use kronika_format::JOURNAL_MAGIC;
use std::path::PathBuf;

/// Where the fixture trees live. The BDD image sets this; a checkout run picks
/// them up beside the features.
fn fixtures_dir() -> PathBuf {
    std::env::var("KRONIKA_FIXTURES").map_or_else(|_unset| PathBuf::from("fixtures"), PathBuf::from)
}

#[given("a collector with these settings")]
fn collector_with_settings(world: &mut BddWorld, step: &Step) -> Result<()> {
    for row in table_rows(step, &["variable", "value"])? {
        let [key, value] = row.as_slice() else {
            anyhow::bail!("a settings row needs a variable and a value, got {row:?}");
        };
        world.env.push((key.clone(), value.clone()));
    }
    let root = world
        .prepared_root
        .take()
        .map_or_else(|| tempfile::tempdir().context("create a data root"), Ok)?;
    world.run = Some(Run::spawn(root, &world.env)?);
    Ok(())
}

#[given(regex = r"^a procfs fixture named (\S+)$")]
fn procfs_fixture(world: &mut BddWorld, name: String) -> Result<()> {
    let source = fixtures_dir().join(&name);
    anyhow::ensure!(
        source.is_dir(),
        "no fixture named {name} under {}",
        fixtures_dir().display()
    );
    let fixture = tempfile::tempdir().context("create a fixture root")?;
    let root = fixture.path().join(&name);
    copy_tree(&source, &root)?;
    world.env.push((
        "KRONIKA_PROC_ROOT".to_owned(),
        root.to_string_lossy().into_owned(),
    ));
    world.fixture = Some(fixture);
    Ok(())
}

#[given(regex = r#"^a data root whose active\.wal is the journal magic followed by "(.+)"$"#)]
fn prepared_corrupt_journal(world: &mut BddWorld, trailer: String) -> Result<()> {
    let root = tempfile::tempdir().context("create a data root")?;
    let out_dir = root.path().join("segments");
    std::fs::create_dir_all(&out_dir).context("create the segments directory")?;
    let mut bytes = JOURNAL_MAGIC.to_vec();
    bytes.extend_from_slice(trailer.as_bytes());
    std::fs::write(out_dir.join("active.wal"), bytes).context("write the damaged journal")?;
    world.prepared_root = Some(root);
    Ok(())
}

#[given(regex = r"^a PostgreSQL writing its log as (\S+)$")]
fn postgres_writing_its_log(world: &mut BddWorld, destination: String) -> Result<()> {
    let postgres = Postgres::start(&destination)?;
    world.env.push((
        "KRONIKA_PG_LOG".to_owned(),
        postgres.log_path.to_string_lossy().into_owned(),
    ));
    world
        .env
        .push(("KRONIKA_PG_DSN".to_owned(), postgres.dsn.clone()));
    world.postgres = Some(postgres);
    Ok(())
}

#[given("a PgBouncer in front of it")]
fn pgbouncer_in_front(world: &mut BddWorld) -> Result<()> {
    let pgbouncer = PgBouncer::start()?;
    world.env.push((
        "KRONIKA_PGBOUNCER_LOG".to_owned(),
        pgbouncer.log_path.to_string_lossy().into_owned(),
    ));
    world.pgbouncer = Some(pgbouncer);
    Ok(())
}
