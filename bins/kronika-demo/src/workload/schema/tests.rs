use super::{SHAPES, Table, table_ddl};

#[test]
fn table_ddl_names_the_table_by_schema_and_index() {
    let (name, ddl) = table_ddl(Table {
        schema: 3,
        index_in_schema: 407,
    });
    assert_eq!(name, "shop_3.archive_407");
    assert!(ddl.starts_with("create table if not exists shop_3.archive_407 ("));
}

#[test]
fn table_ddl_rotates_through_every_shape_in_order() {
    let shape_count = u32::try_from(SHAPES.len()).expect("small constant");
    for index_in_schema in 0..shape_count * 3 {
        let (_name, ddl) = table_ddl(Table {
            schema: 0,
            index_in_schema,
        });
        let expected_shape = SHAPES[index_in_schema as usize % SHAPES.len()];
        assert!(
            ddl.ends_with(expected_shape),
            "table {index_in_schema} did not get shape {expected_shape:?}: {ddl:?}"
        );
    }
}

#[test]
fn orders_have_the_columns_and_index_needed_by_the_plan_story() {
    let (name, ddl) = table_ddl(Table {
        schema: 0,
        index_in_schema: 0,
    });
    assert_eq!(name, "shop.orders");
    assert!(ddl.contains("customer_id bigint"));
    assert!(ddl.contains("status text"));
    assert!(ddl.contains("placed_at timestamptz"));
}
