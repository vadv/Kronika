use std::io;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::SegmentId;

use super::{
    CatalogSummary, ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceFailureKind,
    ResourceIdentity, ResourceKind, ResourceListing, ResourceWarning, ResourceWarningSubject,
    SegmentResource,
};

trait NeutralProbe {}

impl NeutralProbe for &'static str {}
impl NeutralProbe for usize {}
impl NeutralProbe for u32 {}
impl NeutralProbe for u64 {}
impl NeutralProbe for SegmentId {}
impl NeutralProbe for ResourceKind {}
impl NeutralProbe for ResourceIdentity {}
impl NeutralProbe for ResourceWarningSubject {}
impl NeutralProbe for ResourceWarning {}
impl NeutralProbe for ResourceFailureKind {}
impl NeutralProbe for kronika_format::DecodeError {}
impl NeutralProbe for kronika_format::CatalogLayoutError {}
impl NeutralProbe for CatalogSummary {}
impl<T: NeutralProbe> NeutralProbe for Arc<T> {}
impl<T: NeutralProbe> NeutralProbe for Vec<T> {}
impl<R: NeutralProbe> NeutralProbe for SegmentResource<R> {}

#[derive(Debug, Clone)]
struct ProbeResource;

impl NeutralProbe for ProbeResource {}

#[derive(Debug)]
struct ProbeBytes;

impl ReadAt for ProbeBytes {
    fn read_exact_at(&self, buf: &mut [u8], _offset: u64) -> io::Result<()> {
        if buf.is_empty() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "probe"))
        }
    }

    fn byte_len(&self) -> io::Result<u64> {
        Ok(0)
    }
}

struct ProbeSource;

impl ResourceCatalog for ProbeSource {
    type Resource = ProbeResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        Ok(ResourceListing {
            resources: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

impl ImmutableSegmentSource for ProbeSource {
    type Bytes = ProbeBytes;

    fn open_resource(
        &self,
        _resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        Ok(ProbeBytes)
    }

    fn validate_opened(
        &self,
        _resource: &SegmentResource<Self::Resource>,
        _bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        Ok(())
    }
}

fn assert_neutral<T: NeutralProbe>(value: &T) {
    let _ = value;
}

fn identity_shape(value: ResourceIdentity) {
    let ResourceIdentity { segment_id, kind } = value;
    assert_neutral(&segment_id);
    assert_neutral(&kind);
}

fn resource_shape(value: SegmentResource<ProbeResource>) {
    let SegmentResource {
        identity,
        captured_bytes,
        summary,
        handle,
    } = value;
    assert_neutral(&identity);
    assert_neutral(&captured_bytes);
    assert_neutral(&summary);
    assert_neutral(&handle);
}

fn listing_shape(value: ResourceListing<ProbeResource>) {
    let ResourceListing {
        resources,
        warnings,
    } = value;
    assert_neutral(&resources);
    assert_neutral(&warnings);
}

fn warning_shape(value: ResourceWarning) {
    let ResourceWarning { subject, code } = value;
    assert_neutral(&subject);
    assert_neutral(&code);
}

fn warning_subject_shape(value: ResourceWarningSubject) {
    match value {
        ResourceWarningSubject::FinishedSegment(identity) => assert_neutral(&identity),
    }
}

fn error_shape(value: &ResourceError) {
    match value {
        ResourceError::DuplicateIdentity(identity) => assert_neutral(identity),
        ResourceError::Unavailable(kind) => assert_neutral(kind),
        ResourceError::TooLarge { len, max } => {
            assert_neutral(len);
            assert_neutral(max);
        }
        ResourceError::ForeignResource
        | ResourceError::Changed
        | ResourceError::TooSmall
        | ResourceError::BadMagic
        | ResourceError::BadCatalogLength => {}
        ResourceError::TailIndex(error) | ResourceError::Catalog(error) => {
            assert_neutral(error);
        }
        ResourceError::UnsupportedFormat { version } => assert_neutral(version),
        ResourceError::SectionLayout(error) => assert_neutral(error),
        ResourceError::SectionChecksum { type_id } => assert_neutral(type_id),
        ResourceError::MetadataLimit { limit } => assert_neutral(limit),
    }
}

#[test]
fn public_resource_shapes_and_trait_contract_are_storage_neutral() {
    fn assert_source<S: ImmutableSegmentSource>() {}

    assert_source::<ProbeSource>();
    let _: fn(ResourceIdentity) = identity_shape;
    let _: fn(SegmentResource<ProbeResource>) = resource_shape;
    let _: fn(ResourceListing<ProbeResource>) = listing_shape;
    let _: fn(ResourceWarning) = warning_shape;
    let _: fn(ResourceWarningSubject) = warning_subject_shape;
    let _: fn(&ResourceError) = error_shape;
}

#[test]
fn core_storage_module_has_no_platform_or_transport_identifiers() {
    let source = include_str!("../resource.rs");
    for rejected in [
        "std::path",
        "std::fs::File",
        "LocalDir",
        "hyper::",
        "tokio::",
        "http::",
    ] {
        assert!(!source.contains(rejected), "core API contains {rejected}");
    }
}
