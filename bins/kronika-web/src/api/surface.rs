//! Public product-surface projections shared by HTTP and MCP.

pub(crate) const LOCK_GRAPH_FIELDS: &[&str] = &["pid", "blocked_by", "datname", "lock_target"];

const ACTIVITY_DEFAULT_FIELDS: &[&str] = &[
    "pid",
    "datid",
    "datname",
    "usename",
    "application_name",
    "client_addr",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "query",
    "query_id",
    "backend_xid_age",
    "backend_xmin_age",
    "backend_start",
    "xact_start",
    "query_start",
    "state_change",
];

const LOCK_DEFAULT_FIELDS: &[&str] = &[
    "pid",
    "blocked_by",
    "datid",
    "datname",
    "usename",
    "application_name",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "query",
    "lock_locktype",
    "lock_mode",
    "lock_database",
    "lock_relation",
    "lock_relname",
    "lock_page",
    "lock_tuple",
    "lock_virtualxid",
    "lock_transactionid",
    "lock_classid",
    "lock_objid",
    "lock_objsubid",
    "lock_target",
    "waitstart",
];

pub(super) const fn default_fields(logical_name: &str) -> Option<&'static [&'static str]> {
    match logical_name.as_bytes() {
        b"pg_stat_activity" => Some(ACTIVITY_DEFAULT_FIELDS),
        b"pg_locks" => Some(LOCK_DEFAULT_FIELDS),
        _ => None,
    }
}

pub(crate) fn field_is_public(logical_name: &str, name: &str) -> bool {
    match logical_name {
        "pg_stat_activity" => name == "leader_pid" || ACTIVITY_DEFAULT_FIELDS.contains(&name),
        "pg_locks" => LOCK_DEFAULT_FIELDS.contains(&name),
        _ => false,
    }
}
