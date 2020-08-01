use async_backplane::prelude::*;
pub mod one_for_one;
use one_for_one::Start;

pub enum SupervisionError {
    /// The supervisor restarted too many times within its period
    Throttled,
}

/// When should a supervisor restart a child?
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Restart {
    /// Do not restart: this is only supposed to run once.
    Never,
    /// We won't restart it if it succeeds.
    Failed,
    /// Restart even if it succeeds.
    Always,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Kind {
    Managed,
    Supervisor,
}

pub struct Spec {
    pub start: Start,
    pub kind: Kind,
    pub restart: Restart,
}

impl Spec {
    pub fn new(start: Start, kind: Kind, restart: Restart) -> Spec {
        Spec { start, kind, restart }
    }
}

pub(crate) struct Running {
    id: DeviceID,
}


