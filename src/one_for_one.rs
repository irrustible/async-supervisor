//! A Supervisor where processes are restarted independently of each other.
use crate::*;
use futures_lite::stream::StreamExt;
use simple_rate_limit::{RateLimit, RateLimiter};

/// A boxed function which is required to spawn a child using the provided Device.
pub type Start = Box<dyn Fn(Device)>;

/// A one-for-one Supervisor
pub struct OneForOne {
    restart_rate: RateLimit,
    specs: Vec<Spec>,
    states: Vec<Option<Running>>,
}

impl OneForOne {
    pub fn new(restart_rate: RateLimit) -> OneForOne {
        OneForOne { restart_rate, specs: Vec::new(), states: Vec::new() }
    }
    pub fn add(&mut self, spec: Spec) {
        self.specs.push(spec);
    }
    pub async fn supervise(
        mut self,
        mut device: Device,
    ) -> Result<(), Crash<SupervisionError>> {
        self.start_up(&mut device);
        self.watch(device).await?;
        Ok(())
    }

    fn start_up(&mut self, device: &mut Device) {
        for index in 0..self.specs.len() {
            let id = self.start_link(device, index);
            self.states.push(Some(Running { id }));
        }
    }

    async fn watch(
        mut self,
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
                    match self.disconnected(device, id, result, &mut limiter) {
                        Ok(d) => { device = d; }
                        Err(crash) => { return Err(crash); }
                    }
                }
            }
        }
        Ok(()) // Not found
    }

    fn disconnected(
        &mut self,
        device: Device,
        id: DeviceID,
        result: Option<Fault>,
        limiter: &mut RateLimiter
    ) -> Result<Device, Crash<SupervisionError>> {
        for index in 0..self.states.len() {
            let state = &self.states[index];
            if let Some(running) = state {
                if running.id == id {
                    return self.handle_restart(device, index, result, limiter);
                }
            }
        }
        Ok(device)
    }

    fn handle_restart(
        &mut self,
        device: Device,
        index: usize,
        result: Option<Fault>,
        limiter: &mut RateLimiter
    ) -> Result<Device, Crash<SupervisionError>> {
        self.states[index].take().unwrap();
        match self.specs[index].restart {
            Restart::Never => Ok(device),
            Restart::Always => self.restart(device, index, limiter),
            Restart::Failed => {
                if result.is_some() { self.restart(device, index, limiter) }
                else { Ok(device) }
            }
        }

    }

    fn start_link(
        &mut self,
        device: &mut Device,
        index: usize
    ) -> DeviceID {
        let d = Device::new();
        d.link(&device, LinkMode::Peer);
        let id = d.device_id();
        (self.specs[index].start)(d);
        id
    }

    fn restart(
        &mut self,
        mut device: Device,
        index: usize,
        limiter: &mut RateLimiter
    ) -> Result<Device, Crash<SupervisionError>> {
        if limiter.check() {
            let id = self.start_link(&mut device, index);
            self.states[index] = Some(Running { id });
            Ok(device)
        } else {
            device.disconnect(None);
            Err(Crash::Error(SupervisionError::Throttled))
        }
    }
}

