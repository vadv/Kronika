//! Type `1_123_001`: exact Linux block partition-parent edges.

use crate::{Section, Ts};

/// One exact sysfs partition to parent-block-device relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_123_001,
    name = "os_block_topology",
    semantics = on_change,
    sort_key("major", "minor", "ts"),
    identity("major", "minor")
)]
pub struct OsBlockTopology {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Partition major number.
    #[column(l)]
    pub major: i32,
    /// Partition minor number.
    #[column(l)]
    pub minor: i32,
    /// Exact parent block-device major number.
    #[column(l)]
    pub parent_major: i32,
    /// Exact parent block-device minor number.
    #[column(l)]
    pub parent_minor: i32,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsBlockTopology;
    use crate::{Section, Ts, contract::lint};

    #[test]
    fn contract_is_an_exact_child_identity() {
        assert_eq!(lint(&[OsBlockTopology::CONTRACT]), Ok(()));
        assert_eq!(OsBlockTopology::CONTRACT.identity, ["major", "minor"]);
        crate::assert_roundtrips(&[OsBlockTopology {
            ts: Ts(10),
            major: 259,
            minor: 1,
            parent_major: 259,
            parent_minor: 0,
            scope: 0,
        }]);
    }
}
