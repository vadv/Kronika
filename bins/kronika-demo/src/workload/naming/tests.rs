use super::{schema_name, table_name};

#[test]
fn schema_name_is_a_stable_tenant_prefix() {
    assert_eq!(schema_name(0), "tenant_0");
    assert_eq!(schema_name(12), "tenant_12");
}

#[test]
fn table_name_is_schema_qualified() {
    assert_eq!(table_name(0, 0), "tenant_0.t0");
    assert_eq!(table_name(3, 407), "tenant_3.t407");
}
