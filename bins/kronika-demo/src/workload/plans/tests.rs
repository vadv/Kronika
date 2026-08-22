use super::{checkout_query_sql, setup_sql, transition_sql};

#[test]
fn setup_seeds_a_selective_orders_workload_and_fast_index() {
    let sql = setup_sql(300_000);
    assert!(sql.contains("insert into shop.orders"));
    assert!(sql.contains("generate_series(1, 300000)"));
    assert!(sql.contains("customer_id"));
    assert!(sql.contains("checkout_orders_customer_placed_idx"));
    assert!(sql.contains("(customer_id, placed_at desc)"));
    assert!(sql.contains("analyze shop.orders"));
}

#[test]
fn every_checkout_query_is_bounded_and_keeps_one_normalized_shape() {
    assert_eq!(
        checkout_query_sql(4_242),
        "set statement_timeout = '3s'; select id, status, total_cents from shop.orders where customer_id = 4242 order by placed_at desc limit 50"
    );
    assert_eq!(
        checkout_query_sql(7),
        "set statement_timeout = '3s'; select id, status, total_cents from shop.orders where customer_id = 7 order by placed_at desc limit 50"
    );
}

#[test]
fn regression_removes_and_then_restores_the_exact_index() {
    let sql = transition_sql();
    assert_eq!(
        sql.drop_index,
        "set lock_timeout = '3s'; set statement_timeout = '10s'; drop index if exists shop.checkout_orders_customer_placed_idx"
    );
    assert!(sql.restore_index.contains("statement_timeout = '30s'"));
    assert!(
        sql.restore_index
            .contains("create index if not exists checkout_orders_customer_placed_idx")
    );
    assert!(
        sql.restore_index
            .contains("on shop.orders (customer_id, placed_at desc)")
    );
    assert!(sql.restore_index.ends_with("analyze shop.orders"));
}
