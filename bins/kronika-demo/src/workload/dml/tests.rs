use super::{Action, next_action, session_rng};
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
fn rare_actions_sit_at_the_top_of_the_range() {
    assert_eq!(next_action(90), Action::Delete);
    assert_eq!(next_action(95), Action::Delete);
    assert_eq!(next_action(96), Action::SlowQuery);
    assert_eq!(next_action(97), Action::BadStatement);
    assert_eq!(next_action(98), Action::BadStatement);
    assert_eq!(next_action(99), Action::BadDatabase);
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
