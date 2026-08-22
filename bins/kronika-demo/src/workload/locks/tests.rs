use super::periodic_chain_keys;

#[test]
fn all_chains_run_in_periodic_rounds() {
    assert_eq!(periodic_chain_keys(1).collect::<Vec<_>>(), [0]);
    assert_eq!(periodic_chain_keys(2).collect::<Vec<_>>(), [0, 1]);
}
