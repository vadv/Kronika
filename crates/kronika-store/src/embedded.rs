//! One immutable ZMS segment retained in shared or static bytes.

use std::io;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::{LayoutLimits, SegmentId};

use crate::local::segment::{FinishedValidation, read_zms_summary};
use crate::{
    CatalogSummary, ImmutableSegmentSource, ResourceCatalog, ResourceIdentity, ResourceListing,
    SegmentResource, StoreError,
};

#[derive(Clone)]
enum EmbeddedBytes {
    Shared(Arc<[u8]>),
    Static(&'static [u8]),
}

impl EmbeddedBytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Shared(bytes) => bytes.as_ref(),
            Self::Static(bytes) => bytes,
        }
    }
}

/// Shared positional bytes used by an embedded finished segment.
#[derive(Clone)]
pub struct SharedSegmentBytes {
    bytes: EmbeddedBytes,
    source_id: Arc<()>,
}

impl SharedSegmentBytes {
    /// Complete retained object length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.as_slice().len()
    }

    /// Whether the retained object is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.as_slice().is_empty()
    }

    /// Allocation address, used by copy-accounting tests.
    #[must_use]
    pub fn as_ptr(&self) -> *const u8 {
        self.bytes.as_slice().as_ptr()
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
        self.bytes.as_slice().read_exact_at(buf, offset)
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
    /// Validate shared owned ZMS bytes under the supplied segment identity.
    ///
    /// The ZMS format does not contain its segment ID, so the caller must
    /// provide it. Construction retains the caller's shared allocation and
    /// does not copy the complete segment. `max_segment_bytes` is checked
    /// before format validation.
    ///
    /// # Errors
    ///
    /// Returns a limit, format, checksum, or bounded-metadata error for invalid
    /// bytes.
    pub fn from_shared(
        segment_id: SegmentId,
        bytes: Arc<[u8]>,
        max_segment_bytes: u64,
    ) -> Result<Self, StoreError> {
        Self::from_bytes(segment_id, EmbeddedBytes::Shared(bytes), max_segment_bytes)
    }

    /// Validate static ZMS bytes under the supplied segment identity.
    ///
    /// The ZMS format does not contain its segment ID, so the caller must
    /// provide it. Construction retains only the static slice and does not
    /// copy the complete segment. `max_segment_bytes` is checked before format
    /// validation.
    ///
    /// # Errors
    ///
    /// Returns a limit, format, checksum, or bounded-metadata error for invalid
    /// bytes.
    pub fn from_static(
        segment_id: SegmentId,
        bytes: &'static [u8],
        max_segment_bytes: u64,
    ) -> Result<Self, StoreError> {
        Self::from_bytes(segment_id, EmbeddedBytes::Static(bytes), max_segment_bytes)
    }

    fn from_bytes(
        segment_id: SegmentId,
        bytes: EmbeddedBytes,
        max_segment_bytes: u64,
    ) -> Result<Self, StoreError> {
        let len = bytes.as_slice().len() as u64;
        if len > max_segment_bytes {
            return Err(StoreError::ResourceTooLarge {
                len,
                max: max_segment_bytes,
            });
        }
        let summary = read_zms_summary(
            &bytes.as_slice(),
            0,
            LayoutLimits::default().max_metadata_bytes,
            FinishedValidation::Complete,
        )?;
        let source_id = Arc::new(());
        Ok(Self {
            identity: ResourceIdentity::finished(segment_id),
            bytes: SharedSegmentBytes {
                bytes,
                source_id: Arc::clone(&source_id),
            },
            summary: Arc::new(summary),
            source_id,
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

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), StoreError> {
        if resource.identity() != self.identity
            || !Arc::ptr_eq(&resource.handle().0, &self.source_id)
            || !Arc::ptr_eq(&bytes.source_id, &self.source_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "embedded resource belongs to another source",
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "embedded/tests.rs"]
mod tests;
