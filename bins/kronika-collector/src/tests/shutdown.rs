use std::future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::complete_or_shutdown;

struct PendingWork(Arc<AtomicBool>);

impl Drop for PendingWork {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn shutdown_drops_in_progress_collection() {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = PendingWork(Arc::clone(&dropped));
    let work = async move {
        let _guard = guard;
        future::pending::<()>().await;
    };

    assert!(
        complete_or_shutdown(work, future::ready(()))
            .await
            .is_none()
    );
    assert!(dropped.load(Ordering::Relaxed));
}
