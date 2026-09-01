use std::sync::Arc;

use kronika_layout::SegmentId;

use super::EmbeddedSource;
use crate::{ImmutableSegmentSource as _, ResourceCatalog as _, ResourceKind, read_catalog};

const FIXTURE: &[u8] = include_bytes!("../../../kronika-format/tests/fixtures/minimal.zms");

fn source(id: i64) -> (Arc<[u8]>, EmbeddedSource) {
    let bytes: Arc<[u8]> = Arc::from(FIXTURE);
    let source = EmbeddedSource::new(SegmentId::new(id).expect("segment id"), Arc::clone(&bytes))
        .expect("valid embedded fixture");
    (bytes, source)
}

#[test]
fn embedded_catalog_keeps_explicit_identity_and_shared_bytes() {
    let (bytes, source) = source(42);
    let listing = source.resources().expect("embedded catalog");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.resources.len(), 1);
    let resource = &listing.resources[0];
    assert_eq!(resource.identity().segment_id().get(), 42);
    assert_eq!(resource.identity().kind(), ResourceKind::FinishedSegment);
    assert_eq!(resource.captured_bytes(), FIXTURE.len() as u64);

    let opened = source.open_resource(resource).expect("open embedded bytes");
    assert_eq!(
        opened.as_ptr(),
        bytes.as_ptr(),
        "open must not copy the ZMS"
    );
    assert_eq!(source.retained_segment_ptr(), bytes.as_ptr());
    assert_eq!(source.retained_segment_bytes(), FIXTURE.len());
    assert_eq!(
        read_catalog(&opened).expect("embedded catalog"),
        read_catalog(&FIXTURE).expect("fixture catalog")
    );
}

#[test]
fn embedded_resource_is_bound_to_the_source_that_listed_it() {
    let (_first_bytes, first) = source(42);
    let (_second_bytes, second) = source(42);
    let listing = first.resources().expect("first catalog");
    let error = second
        .open_resource(&listing.resources[0])
        .expect_err("foreign resource must fail");
    assert!(matches!(
        error,
        crate::StoreError::Io(ref source)
            if source.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[test]
fn embedded_constructor_rejects_invalid_zms_without_deriving_an_id() {
    let bytes: Arc<[u8]> = Arc::from(&b"not a ZMS"[..]);
    let error = EmbeddedSource::new(SegmentId::new(77).expect("segment id"), bytes)
        .expect_err("invalid bytes");
    assert!(matches!(
        error,
        crate::StoreError::TailIndex(_) | crate::StoreError::TooSmall
    ));
}

#[test]
fn storage_neutral_api_compiles_without_posix_types() {
    fn assert_api<S>(source: &S)
    where
        S: crate::ResourceCatalog + crate::ImmutableSegmentSource,
    {
        let listing = source.resources().expect("catalog");
        let resource = listing.resources.first().expect("resource");
        let bytes = source.open_resource(resource).expect("bytes");
        drop(read_catalog(&bytes).expect("catalog bytes"));
    }

    let (_bytes, embedded) = source(42);
    assert_api(&embedded);
}
