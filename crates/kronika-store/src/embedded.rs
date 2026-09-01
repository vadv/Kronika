//! One immutable ZMS segment retained in shared owned bytes.

use std::io;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::{LayoutLimits, SegmentId};

use crate::local::segment::{FinishedValidation, read_zms_summary};
use crate::{
    CatalogSummary, ImmutableSegmentSource, ResourceCatalog, ResourceIdentity, ResourceListing,
    SegmentResource, StoreError,
};

/// Shared owned bytes used by an embedded finished segment.
#[derive(Clone)]
pub struct SharedSegmentBytes(Arc<[u8]>);

impl SharedSegmentBytes {
    /// Complete retained object length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the retained object is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Allocation address, used by copy-accounting tests.
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

impl std::fmt::Debug for SharedSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSegmentBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

impl ReadAt for SharedSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.0.as_ref().read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.0.len() as u64)
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
    /// Validate one complete ZMS object under the supplied segment identity.
    ///
    /// The ZMS format does not contain its segment ID, so the caller must
    /// provide it. Construction retains the caller's shared allocation and
    /// does not copy the complete segment.
    ///
    /// # Errors
    ///
    /// Returns a format, checksum, or bounded-metadata error for invalid bytes.
    pub fn new(segment_id: SegmentId, bytes: Arc<[u8]>) -> Result<Self, StoreError> {
        let summary = read_zms_summary(
            &bytes.as_ref(),
            0,
            LayoutLimits::default().max_metadata_bytes,
            FinishedValidation::Complete,
        )?;
        Ok(Self {
            identity: ResourceIdentity::finished(segment_id),
            bytes: SharedSegmentBytes(bytes),
            summary: Arc::new(summary),
            source_id: Arc::new(()),
        })
    }

    /// Bytes retained for the complete embedded segment allocation.
    #[must_use]
    pub fn retained_segment_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Allocation address retained from the caller.
    #[must_use]
    pub fn retained_segment_ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }
}

impl ResourceCatalog for EmbeddedSource {
    type Resource = EmbeddedResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, StoreError> {
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
    ) -> Result<Self::Bytes, StoreError> {
        if resource.identity() != self.identity
            || !Arc::ptr_eq(&resource.handle().0, &self.source_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "embedded resource belongs to another source",
            )
            .into());
        }
        Ok(self.bytes.clone())
    }
}

#[cfg(test)]
#[path = "embedded/tests.rs"]
mod tests;
