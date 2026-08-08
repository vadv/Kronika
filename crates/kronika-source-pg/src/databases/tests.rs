use super::{Database, refresh};
use crate::{MAX_AGE, Pool};
use std::collections::BTreeMap;

fn primary() -> Pool {
    Pool::new("host=/nonexistent dbname=postgres", MAX_AGE).expect("the DSN parses")
}

fn found(names: &[&str]) -> Vec<Database> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| Database {
            oid: u32::try_from(index).expect("a small index") + 16_400,
            name: (*name).to_owned(),
        })
        .collect()
}

#[test]
fn every_database_gets_a_connection_of_its_own() {
    let mut pools = BTreeMap::new();
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    assert_eq!(
        pools.keys().collect::<Vec<_>>(),
        ["appdb", "payments"].iter().collect::<Vec<_>>()
    );
}

#[test]
fn a_database_that_was_dropped_loses_its_connection() {
    let mut pools = BTreeMap::new();
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    refresh(&mut pools, &found(&["appdb"]), &primary());
    assert_eq!(pools.keys().collect::<Vec<_>>(), vec!["appdb"]);
}

#[test]
fn a_database_that_stayed_keeps_the_connection_it_had() {
    let mut pools = BTreeMap::new();
    let kept = Pool::new("host=/nonexistent dbname=sentinel", MAX_AGE).expect("the DSN parses");
    pools.insert("appdb".to_owned(), kept);
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    let survivor = format!("{:?}", pools.get("appdb").expect("appdb still has a pool"));
    assert!(
        survivor.contains("sentinel"),
        "the surviving pool was replaced: {survivor}"
    );
}
