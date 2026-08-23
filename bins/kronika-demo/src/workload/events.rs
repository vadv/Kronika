use super::dml::Action;
use super::{WorkloadConfig, connect_as, dml, naming, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(crate) const fn episode_actions() -> [Action; 3] {
    [Action::SlowQuery, Action::BadStatement, Action::BadDatabase]
}

pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let table = naming::table_name(0, 1);
    let client = match connect_as(&config.dsn, "catalog-api").await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: event workload could not connect: {error:#}");
            return;
        }
    };
    wait_for_stop(stop, Duration::from_secs(140)).await;
    while !stop.load(Ordering::Relaxed) {
        for action in episode_actions() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if let Err(error) = dml::perform(&client, config, &table, action, 0).await {
                eprintln!("kronika-demo: event action {action:?} failed: {error:#}");
            }
        }
        wait_for_stop(stop, Duration::from_secs(config.event_round_interval_s)).await;
    }
}

#[cfg(test)]
mod tests;
