//! Transport-neutral cancellation and deadline checkpoints for product reads.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

/// One immutable execution boundary shared by HTTP and MCP adapters.
#[derive(Clone)]
pub(crate) struct Execution {
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    deadline: Instant,
}

impl fmt::Debug for Execution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Execution")
            .field("cancelled", &"[callback]")
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl Execution {
    /// Build a boundary from one caller-disconnect signal and absolute deadline.
    pub(crate) fn new(
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
        deadline: Instant,
    ) -> Self {
        Self {
            cancelled: Arc::new(cancelled),
            deadline,
        }
    }

    /// Stop bounded work promptly when the caller leaves or time expires.
    pub(crate) fn checkpoint(&self) -> Result<(), ExecutionStop> {
        if (self.cancelled)() {
            return Err(ExecutionStop::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ExecutionStop::DeadlineExceeded);
        }
        Ok(())
    }
}

/// Stable reason a product read stopped before completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionStop {
    Cancelled,
    DeadlineExceeded,
}

impl fmt::Display for ExecutionStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "execution was cancelled",
            Self::DeadlineExceeded => "execution deadline elapsed",
        })
    }
}

impl std::error::Error for ExecutionStop {}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Execution, ExecutionStop};

    #[test]
    fn cancellation_precedes_deadline_and_both_are_stable() {
        let past = || {
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("one second before now is representable")
        };
        let cancelled = Execution::new(|| true, past());
        assert_eq!(cancelled.checkpoint(), Err(ExecutionStop::Cancelled));

        let expired = Execution::new(|| false, past());
        assert_eq!(expired.checkpoint(), Err(ExecutionStop::DeadlineExceeded));
    }
}
