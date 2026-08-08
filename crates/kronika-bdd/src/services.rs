//! Starting the databases a log scenario needs.
//!
//! Nothing here writes a log file by hand. A scenario asserts what the
//! collector made of what `PostgreSQL` and `PgBouncer` actually wrote, so the
//! servers are the real ones, started in the image the suite runs in.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `PostgreSQL` keeps its cluster and its log.
const PG_DATA: &str = "/tmp/kronika-pgdata";
/// Where `PgBouncer` keeps its configuration and its log.
const PGB_DIR: &str = "/tmp/kronika-pgbouncer";
/// The unix user `PostgreSQL` runs as; it refuses to run as root.
const PG_USER: &str = "postgres";
const PG_PORT: &str = "5432";
const PGB_PORT: &str = "6432";

/// One started `PostgreSQL`.
#[derive(Debug)]
pub(crate) struct Postgres {
    /// The log file the collector is pointed at.
    pub(crate) log_path: PathBuf,
    /// How the collector reaches the server for `log_line_prefix`.
    pub(crate) dsn: String,
    /// The `psql` invocation a statement is run through.
    psql: String,
}

/// One started `PgBouncer`.
#[derive(Debug)]
pub(crate) struct PgBouncer {
    /// The log file the collector is pointed at.
    pub(crate) log_path: PathBuf,
    /// The `psql` invocation a client connects through, minus its database.
    psql: String,
}

/// Run a command, failing with what it printed.
fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("run {}", command.get_program().display()))?;
    if !output.status.success() {
        bail!(
            "{} {:?} failed: {}{}",
            command.get_program().display(),
            command.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `line` as the `postgres` user, which is what the server tools require.
fn as_postgres(line: &str) -> Result<String> {
    run(Command::new("su").args([PG_USER, "-c", line]))
}

/// The directory the `PostgreSQL` server binaries are installed in.
fn pg_bin() -> Result<String> {
    let listing = std::fs::read_dir("/usr/lib/postgresql").context("no PostgreSQL is installed")?;
    let mut versions: Vec<String> = listing
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    versions.sort();
    let version = versions
        .last()
        .context("/usr/lib/postgresql holds no version")?;
    Ok(format!("/usr/lib/postgresql/{version}/bin"))
}

impl Postgres {
    /// Initialize and start a cluster whose `log_destination` is `destination`.
    ///
    /// The log file's name follows the destination, which is what tells the
    /// collector how to read it.
    pub(crate) fn start(destination: &str) -> Result<Self> {
        let bin = pg_bin()?;
        stop_previous(&bin);
        reset_directory(Path::new(PG_DATA))?;
        as_postgres(&format!(
            "{bin}/initdb --pgdata={PG_DATA} --auth=trust --username={PG_USER} --encoding=UTF8 \
             --no-sync"
        ))?;
        let extension = match destination {
            "csvlog" => "csv",
            "jsonlog" => "json",
            _ => "log",
        };
        append_config(
            &Path::new(PG_DATA).join("postgresql.conf"),
            &format!(
                "listen_addresses = '127.0.0.1'\n\
                 port = {PG_PORT}\n\
                 logging_collector = on\n\
                 log_destination = '{destination}'\n\
                 log_directory = 'log'\n\
                 log_filename = 'postgresql.log'\n\
                 log_rotation_age = 0\n\
                 log_rotation_size = 0\n\
                 log_checkpoints = on\n\
                 log_lock_waits = on\n\
                 log_temp_files = 0\n\
                 log_min_duration_statement = 0\n\
                 fsync = off\n"
            ),
        )?;
        as_postgres(&format!(
            "{bin}/pg_ctl --pgdata={PG_DATA} --wait --timeout=60 --log={PG_DATA}/startup.log start"
        ))?;
        Ok(Self {
            log_path: PathBuf::from(PG_DATA)
                .join("log")
                .join(format!("postgresql.{extension}")),
            dsn: format!(
                "host=127.0.0.1 port={PG_PORT} user={PG_USER} dbname={PG_USER} connect_timeout=5"
            ),
            psql: format!(
                "{bin}/psql --host=127.0.0.1 --port={PG_PORT} --username={PG_USER} \
                 --dbname={PG_USER}"
            ),
        })
    }

    /// Run one statement, whether or not the server accepts it: a scenario
    /// asks for statements that fail on purpose.
    pub(crate) fn statement(&self, sql: &str) -> Result<()> {
        let quoted = sql.replace('\'', r"'\''");
        Command::new("su")
            .args([
                PG_USER,
                "-c",
                &format!("{} --command '{quoted}'", self.psql),
            ])
            .output()
            .context("run psql")?;
        Ok(())
    }
}

impl PgBouncer {
    /// Start a pooler in front of the running cluster.
    pub(crate) fn start() -> Result<Self> {
        let dir = Path::new(PGB_DIR);
        stop_previous_pooler(dir);
        reset_directory(dir)?;
        std::fs::write(dir.join("users.txt"), format!("\"{PG_USER}\" \"\"\n"))
            .context("write the auth file")?;
        std::fs::write(
            dir.join("pgbouncer.ini"),
            format!(
                "[databases]\n\
                 {PG_USER} = host=127.0.0.1 port={PG_PORT} dbname={PG_USER}\n\
                 [pgbouncer]\n\
                 listen_addr = 127.0.0.1\n\
                 listen_port = {PGB_PORT}\n\
                 auth_type = trust\n\
                 auth_file = {PGB_DIR}/users.txt\n\
                 logfile = {PGB_DIR}/pgbouncer.log\n\
                 pidfile = {PGB_DIR}/pgbouncer.pid\n\
                 pool_mode = transaction\n"
            ),
        )
        .context("write the pooler configuration")?;
        // PgBouncer refuses to run as root, so it runs as the same unix user
        // the server does.
        as_postgres(&format!("pgbouncer -d {PGB_DIR}/pgbouncer.ini"))?;
        Ok(Self {
            log_path: dir.join("pgbouncer.log"),
            psql: format!(
                "{}/psql --host=127.0.0.1 --port={PGB_PORT} --username={PG_USER}",
                pg_bin()?
            ),
        })
    }

    /// Connect through the pooler to `database`, whether or not it exists.
    pub(crate) fn connect_to(&self, database: &str) -> Result<()> {
        Command::new("su")
            .args([
                PG_USER,
                "-c",
                &format!("{} --dbname={database} --command 'select 1'", self.psql),
            ])
            .output()
            .context("run psql through the pooler")?;
        Ok(())
    }
}

/// Add the scenario's settings to a fresh cluster's configuration.
fn append_config(path: &Path, settings: &str) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(settings.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

/// A scenario that ran before this one left a server running; it goes before
/// its data directory does.
fn stop_previous(bin: &str) {
    let _stopped = as_postgres(&format!(
        "{bin}/pg_ctl --pgdata={PG_DATA} --mode=immediate --wait --timeout=30 stop"
    ));
}

fn stop_previous_pooler(dir: &Path) {
    let Ok(pid) = std::fs::read_to_string(dir.join("pgbouncer.pid")) else {
        return;
    };
    let _killed = run(Command::new("kill").args(["-TERM", pid.trim()]));
    std::thread::sleep(std::time::Duration::from_millis(500));
}

/// Start from an empty directory, so a second scenario is not a resumption of
/// the first.
fn reset_directory(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    }
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    run(Command::new("chown").args(["-R", PG_USER, &path.to_string_lossy()]))?;
    Ok(())
}
