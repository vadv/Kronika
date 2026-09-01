//! Storage-neutral identities and immutable segment access.

use std::fmt::Debug;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::SegmentId;

use crate::{CatalogSummary, StoreError, StoreObject, StoreWarning};

/// The product-visible kind of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ResourceKind {
    /// One immutable, self-contained ZMS segment.
    FinishedSegment,
}

/// Stable product identity of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceIdentity {
    segment_id: SegmentId,
    kind: ResourceKind,
}

impl ResourceIdentity {
    /// Identity of one immutable finished segment.
    #[must_use]
    pub const fn finished(segment_id: SegmentId) -> Self {
        Self {
            segment_id,
            kind: ResourceKind::FinishedSegment,
        }
    }

    /// Explicit segment identity supplied by the catalog source.
    #[must_use]
    pub const fn segment_id(self) -> SegmentId {
        self.segment_id
    }

    /// Product-visible resource kind.
    #[must_use]
    pub const fn kind(self) -> ResourceKind {
        self.kind
    }
}

/// One resource returned by catalog discovery.
///
/// `R` is an adapter-owned open token. Product code uses the identity and
/// catalog summary and passes the complete value back to its source to open
/// bytes; it does not interpret the token.
#[derive(Debug, Clone)]
pub struct SegmentResource<R> {
    identity: ResourceIdentity,
    captured_bytes: u64,
    summary: Arc<CatalogSummary>,
    handle: R,
}

impl<R> SegmentResource<R> {
    pub(crate) const fn new(
        identity: ResourceIdentity,
        captured_bytes: u64,
        summary: Arc<CatalogSummary>,
        handle: R,
    ) -> Self {
        Self {
            identity,
            captured_bytes,
            summary,
            handle,
        }
    }

    /// Stable product identity and kind.
    #[must_use]
    pub const fn identity(&self) -> ResourceIdentity {
        self.identity
    }

    /// Complete immutable object length in bytes.
    #[must_use]
    pub const fn captured_bytes(&self) -> u64 {
        self.captured_bytes
    }

    /// Compact validated catalog metadata.
    #[must_use]
    pub fn summary(&self) -> &CatalogSummary {
        &self.summary
    }

    pub(crate) const fn handle(&self) -> &R {
        &self.handle
    }
}

/// Product-visible subject of a non-fatal resource notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceWarningSubject {
    /// One immutable finished segment.
    FinishedSegment(ResourceIdentity),
    /// A mutable journal was unavailable to the catalog pass.
    ActiveJournal,
    /// An entry outside the source's supported catalog layout was ignored.
    ForeignEntry,
}

/// Storage-neutral, payload-free notice from resource discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceWarning {
    subject: ResourceWarningSubject,
    code: &'static str,
}

impl ResourceWarning {
    /// Build a notice from a neutral subject and stable low-cardinality code.
    #[must_use]
    pub const fn new(subject: ResourceWarningSubject, code: &'static str) -> Self {
        Self { subject, code }
    }

    pub(crate) const fn from_store(warning: StoreWarning) -> Self {
        let subject = match warning.affected {
            StoreObject::Segment(address) => {
                ResourceWarningSubject::FinishedSegment(ResourceIdentity::finished(address.id))
            }
            StoreObject::ActiveJournal => ResourceWarningSubject::ActiveJournal,
            StoreObject::Foreign(_) => ResourceWarningSubject::ForeignEntry,
        };
        Self::new(subject, warning.reason.code())
    }

    /// Product-visible object class affected by the notice.
    #[must_use]
    pub const fn subject(self) -> ResourceWarningSubject {
        self.subject
    }

    /// Stable low-cardinality notice code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.code
    }
}

/// Immutable resources and non-fatal discovery diagnostics from one catalog pass.
#[derive(Debug)]
pub struct ResourceListing<R> {
    /// Immutable resources in source-defined stable order.
    pub resources: Vec<SegmentResource<R>>,
    /// Objects excluded by catalog discovery.
    pub warnings: Vec<ResourceWarning>,
}

/// Discovers immutable segment identities and compact catalogs.
///
/// Discovery is deliberately separate from opening section bytes. A future
/// remote adapter can prepare an object range or local cache before returning
/// its synchronous [`ReadAt`] implementation.
pub trait ResourceCatalog {
    /// Adapter-owned token carried by a discovered resource.
    type Resource: Clone + Debug + Send + Sync + 'static;

    /// List immutable resources without retaining their complete bytes.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a bounded catalog pass cannot complete.
    fn resources(&self) -> Result<ResourceListing<Self::Resource>, StoreError>;
}

/// Opens positional bytes for one immutable resource returned by a catalog.
pub trait ImmutableSegmentSource: ResourceCatalog {
    /// Synchronous byte reader handed to the existing decoder.
    type Bytes: ReadAt + Send + Sync + 'static;

    /// Open exactly one listed immutable object.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the identity is foreign, stale, or unreadable.
    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, StoreError>;

    /// Confirm that opened bytes still name the object captured by discovery.
    ///
    /// Call this after decoding the catalog and before publishing a product
    /// view. Sources with versioned immutable bytes may only need to confirm
    /// provenance; POSIX sources also recheck the opened file identity.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the opened object no longer matches the
    /// listed resource.
    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), StoreError>;
}

#[cfg(test)]
#[path = "resource/tests.rs"]
mod tests;
