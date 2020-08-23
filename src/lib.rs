mod start;
pub use start::{Start, StartError, StartFn, Starting};

mod supervisor;
pub use supervisor::Supervisor;

// pub mod rest_for_one;
// pub mod one_for_one;

pub use simple_rate_limit::RateLimit;

use async_backplane::{Device, DeviceID, Fault};
use futures_lite::FutureExt;
use std::future::Future;
use std::time::Duration;

/// A logic for determining which other tasks to restart when one fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryLogic {
    /// No other tasks will be restarted.
    Isolated,
    /// Tasks started after this one will be restarted.
    CascadeNewer,
    /// All tasks will be restarted.
    CascadeAll,
}

/// A period of time permitted for a startup/shutdown to occur.
pub enum Grace {
    /// A fixed period of time.
    Fixed(Duration),
    /// As long as needed. Mainly for supervisors. Be very careful!
    Forever,
}

/// How patient should we be shutting down a task?
pub enum Haste {
    /// We will wait for it to end before we continue.
    Gracefully(Grace),
    /// We will assume it to have disconnected and continue our work.
    Quickly,
}

/// When a service completes its startup phase successfully. Notifies
/// the supervisor of whether it's finished or continuing to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Started {
    /// We've done our work and we don't need to keep running.
    Completed,
    /// We've started up successfully.
    Running,
}

/// When should a supervisor restart a child?
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restart {
    /// Do not restart: this is only supposed to run once.
    Never,
    /// We won't restart it if it succeeds.
    Failed,
    /// Restart even if it succeeds.
    Always,
}

/// A structure describing how the supervisor starts and manages a task.
pub struct Spec {
    pub start: Start,
    pub restart: Restart,
    pub shutdown: Haste,
}

impl Spec {
    /// Creates a new Spec with the provided [`Start`].
    pub fn new(start: Start) -> Self {
        Spec {
            start,
            restart: Restart::Always,
            shutdown: Haste::Gracefully(Grace::Fixed(Duration::from_secs(5))),
        }
    }

    /// Replaces [`start`] with a new [`Start`]
    pub fn set_start(mut self, start: Start) -> Self {
        self.start = start;
        self
    }

    /// Replaces [`restart`] with a new [`Restart`]
    pub fn set_restart(mut self, restart: Restart) -> Self {
        self.restart = restart;
        self
    }

    /// Replaces [`shutdown`] with a new [`Shutdown`]
    pub fn set_shutdown(mut self, shutdown: Haste) -> Self {
        self.shutdown = shutdown;
        self
    }
}

/// The Supervisor failed - why?
pub enum SupervisionError {
    /// We were asked to shutdown, presumably by our own supervisor.
    Shutdown(DeviceID),
    /// We didn't manage to initialise the spec with the given index.
    StartupFailed(usize, StartError),
    /// Exceeded its restart rate limit.
    Throttled,
}
