use crate::*;
use async_io::Timer;

/// A boxed function that spawns a Future using the provided Device
/// and returns a [`Starting`] the supervisor can await to confirm the
/// process has started up.
pub type StartFn = Box<dyn Fn(Device) -> Starting>;

/// Type of the boxed Future returned by [`StartFn`].
pub type Starting = Box<dyn Future<Output = Result<Started, Fault>> + Unpin>;

pub enum StartError {
    /// The task failed on its own terms.
    Fault(Fault),
    /// The grace period was exceeded.
    Timeout,
}

/// Describes how a supervisor should start a task. The major
/// component is a boxed function ([`Init`]),
pub struct Start {
    pub fun: StartFn,
    /// How much of a hurry we are in to get onto starting the next task.
    pub haste: Haste,
}

impl Start {
    /// Creates a new [`Start`] from a [`Fn`] closure.
    pub fn new(fun: StartFn) -> Self {
        Start {
            fun,
            haste: Haste::Gracefully(Grace::Fixed(Duration::from_secs(5))),
        }
    }

    /// Sets the inner function to the provided value.
    pub fn set_fn(mut self, fun: StartFn) -> Self {
        self.fun = fun;
        self
    }

    /// Sets the grace period to the provided value.
    pub fn set_haste(mut self, haste: Haste) -> Self {
        self.haste = haste;
        self
    }

    /// Starts a process, giving it the appropriate grace period to start up
    pub async fn start(&self, device: Device) -> Result<Started, StartError> {
        match self.haste {
            Haste::Gracefully(Grace::Forever) => {
                (self.fun)(device).await.map_err(StartError::Fault)
            }
            Haste::Gracefully(Grace::Fixed(duration)) => {
                async { (self.fun)(device).await.map_err(StartError::Fault) }
                    .or(async {
                        Timer::new(duration).await;
                        Err(StartError::Timeout)
                    })
                    .await
            }
            Haste::Quickly => unimplemented!(),
        }
    }
}
