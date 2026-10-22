//! Which widget drew each instance range of a draw, recorded only while a frame capture (or a
//! profiler's split frame) asks for it: [`Cx::arm_draw_owners`]. Nothing of this runs otherwise --
//! the widget draw wrappers and the draw-call lookup test one `Option`.
//!
//! While armed, the widget draw wrapper keeps `current` = the uid of the innermost widget drawing,
//! and every draw-call lookup (new or appended) notes (list, item, instance offset, current): the
//! instances the caller then adds belong to that widget, up to the next note of the same item. The
//! frame builder turns the notes of each emitted draw into (draw index in its packet, first
//! instance, count, uid) rows, kept per packet serial. The HAL's `draw_owners` query reads the
//! rows (with each uid's widget path) and disarms. A side table only: no instance data, no draw
//! state changes, batching is untouched.

use crate::cx::Cx;
use std::collections::{HashMap, VecDeque};

/// Packets kept for a reader that has not asked yet.
const KEEP_PACKETS: usize = 64;
/// Frames a recording stays on without a reader before it turns itself off.
const MAX_FRAMES: u32 = 8;

#[derive(Default)]
pub struct DrawOwners {
    /// uid of the widget drawing now (0: none)
    pub current: u64,
    /// (list, item) -> (instance offset in floats where a widget's instances start, uid)
    notes: HashMap<(usize, usize), Vec<(usize, u64)>>,
    /// the packet being built: serial and its rows [draw index, first instance, count, uid]
    packet: Option<(u64, u32, Vec<[u64; 4]>)>,
    /// finished packets: serial -> rows
    pub packets: VecDeque<(u64, Vec<[u64; 4]>)>,
    frames: u32,
}

impl DrawOwners {
    /// A draw call of `(list, item)` was looked up with `offset` floats of instances in it: the
    /// instances added from here belong to the current widget. A re-recording of the item (an
    /// offset at or below an earlier note) replaces the later notes.
    pub fn note(&mut self, list: usize, item: usize, offset: usize) {
        let v = self.notes.entry((list, item)).or_default();
        v.retain(|(o, _)| *o < offset);
        if v.last().is_none_or(|(_, uid)| *uid != self.current) {
            v.push((offset, self.current));
        }
    }

    pub(crate) fn begin_packet(&mut self, serial: u64) {
        self.packet = Some((serial, 0, Vec::new()));
    }

    /// The draw the builder emits next for `(list, item)`, of `count` instances of `slots` floats.
    pub(crate) fn draw(&mut self, list: usize, item: usize, slots: usize, count: usize) {
        let Some((_, index, rows)) = self.packet.as_mut() else { return };
        if let Some(notes) = self.notes.get(&(list, item)) {
            for (i, (offset, uid)) in notes.iter().enumerate() {
                let first = offset / slots.max(1);
                let end = notes.get(i + 1).map_or(count, |(o, _)| o / slots.max(1)).min(count);
                if end > first {
                    rows.push([*index as u64, first as u64, (end - first) as u64, *uid]);
                }
            }
        }
        *index += 1;
    }

    pub(crate) fn end_packet(&mut self) {
        if let Some((serial, _, rows)) = self.packet.take() {
            self.packets.push_back((serial, rows));
            if self.packets.len() > KEEP_PACKETS {
                self.packets.pop_front();
            }
        }
    }
}

impl Cx {
    /// Records which widget draws each instance range from now until the HAL's `draw_owners` query
    /// reads it (or a few frames pass unread). See [`DrawOwners`].
    pub fn arm_draw_owners(&mut self) {
        self.draw_owners = Some(Box::default());
    }

    /// One frame was built while armed: a recording nobody reads turns itself off.
    pub(crate) fn draw_owners_frame_done(&mut self) {
        if let Some(o) = self.draw_owners.as_mut() {
            o.frames += 1;
            if o.frames > MAX_FRAMES {
                self.draw_owners = None;
            }
        }
    }

    /// The `draw_owners` query's answer: `p <serial> <draw> <first> <count> <uid>` rows, then
    /// `w <uid>\t<path>\t<kind>` for every uid named. Disarms.
    pub(crate) fn draw_owners_answer(&mut self) -> String {
        let Some(o) = self.draw_owners.take() else { return "error: not armed".into() };
        let mut out = String::new();
        let mut uids: Vec<u64> = Vec::new();
        for (serial, rows) in &o.packets {
            for [draw, first, count, uid] in rows {
                out.push_str(&format!("p {serial} {draw} {first} {count} {uid}\n"));
                if *uid != 0 && !uids.contains(uid) {
                    uids.push(*uid);
                }
            }
        }
        if let Some(paths) = self.widget_uid_paths_callback {
            for (uid, path, kind) in paths(self, &uids) {
                out.push_str(&format!("w {uid}\t{path}\t{kind}\n"));
            }
        }
        out
    }
}
