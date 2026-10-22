//! The clock the engine schedules against, supplied by the host.

use std::future::Future;
use std::time::Duration;

/// A monotonic clock and sleep. Native hosts use tokio's, the browser uses
/// `setTimeout`, and tests use a manual clock.
pub trait Timer {
    /// Milliseconds since an arbitrary fixed origin. Never goes backwards.
    fn now(&self) -> u64;

    /// Completes once `duration` has passed. Dropping it cancels the sleep.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()>;
}
