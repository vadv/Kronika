use super::{
    Action, bounded_row_id, next_action, ordinary_sql, session_application_name, session_rng,
};
use rand::Rng as _;

#[test]
fn common_dml_dominates_the_low_rolls() {
    assert_eq!(next_action(0), Action::Insert);
    assert_eq!(next_action(29), Action::Insert);
    assert_eq!(next_action(30), Action::Update);
    assert_eq!(next_action(54), Action::Update);
    assert_eq!(next_action(55), Action::Select);
    assert_eq!(next_action(89), Action::Select);
}

#[test]
fn steady_sessions_never_emit_showcase_events() {
    for roll in 0..100 {
        assert!(
            matches!(
                next_action(roll),
                Action::Insert | Action::Update | Action::Select | Action::Delete
            ),
            "roll {roll} escaped the steady DML set"
        );
    }
}

#[test]
fn the_roll_wraps_at_100() {
    assert_eq!(next_action(100), next_action(0));
    assert_eq!(next_action(199), next_action(99));
}

#[test]
fn each_session_has_a_repeatable_independent_sequence() {
    let draw = |session| {
        let mut random = session_rng(session);
        (random.r#gen::<u64>(), random.r#gen::<u64>())
    };
    assert_eq!(draw(3), draw(3));
    assert_ne!(draw(3), draw(4));
}

#[test]
fn sessions_have_roles_an_operator_can_recognize() {
    assert_eq!(session_application_name(0), "checkout-api");
    assert_eq!(session_application_name(1), "catalog-api");
    assert_eq!(session_application_name(2), "payments-worker");
    assert_eq!(session_application_name(3), "fulfillment-worker");
    assert_eq!(session_application_name(4), "checkout-api");
}

#[test]
fn steady_updates_touch_one_key_instead_of_rewriting_the_whole_table() {
    assert_eq!(
        ordinary_sql("shop.orders", Action::Update, 12_345),
        Some("update shop.orders set id = id where id = 12345".to_owned())
    );
    assert!(
        !ordinary_sql("shop.orders", Action::Update, 12_345)
            .unwrap()
            .contains("where id is not null")
    );
}

#[test]
fn generated_row_ids_stay_inside_a_fixed_reusable_keyspace() {
    assert_eq!(bounded_row_id(0), 1);
    assert_eq!(bounded_row_id(9_999), 10_000);
    assert_eq!(bounded_row_id(10_000), 1);
    assert!((1..=10_000).contains(&bounded_row_id(u64::MAX)));
}
