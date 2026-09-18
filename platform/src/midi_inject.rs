//! Injected MIDI: a way to drive the whole input path with no hardware.
//!
//! Everything downstream of `MidiInput::receive` — the port gate, the learn
//! layer, the surface decoder, the LED writer — is ordinary app code that
//! can be exercised, and until now none of it could be, because exercising
//! it needed a controller physically present. A console's controller path is
//! full of state machines that only misbehave in a set, which is the worst
//! possible place to find out.
//!
//! WHAT THIS PROVES AND WHAT IT DOES NOT. Injected bytes enter at
//! `MidiInput::receive`, so everything the app does with a message is really
//! being done. Everything BELOW that seam — enumeration, the OS device
//! handles, the per-backend receive closures, real hot-plug — sits under the
//! injection point and is proved by nothing here. A test that says "the
//! decoder handled a note" is honest; one that says "the device works" would
//! not be.
//!
//! Inert until armed, so a shipped build that never arms it pays one relaxed
//! atomic load per drain and nothing else. Arming is the bridge's doing, and
//! the bridge only exists under `--remote`.
//!
//! The outgoing ring is the other half: LED writes are otherwise invisible
//! without a controller to look at, so what the app SENDS is recorded here
//! for a test to read back.

use crate::makepad_live_id::LiveId;
use crate::midi::{MidiData, MidiPortDesc, MidiPortId, MidiPortType};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Nothing is queued and nothing is recorded until this is set. Checked with
/// a relaxed load on the drain path, which runs on the app's own pump.
static ARMED: AtomicBool = AtomicBool::new(false);

/// The most messages either queue holds.
///
/// A cap rather than an unbounded queue because the producer is an HTTP
/// route and the consumer is a 20 Hz pump: a test that pushes faster than
/// the app drains should lose the newest and say so, not grow without limit
/// inside a live audio process.
pub const MAX_QUEUED: usize = 4096;

struct Queues {
    /// Messages waiting to be handed to the app as if a device sent them.
    incoming: VecDeque<(MidiPortId, MidiData)>,
    /// What the app has sent, oldest first.
    outgoing: VecDeque<(MidiPortId, MidiData)>,
    /// How many incoming messages were dropped because the queue was full.
    dropped: u64,
}

static QUEUES: Mutex<Queues> = Mutex::new(Queues {
    incoming: VecDeque::new(),
    outgoing: VecDeque::new(),
    dropped: 0,
});

/// Ports the bridge has declared, waiting for the app to adopt them.
///
/// A declaration is not a device. It is a claim that the app should behave
/// as though these ports exist, and the app then does its OWN matching --
/// which on this console means picking a surface dialect by the port's
/// NAME. That matching is exactly the part worth driving, so the name is
/// the caller's to choose and the id is derived from it.
static DECLARED: Mutex<Option<Vec<MidiPortDesc>>> = Mutex::new(None);

pub fn arm() {
    ARMED.store(true, Ordering::Relaxed);
}

/// The id a named port answers to. Derived from the name, so a caller that
/// declared "Wide Surface" can inject to it by that name without having to
/// carry a number around.
pub fn port_id_for(name: &str) -> MidiPortId {
    MidiPortId(LiveId::from_str(name))
}

/// Declare a set of ports for the app to adopt. Each name becomes an input,
/// an output, or both; the descs are handed back so the caller learns the
/// ids without having to derive them itself.
pub fn declare_ports(names: &[(String, bool, bool)]) -> Vec<MidiPortDesc> {
    arm();
    let mut descs = Vec::new();
    for (name, input, output) in names {
        if *input {
            descs.push(MidiPortDesc {
                name: name.clone(),
                port_id: port_id_for(name),
                port_type: MidiPortType::Input,
            });
        }
        if *output {
            descs.push(MidiPortDesc {
                name: name.clone(),
                port_id: port_id_for(name),
                port_type: MidiPortType::Output,
            });
        }
    }
    if let Ok(mut slot) = DECLARED.lock() {
        // The LAST declaration wins rather than accumulating: a port set is
        // a whole picture of what is plugged in, exactly as the OS hands
        // one over, so two declarations are two pictures and not a sum.
        *slot = Some(descs.clone());
    }
    descs
}

/// The port set the app has yet to adopt, if there is one. Called from the
/// app's own pump, which is the only place that holds what adopting needs.
pub fn take_declared_ports() -> Option<Vec<MidiPortDesc>> {
    if !is_armed() {
        return None;
    }
    DECLARED.lock().ok()?.take()
}

pub fn is_armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Queue a message to be delivered as if a device had sent it.
///
/// Returns false when the queue is full, so a caller is told rather than
/// having its message silently vanish.
pub fn push_incoming(port: MidiPortId, data: MidiData) -> bool {
    arm();
    let Ok(mut q) = QUEUES.lock() else { return false };
    if q.incoming.len() >= MAX_QUEUED {
        q.dropped += 1;
        return false;
    }
    q.incoming.push_back((port, data));
    true
}

/// The next injected message, if any. Called from `MidiInput::receive`
/// BEFORE the real device is drained, so an injected message and a real one
/// cannot be reordered against each other within a drain.
pub fn take_incoming() -> Option<(MidiPortId, MidiData)> {
    if !is_armed() {
        return None;
    }
    QUEUES.lock().ok()?.incoming.pop_front()
}

