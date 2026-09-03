//! One immutable ZMS segment retained in owned bytes.

use std::io;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::{LayoutLimits, SegmentId};

use crate::zms::{FinishedValidation, read_zms_summary};
use crate::{
    CatalogSummary, ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceIdentity,
    ResourceListing, SegmentResource,
};

struct OwnedSegment(Vec<u8>);

/// Shared positional bytes used by an embedded finished segment.
#[derive(Clone)]
pub struct SharedSegmentBytes {
    bytes: Arc<OwnedSegment>,
    source_id: Arc<()>,
}

impl SharedSegmentBytes {
    fn len(&self) -> usize {
        self.bytes.0.len()
    }
}

impl std::fmt::Debug for SharedSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSegmentBytes")
            .field("len", &self.len())
            .finish()
    }
}

impl ReadAt for SharedSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.bytes.0.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

/// Opaque token binding a resource to the embedded source that listed it.
#[derive(Debug, Clone)]
pub struct EmbeddedResource(Arc<()>);

/// Catalog and bytes for exactly one explicit finished segment.
#[derive(Debug, Clone)]
pub struct EmbeddedSource {
    identity: ResourceIdentity,
    bytes: SharedSegmentBytes,
    summary: Arc<CatalogSummary>,
    source_id: Arc<()>,
}

impl EmbeddedSource {
    fn resource_is_canonical(&self, resource: &SegmentResource<EmbeddedResource>) -> bool {
        resource.identity() == self.identity
            && resource.captured_bytes() == self.bytes.len() as u64
            && resource.summary() == self.summary.as_ref()
            && Arc::ptr_eq(&resource.token().0, &self.source_id)
    }

    /// Validate exclusively owned ZMS bytes under the supplied segment identity.
    ///
    /// The `Vec` allocation, including its capacity, is moved behind shared
    /// ownership without copying its contents. Source, open, and clone handles
    /// retain that same allocation. Retained payload capacity is the original
    /// `Vec` capacity and excludes the `Arc` control block and compact catalog
    /// metadata. The ZMS format does not contain its segment ID, so the caller
    /// must provide it. `max_segment_bytes` bounds the logical length before
    /// format validation; a caller with a retained-memory budget must account
    /// the vector capacity separately.
    ///
    /// # Errors
    ///
    /// Returns a limit, format, checksum, or bounded-metadata error for invalid
    /// bytes.
    pub fn from_owned(
        segment_id: SegmentId,
        bytes: Vec<u8>,
        max_segment_bytes: u64,
    ) -> Result<Self, ResourceError> {
        let len = bytes.len() as u64;
        if len > max_segment_bytes {
            return Err(ResourceError::TooLarge {
                len,
                max: max_segment_bytes,
            });
        }
        let summary = read_zms_summary(
            &bytes,
            0,
            LayoutLimits::default().max_metadata_bytes,
            FinishedValidation::Complete,
        )
        .map_err(ResourceError::from_zms)?;
        let source_id = Arc::new(());
        Ok(Self {
            identity: ResourceIdentity::finished(segment_id),
            bytes: SharedSegmentBytes {
                bytes: Arc::new(OwnedSegment(bytes)),
                source_id: Arc::clone(&source_id),
            },
            summary: Arc::new(summary),
            source_id,
        })
    }
}

impl ResourceCatalog for EmbeddedSource {
    type Resource = EmbeddedResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        Ok(ResourceListing {
            resources: vec![SegmentResource::new(
                self.identity,
                self.bytes.len() as u64,
                Arc::clone(&self.summary),
                EmbeddedResource(Arc::clone(&self.source_id)),
            )],
            warnings: Vec::new(),
        })
    }
}

impl ImmutableSegmentSource for EmbeddedSource {
    type Bytes = SharedSegmentBytes;

    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        if !self.resource_is_canonical(resource) {
            return Err(ResourceError::ForeignResource);
        }
        Ok(self.bytes.clone())
    }

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        if !self.resource_is_canonical(resource) || !Arc::ptr_eq(&bytes.source_id, &self.source_id)
        {
            return Err(ResourceError::ForeignResource);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "embedded/tests.rs"]
mod tests;
