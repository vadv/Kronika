use super::{collector_log_to_stderr, resolve_storage_dir};
use std::path::{Path, PathBuf};

#[test]
fn collector_log_destination_is_explicit() {
    assert!(!collector_log_to_stderr(None).expect("default file"));
    assert!(!collector_log_to_stderr(Some("file")).expect("file"));
    assert!(collector_log_to_stderr(Some("stderr")).expect("stderr"));
    assert!(collector_log_to_stderr(Some("stdout")).is_err());
}

#[test]
fn storage_directory_is_explicit_or_belongs_to_the_demo_run() {
    let root = Path::new("/demo");
    assert_eq!(resolve_storage_dir(root, None), root.join("segments"));
    assert_eq!(
        resolve_storage_dir(root, Some(PathBuf::from("/storage"))),
        PathBuf::from("/storage")
    );
}

#[test]
fn healthcheck_does_not_probe_a_database_named_after_the_monitor_role() {
    let healthcheck = include_str!("../../../scripts/demo-healthcheck.sh");
    let readiness = healthcheck
        .lines()
        .find(|line| line.contains("pg_isready"))
        .expect("PostgreSQL readiness command");
    assert!(readiness.contains("--dbname=postgres"));
}
