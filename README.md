# async-supervisor

<!-- [![License](https://img.shields.io/crates/l/async-supervisor.svg)](https://github.com/irrustible/async-supervisor/blob/main/LICENSE) -->
<!-- [![Package](https://img.shields.io/crates/v/async-supervisor.svg)](https://crates.io/crates/async-supervisor) -->
<!-- [![Documentation](https://docs.rs/async-supervisor/badge.svg)](https://docs.rs/async-supervisor) -->

Erlang-style Supervisors for
[async-backplane](https://github.com/irrustible/async-backplane).

## Status: alpha

It's built on battle tested principles, but the implementation is
brand new, not yet finished and currently without tests .

Currently unimplemented (contributions welcome!):

* Startup with `Haste::Quickly` - panics.
* Shutdown without `Haste::Quickly` - accepted but ignored.
* The entire test suite, lol.

## Guide

I wanted to bring erlang style reliability to rust, so I wrote
[async-backplane](https://github.com/irrustible/async-backplane), a
fabulous way of building reliable systems in the erlang style. But
it was only the building blocks, not the full package.

To build erlang style systems needs supervisors. These are my
backplane adaptations of the best erlang/elixir ones.

If you haven't read the
[async-backplane](https://github.com/irrustible/async-backplane)
README, you will want to do that before you continue!

### What is a Supervisor, anyway?

A `Supervisor` is a Future responsible for starting and managing tasks
(Device-holding Futures spawned on an executor). It starts up all of
its tasks and attempts to recover from the failure of one of them by
restarting it and potentially its peers, according to the provided
configuration. We control which tasks are restarted by selecting a
`RecoveryLogic`, reproduced below:

```rust
pub enum RecoveryLogic {
    /// No other tasks will be restarted.
    Isolated,
    /// Tasks started after this one will be restarted.
    CascadeNewer,
    /// All tasks will be restarted.
    CascadeAll,
}
```

We then choose a `RateLimit` for how often restarts are allowed to
happen before the `Supervisor` gives up trying to restart things and
disconnects itself. We create a supervisor thus:

```rust
use async_supervisor::{RateLimit, RecoveryLogic, Supervisor};

fn my_sup() -> Supervisor {
    // One task failing does not affect any others.
    Supervisor::new(RecoveryLogic::Isolated)
}
```

The supervisor defaults to a `restart_rate` of 5 restarts within 5
seconds. This means that on the sixth restart within 5 seconds, the
supervisor will abort trying to restart tasks and will
disconnect. This can be customed by providing a new `RateLimit` to
`Supervisor.set_restart_rate()`.

A supervisor with no tasks isn't much use, however. We describe tasks
by creating a `Spec`, a pairing of a boxed function to spawn it with
some configuration about how to manage it. We'll cover configuration
in a minute, but first let's explain that boxed function.

If we're going to support restarting tasks, we need to have some
concept of a lifecycle those tasks must obey. Ours is very simple - it
first performs startup work and then it runs. We separate things into
two phases because supervisors have the option to perform an orderly
startup, where we wait for each task to start up before going on to
start the next task.

In the event that during startup, one of the tasks fails to start, the
supervisor will shut down with a success status. Its supervisor will
then restart it only if it is configured to `Always` restart it.

Now come some rather wordy types that are actually quite simple:

```rust
pub type StartFn = Box<dyn Fn(Device) -> Starting>;
pub type Starting = Box<dyn Future<Output=Result<Started, Fault>> + Unpin>;
```

`StartFn` is a boxed function from `Device` to `Starting`. It's boxed
so we can start different tasks under the same `Supervisor`.
`Starting` is mostly wordy because we're specifying the `Future`'s
output. It's also boxed, for the same reason.

If we ignore the boxing for a moment, this would be a suitable start
function:

```rust
async fn start_fn(device: Device) -> Result<Started, Fault> { ... }
```

The reason is that `async fn` is just syntax sugar over a `Fn`
returning a `Future`. The type of this function would be this, if we
could write it this way:

```rust
Fn(Device) -> impl Future<Output=Result<Started, Fault>>
```

So ours is just the version of that with the added boxes. The future
that is returned should complete when the task has successfully
completed its startup work. It should return a `Started`:

```rust
pub enum Started {
    /// We've done our work and we don't need to keep running.
    Completed,
    /// We've started up successfully.
    Running,
}
```

Let's write a simple start fn that doesn't need to do anything to
start up:

```rust
use async_backplane::Device;
use async_supervisor::{StartFn, Starting};
use smol::Task; // A simple futures executor.
use futures_micro::ready;

fn start(device: Device) -> Starting {
    // Start the task.
    Task::spawn(async { // How you spawn in smol.
        // Go straight into managed mode, in this case
        // just completing successfully.
        device.manage(|| Ok(()))
    }).detach();
    // Return the future for the supervisor to wait on. `ready()`
    // just immediately succeeds with the provided value
    Box::new(ready(Started::Running)) // Not for very long, ha!
}

```

And here's one that has a startup phase:

```rust
use async_backplane::Device;
use async_supervisor::{StartFn, Starting};
use smol::Task;
use async_oneshot::oneshot; // A simple oneshot channel.

fn start(device: Device) -> Starting {
    // Create a channel for the device to signal us on.
    let (send, recv) = oneshot();
    Task::spawn(async {
        // ... startup work goes here ...
        // Announce we're all good.
        send.send(Ok(Started::Running)).unwrap();
        // Now go into managed mode. You should probably unwrap
        // the result it returns instead of ignoring it.
        device.manage(|| Ok(())).await;
    }).detach();
    Box::new(recv)
}
```

Now let's tie everything together - creating a supervisor with a
single task and running it:

```rust
use async_supervisor::{RateLimit, RecoveryLogic, Spec, Supervisor};
use smol::Task; // A simple Futures executor.
use async_oneshot::oneshot;

async fn my_sup(device: Device) {
    // This is the code from the last example.
    let limit = RateLimit::new(5, Duration::from_secs(5));
    let mut sup = Supervisor::new(RecoveryLogic::Isolated, limit);
    sup.add_task(Spec::new(Start::new(Box::new(start)))); // function from last example.
    sup.supervise().await; // you should check the result.
}
```

We didn't change any of the default options here, but we should cover
what they are. Firstly the `Start` object we create has the option to
set a grace period for startup other than the default (5 seconds) with
the `set_grace` method. Most of the options are on the `Spec` though:

```rust
pub struct Spec {
    pub start: Start,
    pub restart: Restart,
    pub shutdown: Haste,
}
```

`Restart` is a simple enum that tells the supervisor when to restart
this task:

```rust
/// When should a supervisor restart a child?
pub enum Restart {
    /// Do not restart: this is only supposed to run once.
    Never,
    /// We won't restart it if it succeeds.
    Failed,
    /// Restart even if it succeeds.
    Always,
}
```

`Haste` describes how much time we give a task to start up or shut
down.We can either wait for it for some (potentially infinite) grace
period or we can just assume it succeeded and carry on:

```rust
/// How should a task be restarted?
pub enum Haste {
    /// We will wait for it to end before we continue.
    Gracefully(Grace),
    /// We will assume it to have disconnected and continue our work.
    Quickly,
}

/// A period of time permitted for a startup/shutdown to occur.
pub enum Grace {
    /// A fixed period of time.
    Fixed(Duration),
    /// As long as needed. Mainly for supervisors. Be very careful!
    Forever,
}
```

You can set `restart` and `shutdown` with the `set_restart` and
`set_shutdown` methods on `Spec`.




TODO: describe interactions.

## Differences from Erlang Supervisors

Obviously, being built in rust, we already have to diverge somewhat
from Erlang. We rely on a rust adaptation of the erlang principles,
[async-backplane](https://github.com/irrustible/async-backplane),
which loosely resembles the basic erlang environment.

The obvious difference, therefore, is types. We have rearranged the
structure of things to feel more natural in rust. In particular we do
not distinguish between 'worker' and 'supervisor' processes - the user
simply configures appropriate grace periods for their tasks.

Because we don't control task spawning, we don't maintain the ability
to terminate a task. We therefore rely on spawned tasks to obey a
contract in order to guarantee we work correctly.

## Copyright and License

Copyright (c) 2020 James Laver, async-supervisor Contributors

This Source Code Form is subject to the terms of the Mozilla Public
License, v. 2.0. If a copy of the MPL was not distributed with this
file, You can obtain one at http://mozilla.org/MPL/2.0/.

