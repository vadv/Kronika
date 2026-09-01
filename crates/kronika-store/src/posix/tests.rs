use std::fs;
use std::io;

use kronika_format::ReadAt as _;
use kronika_layout::{SegmentAddress, SegmentId};

use super::PosixSource;
use crate::{
    ImmutableSegmentSource as _, ResourceCatalog as _, ResourceKind, ResourceWarningSubject,
    read_catalog,
};

const FIXTURE: &[u8] = include_bytes!("../../../kronika-format/tests/fixtures/minimal.zms");

fn write_segment(root: &std::path::Path, raw_id: i64) -> std::path::PathBuf {
    let address =
        SegmentAddress::new(SegmentId::new(raw_id).expect("segment id")).expect("segment address");
    let day = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    fs::create_dir_all(&day).expect("create day directory");
    let path = day.join(address.zms_name());
    fs::write(&path, FIXTURE).expect("write fixture");
    path
}

#[test]
fn posix_catalog_opens_opaque_positional_bytes_without_retaining_payload() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");

    let listing = source.resources().expect("POSIX catalog");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.resources.len(), 1);
    let resource = &listing.resources[0];
    assert_eq!(resource.identity().segment_id().get(), 1_000);
    assert_eq!(resource.identity().kind(), ResourceKind::FinishedSegment);
    assert_eq!(resource.captured_bytes(), FIXTURE.len() as u64);

    let bytes = source.open_resource(resource).expect("opened bytes");
    assert_eq!(source.retained_segment_bytes(), 0);
    assert_eq!(bytes.retained_segment_bytes(), 0);
    assert_eq!(bytes.byte_len().expect("byte length"), FIXTURE.len() as u64);
    assert_eq!(
        read_catalog(&bytes).expect("POSIX catalog bytes"),
        read_catalog(&FIXTURE).expect("fixture catalog")
    );
    source
        .validate_opened(resource, &bytes)
        .expect("stable opened bytes");
}

#[test]
fn posix_resource_and_bytes_are_bound_to_the_source_that_opened_them() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let first = PosixSource::open(dir.path()).expect("first source");
    let second = PosixSource::open(dir.path()).expect("second source");
    let listing = first.resources().expect("first catalog");
    let resource = &listing.resources[0];
    let bytes = first.open_resource(resource).expect("first bytes");

    let open_error = second
        .open_resource(resource)
        .expect_err("foreign resource must fail");
    assert!(matches!(
        open_error,
        crate::StoreError::Io(ref source) if source.kind() == io::ErrorKind::InvalidInput
    ));
    let validation_error = second
        .validate_opened(resource, &bytes)
        .expect_err("foreign bytes must fail");
    assert!(matches!(
        validation_error,
        crate::StoreError::Io(ref source) if source.kind() == io::ErrorKind::InvalidInput
    ));
}

#[test]
fn posix_post_catalog_validation_rechecks_the_opened_identity() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    let listing = source.resources().expect("POSIX catalog");
    let resource = &listing.resources[0];
    let bytes = source.open_resource(resource).expect("opened bytes");
    drop(read_catalog(&bytes).expect("catalog bytes"));

    let mut changed = FIXTURE.to_vec();
    changed[4] ^= 1;
    fs::write(path, changed).expect("change opened fixture");

    let error = source
        .validate_opened(resource, &bytes)
        .expect_err("changed identity must fail");
    assert!(matches!(
        error,
        crate::StoreError::Io(ref source) if source.kind() == io::ErrorKind::Interrupted
    ));
}

#[test]
fn posix_projects_layout_warnings_to_neutral_fields() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    fs::write(dir.path().join("unsupported-entry"), b"private").expect("foreign entry");
    let source = PosixSource::open(dir.path()).expect("POSIX source");

    let listing = source.resources().expect("POSIX catalog");
    let warning = listing.warnings.first().expect("resource warning");
    assert_eq!(warning.subject(), ResourceWarningSubject::ForeignEntry);
    assert!(warning.code().starts_with("foreign_entry_"));
}
