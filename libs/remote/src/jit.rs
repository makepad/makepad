//! Compile activity of a JIT app, read from its host's inspect commands. These commands run
//! independently of the app's event loop, including while it is waiting for a lazy compile.

use crate::link::{Client, Error};

/// Only call for a JIT endpoint. `jit` includes running builds and compile queues; `owner`
/// also covers compiler work between builds. An unrecognised reply is never taken as idle.
pub fn busy(client: &Client) -> Result<bool, Error> {
    let mut active = None;
    for line in client.cmd("jit ms=100")? {
        let line = line?;
        if line.starts_with("busy") {
            active = Some(true);
        } else if line.starts_with("idle · ") {
            active = Some(!line.split(" · ").any(|part| part == "queued: ahead 0, O2 0, O3 0"));
        } else if line.starts_with("{\"err\":") {
            return Err(Error::Io(std::io::Error::other(line)));
        }
    }
    match active {
        Some(true) => return Ok(true),
        Some(false) => {}
        None => return Err(Error::Io(std::io::Error::other("jit reply carries no compile state"))),
    }
    let mut active = None;
    for line in client.cmd("owner")? {
        let line = line?;
        if line.starts_with("{\"err\":") {
            return Err(Error::Io(std::io::Error::other(line)));
        }
        let Ok(value) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        let Some(serving) = value.get("serving").and_then(|v| v.as_str()) else { continue };
        let queued = value.get("queued").and_then(|v| v.get("total")).and_then(|v| v.as_u64())
            .ok_or_else(|| Error::Io(std::io::Error::other("owner reply carries no queue state")))?;
        active = Some(serving != "idle" || queued != 0);
    }
    active.ok_or_else(|| Error::Io(std::io::Error::other("owner reply carries no serving state")))
}

/// Used after each native response timeout, without resending the request. Compilation has
/// no wall-clock cap; after first observing idle, allow a full native timeout for the reply.
#[derive(Default)]
pub struct Wait {
    idle: bool,
}

impl Wait {
    pub fn keep_waiting(&mut self, client: &Client) -> Result<bool, Error> {
        if busy(client)? {
            self.idle = false;
            Ok(true)
        } else {
            Ok(!std::mem::replace(&mut self.idle, true))
        }
    }
}
