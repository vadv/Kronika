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
