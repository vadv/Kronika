use super::admits;
use crate::config::Account;

fn account() -> Account {
    Account {
        user: "dba".to_owned(),
        password: "secret".to_owned(),
    }
}

const CORRECT: &str = "Basic ZGJhOnNlY3JldA==";

#[test]
fn only_the_required_account_is_admitted() {
    assert!(admits(&account(), Some(CORRECT)));
    assert!(admits(&account(), Some("basic ZGJhOnNlY3JldA==")));
    assert!(!admits(&account(), None));
    assert!(!admits(&account(), Some("Basic ZGJhOndyb25n")));
    assert!(!admits(&account(), Some("Bearer ZGJhOnNlY3JldA==")));
    assert!(!admits(&account(), Some("Basic not-base64!!")));
}