/// Record something the app sent. Only while armed, and only ever read back
/// by a test.
pub fn record_outgoing(port: MidiPortId, data: MidiData) {
    if !is_armed() {
        return;
    }
    let Ok(mut q) = QUEUES.lock() else { return };
    if q.outgoing.len() >= MAX_QUEUED {
        // The OLDEST goes, not the newest: a test reading the tail of a
        // long LED burst wants what happened last.
        q.outgoing.pop_front();
    }
    q.outgoing.push_back((port, data));
}

/// Take everything the app has sent since this was last called.
pub fn drain_outgoing() -> Vec<(MidiPortId, MidiData)> {
    let Ok(mut q) = QUEUES.lock() else { return Vec::new() };
    q.outgoing.drain(..).collect()
}

/// How many injected messages have been dropped for want of room.
pub fn dropped() -> u64 {
    QUEUES.lock().map(|q| q.dropped).unwrap_or(0)
}

/// Forget everything queued and recorded. Between tests, so one cannot read
/// the tail of another's traffic.
pub fn reset() {
    if let Ok(mut slot) = DECLARED.lock() {
        *slot = None;
    }
    let Ok(mut q) = QUEUES.lock() else { return };
    q.incoming.clear();
    q.outgoing.clear();
    q.dropped = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One registry, one process, so the tests take turns. Without this
    /// they pass alone and fail together, which is the worst way for a test
    /// to be wrong.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let held = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        held
    }

    fn port(n: u64) -> MidiPortId {
        MidiPortId(crate::makepad_live_id::LiveId(n))
    }

    fn msg(a: u8, b: u8, c: u8) -> MidiData {
        MidiData { data: [a, b, c] }
    }

    #[test]
    fn nothing_is_delivered_until_something_arms_it() {
        let _held = guard();
        ARMED.store(false, Ordering::Relaxed);
        assert_eq!(take_incoming(), None, "a shipped build drains nothing");
        // Pushing arms it, because a caller that queued a message plainly
        // wants it delivered.
        push_incoming(port(1), msg(0x90, 0x40, 0x7f));
        assert!(is_armed());
        assert_eq!(take_incoming(), Some((port(1), msg(0x90, 0x40, 0x7f))));
        reset();
    }

    #[test]
    fn messages_come_back_in_the_order_they_went_in() {
        let _held = guard();
        arm();
        for note in 0x40..0x44 {
            push_incoming(port(1), msg(0x90, note, 0x7f));
        }
        let got: Vec<u8> =
            std::iter::from_fn(take_incoming).map(|(_, data)| data.data[1]).collect();
        assert_eq!(got, vec![0x40, 0x41, 0x42, 0x43]);
        reset();
    }

    #[test]
    fn a_full_queue_refuses_and_counts_rather_than_growing() {
        let _held = guard();
        arm();
        for _ in 0..MAX_QUEUED {
            assert!(push_incoming(port(1), msg(0xb0, 0x0e, 64)));
        }
        assert!(!push_incoming(port(1), msg(0xb0, 0x0e, 64)), "it must say no");
        assert_eq!(dropped(), 1, "and say how often");
        reset();
    }

    /// The name is the address: a caller that declared a port by name can
    /// inject to it by that name, with no id bookkeeping of its own.
    #[test]
    fn a_declared_port_answers_to_the_name_it_was_declared_under() {
        let _held = guard();
        let descs = declare_ports(&[("Wide Surface".to_string(), true, true)]);
        assert_eq!(descs.len(), 2, "one name, both directions");
        assert!(descs[0].port_type.is_input());
        assert!(descs[1].port_type.is_output());
        assert_eq!(descs[0].port_id, port_id_for("Wide Surface"));
        assert_eq!(descs[0].port_id, descs[1].port_id, "one port, two ends");
        reset();
    }

    /// A port set is a whole picture of what is plugged in. Declaring twice
    /// must therefore not leave the app adopting the FIRST picture, which is
    /// how a hot-unplug would be silently lost.
    #[test]
    fn a_second_declaration_replaces_the_first_rather_than_adding_to_it() {
        let _held = guard();
        declare_ports(&[("One".to_string(), true, false)]);
        declare_ports(&[("Two".to_string(), true, false)]);
        let taken = take_declared_ports().expect("a set is waiting");
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].name, "Two");
        assert!(take_declared_ports().is_none(), "adopting takes it");
        reset();
    }

    #[test]
    fn the_outgoing_ring_keeps_the_end_of_a_long_burst() {
        let _held = guard();
        arm();
        // More than it can hold: a test reading back an LED burst wants the
        // last of it, which is the state the surface was left in.
        for note in 0..(MAX_QUEUED + 8) {
            record_outgoing(port(2), msg(0x90, (note % 128) as u8, 0));
        }
        let out = drain_outgoing();
        assert_eq!(out.len(), MAX_QUEUED, "bounded");
        assert_eq!(
            out[out.len() - 1].1.data[1],
            ((MAX_QUEUED + 7) % 128) as u8,
            "and it is the TAIL that survived, not the head",
        );
        assert!(drain_outgoing().is_empty(), "draining takes them");
        reset();
    }
}
