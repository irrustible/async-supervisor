//! A Supervisor where any process being restarted causes all the
//! others to restart.
use crate::*;
use async_backplane::prelude::*;
use async_io::Timer;
use futures_lite::*;
use futures_many::Many;
use simple_rate_limit::{RateLimit, RateLimiter};

/// A one-for-one Supervisor
pub struct Supervisor {
    pub logic: RecoveryLogic,
    pub restart_rate: RateLimit,
    specs: Vec<Spec>,
    states: Vec<Option<Line>>,
}

impl Supervisor {

    pub fn new(logic: RecoveryLogic) -> Supervisor {
        Supervisor {
            logic,
            restart_rate: RateLimit::new(5, Duration::from_secs(5)).unwrap(),
            specs: Vec::new(),
            states: Vec::new(),
        }
    }

    pub fn set_restart_rate(mut self, restart_rate: RateLimit) -> Self {
        self.restart_rate = restart_rate;
        self
    }

    pub fn add_task(&mut self, spec: Spec) {
        self.specs.push(spec);
    }

    pub async fn supervise(
        mut self,
        mut device: Device,
    ) -> Result<(), Crash<SupervisionError>> {
        if let Err(crash) = self.start_up(&mut device, 0).await {
            self.shut_down(&mut device, 0).await;
            device.disconnect(None);
            Err(crash)
        } else {
            self.watch(device).await.map(|_| ())
        }
    }

    async fn start_up( 
        &mut self,
        device: &mut Device,
        start_index: usize
    ) -> Result<(), Crash<SupervisionError>> {
        for index in start_index..self.specs.len() {
            match self.start_link(device, index).await {
                Ok(line) => { self.states.push(line); }
                Err(error) => { return Err(error); }
            }
        }
        return Ok(());
    }

    async fn start_link(
        &mut self,
        device: &mut Device,
        index: usize
    ) -> Result<Option<Line>, Crash<SupervisionError>> {
        let d = Device::new();
        device.link(&d, LinkMode::Monitor);
        let line = d.line();
        self.specs[index].start.start(d).await
            .map_err(|e| Crash::Error(SupervisionError::StartupFailed(index, e)))
            .map(|s| match s {
                Started::Completed => None,
                Started::Running => Some(line),
            })
    }

    async fn watch(
        &mut self,
        device: Device
    ) -> Result<(), Crash<SupervisionError>> {
        let mut limiter = RateLimiter::new(self.restart_rate);
        let mut device = device;
        while let Some(message) = device.next().await {
            match message {
                Shutdown(id) => {
                    device.disconnect(None);
                    return Err(Crash::PowerOff(id));
                }
                Disconnected(id, result) => {
                    let ret = self.disconnected(&mut device, id, result, &mut limiter).await;
                    if let Err(crash) = ret { return Err(crash); }
                }
            }
        }
        Ok(()) // Not found
    }

    async fn disconnected(
        &mut self,
        device: &mut Device,
        id: DeviceID,
        result: Option<Fault>,
        limiter: &mut RateLimiter
    ) -> Result<(), Crash<SupervisionError>> {
        for index in 0..self.states.len() {
            let state = &self.states[index];
            if let Some(running) = state {
                if running.device_id() == id {
                    return self.handle_restart(device, index, result, limiter).await;
                }
            }
        }
        Ok(())
    }

    async fn handle_restart(
        &mut self,
        device: &mut Device,
        index: usize,
        result: Option<Fault>,
        limiter: &mut RateLimiter
    ) -> Result<(), Crash<SupervisionError>> {
        self.states[index].take().unwrap();
        match self.specs[index].restart {
            Restart::Never => Ok(()),
            Restart::Always => self.restart(device, index, limiter).await,
            Restart::Failed => {
                if result.is_some() { self.restart(device, index, limiter).await }
                else { Ok(()) }
            }
        }
    }

    async fn restart(
        &mut self,
        device: &mut Device,
        index: usize,
        limiter: &mut RateLimiter
    ) -> Result<(), Crash<SupervisionError>> {
        if limiter.check() {
            match self.logic {
                RecoveryLogic::Isolated => {
                    match self.start_link(device, index).await {
                        Ok(line) => {
                            self.states[index] = line;
                            Ok(())
                        }
                        Err(crash) => {
                            self.shut_down(device, 0).await;
                            Err(crash)
                        }
                    }
                }
                RecoveryLogic::CascadeNewer => {
                    self.shut_down(device, index + 1).await;
                    self.start_up(device, index).await
                }
                RecoveryLogic::CascadeAll => {
                    self.shut_down(device, 0).await;
                    self.start_up(device, 0).await
                }
            }
        } else {
            Err(Crash::Error(SupervisionError::Throttled))
        }
    }

    async fn shut_down(&mut self, device: &mut Device, start_index: usize) {
        let mut waiting: Vec<Option<DeviceID>> = Vec::new();
        let mut timers = Many::new();
        self.start_shut_down(device.device_id(), start_index, &mut waiting, &mut timers).await;
        let mut needed = waiting.len();
        while needed > 0 {
            match self.next_shutdown_message(device, &mut timers).await {
                ShuttingDown::Remove(id) => {
                    for x in waiting.iter_mut() {
                        if let Some(y) = x {
                            if *y == id {
                                *x = None;
                                needed -= 1;
                            }
                        }
                    }
                    // not found, ignore
                }
                ShuttingDown::DoneWaiting => { break; }
                ShuttingDown::Done => { return; }
            }
        }
        while needed > 0 {
            if let Some(message) = device.next().await {
                if let Message::Disconnected(id, _) = message {
                    for x in waiting.iter_mut() {
                        if let Some(y) = x {
                            if *y == id {
                                *x = None;
                                needed -= 1;
                            }
                        }
                    }
                } //ignore shutdown requests
            } else { return; } // Well the Device things there are no more left, what to do? unreachable?
        }
    }

    async fn start_shut_down(
        &mut self,
        my_id: DeviceID,
        start_index: usize,
        waiting: &mut Vec<Option<DeviceID>>,
        timers: &mut Many<future::Boxed<DeviceID>>
    ) {
        for (i, state) in self.states.drain(start_index..).enumerate().rev() {
            let index = i + start_index;
            if let Some(line) = state {
                let id = line.device_id();
                #[allow(unused_must_use)]
                match self.specs[index].shutdown {
                    Haste::Quickly => { line.send(Shutdown(my_id)); }
                    Haste::Gracefully(Grace::Forever) => {
                        waiting.push(Some(line.device_id()));
                        line.send(Shutdown(my_id));
                    }
                    Haste::Gracefully(Grace::Fixed(when)) => {
                        waiting.push(Some(line.device_id()));
                        timers.push(timer(when, id).boxed());
                        line.send(Shutdown(my_id));
                    }
                }
            }
        }
    }

    async fn next_shutdown_message(&mut self, device: &mut Device, timers: &mut Many<future::Boxed<DeviceID>>) -> ShuttingDown {
        loop {
            let ret = async { Ok(device.next().await) }.or(async { Err(timers.next().await) }).await;
            match ret {
                Ok(Some(Message::Disconnected(id,_))) => { return ShuttingDown::Remove(id); }
                Err(Some(id)) => { return ShuttingDown::Remove(id); }
                Ok(None) => { return ShuttingDown::Done; }
                Err(None) => { return ShuttingDown::DoneWaiting; }
                _ => (), // ignore, carry on
            }
        }
    }
}

enum ShuttingDown {
    Remove(DeviceID),
    DoneWaiting,
    Done,
}

async fn timer(when: Duration, id: DeviceID) -> DeviceID {
    Timer::new(when).await;
    id
}
