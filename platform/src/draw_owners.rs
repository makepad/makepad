//! Which widget (or labelled drawer) drew each instance range of a draw, recorded only while a
//! frame capture asks for it ([`Cx::arm_draw_owners`]) or a GPU profiler recording runs (armed by
//! the frame builder, [`Cx::draw_owners_follow_profiler`]). Nothing of this runs otherwise -- the
//! widget draw wrappers and the draw-call lookup test one `Option`, and the frame builder asks the
//! HAL's recording flag once per frame.
//!
//! An owner is a widget uid or a [`DrawLabel`]: any drawer (a 3D renderer's model lane, a
//! particle system) names the instances it adds with [`Cx::enter_draw_label`]. Labels are
//! interned once ([`Cx::draw_label`]), so naming a draw while recording is a field store.
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

/// A drawer's name for the instances it adds (see [`Cx::enter_draw_label`]); 0 is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DrawLabel(u32);

/// An owner key with this bit set is a [`DrawLabel`]'s index; without it a widget uid (widget
/// uids count up from 1 and never reach it).
const LABEL_BIT: u64 = 1 << 63;

/// The interned label names; lives on `Cx` whether or not anything records, so a drawer can
/// keep its labels across recordings.
#[derive(Default)]
pub struct DrawLabels {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

#[cfg(not(target_arch = "wasm32"))]
/// Packets kept for a reader that has not asked yet.
const KEEP_PACKETS: usize = 64;
#[cfg(not(target_arch = "wasm32"))]
/// Frames a recording stays on without a reader before it turns itself off.
const MAX_FRAMES: u32 = 8;

#[derive(Default)]
pub struct DrawOwners {
    /// uid of the widget drawing now (0: none)
    pub current: u64,
    /// (list, item) -> (instance offset in floats where a widget's instances start, uid)
    notes: HashMap<(usize, usize), Vec<(usize, u64)>>,
    #[cfg(not(target_arch = "wasm32"))]
    /// the packet being built: serial and its rows [draw index, first instance, count, uid]
    packet: Option<(u64, u32, Vec<[u64; 4]>)>,
    /// finished packets: serial -> rows
    pub packets: VecDeque<(u64, Vec<[u64; 4]>)>,
    #[cfg(not(target_arch = "wasm32"))]
    frames: u32,
    #[cfg(not(target_arch = "wasm32"))]
    /// a frame capture asked (packets are kept for its `draw_owners` query)
    capture: bool,
    #[cfg(not(target_arch = "wasm32"))]
    /// a GPU profiler recording runs: stays armed (no packets) until it stops
    prof: bool,
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn begin_packet(&mut self, serial: u64) {
        // only a capture reads the rows; a profiler recording names each draw as it is emitted
        if self.capture {
            self.packet = Some((serial, 0, Vec::new()));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// The owner of most instances of the draw of `(list, item)` with `count` instances of
    /// `slots` floats (0: none noted). A draw batched across owners is named by its largest.
    pub(crate) fn dominant(&self, list: usize, item: usize, slots: usize, count: usize) -> u64 {
        let Some(notes) = self.notes.get(&(list, item)) else { return 0 };
        let (mut best, mut most) = (0u64, 0usize);
        for (i, (offset, owner)) in notes.iter().enumerate() {
            let first = offset / slots.max(1);
            let end = notes.get(i + 1).map_or(count, |(o, _)| o / slots.max(1)).min(count);
            if end > first && end - first > most {
                (best, most) = (*owner, end - first);
            }
        }
        best
    }

    #[cfg(not(target_arch = "wasm32"))]
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

    #[cfg(not(target_arch = "wasm32"))]
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
        let _o = self.draw_owners.get_or_insert_with(Box::default);
        // the web build has no HAL frame builder to read a capture
        #[cfg(not(target_arch = "wasm32"))]
        {
            _o.capture = true;
            _o.frames = 0;
        }
    }

    /// Owners are being recorded (a capture or a profiler recording): a drawer names its draws
    /// ([`Self::enter_draw_label`]) only then.
    #[inline]
    pub fn draw_owners_armed(&self) -> bool {
        self.draw_owners.is_some()
    }

    /// The label named `name`, interned on first use (keep it: a lookup by name hashes it).
    pub fn draw_label(&mut self, name: &str) -> DrawLabel {
        let l = &mut self.draw_labels;
        if let Some(i) = l.index.get(name) {
            return DrawLabel(*i);
        }
        l.names.push(name.to_string());
        let i = l.names.len() as u32;
        l.index.insert(name.to_string(), i);
        DrawLabel(i)
    }

    /// Attributes the instances added from now on to `label` (while owners are recorded);
    /// returns the previous owner for [`Self::exit_draw_label`]. Free when nothing records.
    #[inline]
    pub fn enter_draw_label(&mut self, label: DrawLabel) -> u64 {
        match self.draw_owners.as_mut() {
            Some(o) if label.0 != 0 => std::mem::replace(&mut o.current, LABEL_BIT | label.0 as u64),
            Some(o) => o.current,
            None => 0,
        }
    }

    /// [`Self::enter_draw_label`] by name, for a fixed name (`"grass"`, `"sky"`): a hash lookup
    /// while owners are recorded, nothing otherwise.
    #[inline]
    pub fn enter_draw_name(&mut self, name: &str) -> u64 {
        if self.draw_owners.is_none() {
            return 0;
        }
        let label = self.draw_label(name);
        self.enter_draw_label(label)
    }

    /// Ends [`Self::enter_draw_label`]'s attribution: `previous` is its answer.
    #[inline]
    pub fn exit_draw_label(&mut self, previous: u64) {
        if let Some(o) = self.draw_owners.as_mut() {
            o.current = previous;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// The name an owner key reads as: a label's name, or a widget's path (`None`: unknown).
    pub(crate) fn draw_owner_name(&self, key: u64) -> Option<String> {
        if key & LABEL_BIT != 0 {
            return self.draw_labels.names.get(((key & !LABEL_BIT) as usize).wrapping_sub(1)).cloned();
        }
        let paths = self.widget_uid_paths_callback?;
        paths(self, &[key]).into_iter().next().map(|(_, path, kind)| if path.is_empty() { kind } else { path })
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Once per frame build: owners are recorded while the HAL's GPU profiler records, so every
    /// draw it sees carries its owner's name. Arming redraws everything once (retained lists
    /// note their owners only when they are drawn again); a stopped recording disarms. One
    /// relaxed load when no recording runs.
    pub(crate) fn draw_owners_follow_profiler(&mut self) {
        let recording = makepad_hal::gpu_prof::recording();
        match self.draw_owners.as_mut() {
            Some(o) if !recording && o.prof => {
                o.prof = false;
                if !o.capture {
                    self.draw_owners = None;
                }
            }
            Some(o) if recording && !o.prof => o.prof = true,
            None if recording => {
                self.draw_owners = Some(Box::new(DrawOwners { prof: true, ..Default::default() }));
                self.redraw_all();
            }
            _ => {}
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// One frame was built while armed: a capture nobody reads turns itself off.
    pub(crate) fn draw_owners_frame_done(&mut self) {
        if let Some(o) = self.draw_owners.as_mut().filter(|o| o.capture) {
            o.frames += 1;
            if o.frames > MAX_FRAMES {
                o.capture = false;
                o.packets.clear();
                if !o.prof {
                    self.draw_owners = None;
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// The `draw_owners` query's answer: `p <serial> <draw> <first> <count> <uid>` rows, then
    /// `w <uid>\t<path>\t<kind>` for every uid named. Ends the capture (a profiler recording
    /// keeps owners armed).
    pub(crate) fn draw_owners_answer(&mut self) -> String {
        let Some(o) = self.draw_owners.as_mut().filter(|o| o.capture) else { return "error: not armed".into() };
        let packets = std::mem::take(&mut o.packets);
        o.capture = false;
        if !o.prof {
            self.draw_owners = None;
        }
        let mut out = String::new();
        let mut uids: Vec<u64> = Vec::new();
        for (serial, rows) in &packets {
            for [draw, first, count, uid] in rows {
                out.push_str(&format!("p {serial} {draw} {first} {count} {uid}\n"));
                if *uid != 0 && *uid & LABEL_BIT == 0 && !uids.contains(uid) {
                    uids.push(*uid);
                }
            }
        }
        if let Some(paths) = self.widget_uid_paths_callback {
            for (uid, path, kind) in paths(self, &uids) {
                out.push_str(&format!("w {uid}\t{path}\t{kind}\n"));
            }
        }
        // labelled owners name themselves
        let mut labels: Vec<u64> = packets.iter().flat_map(|(_, r)| r.iter().map(|x| x[3])).filter(|u| *u & LABEL_BIT != 0).collect();
        labels.sort_unstable();
        labels.dedup();
        for key in labels {
            if let Some(name) = self.draw_owner_name(key) {
                out.push_str(&format!("w {key}\t{name}\tlabel\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batched_draw_is_named_by_the_owner_of_most_instances() {
        let mut o = DrawOwners::default();
        // 2 floats per instance: owner 7 adds 1 instance, label 3 adds 3, owner 7 adds 1 more
        o.current = 7;
        o.note(1, 0, 0);
        o.current = LABEL_BIT | 3;
        o.note(1, 0, 2);
        o.current = 7;
        o.note(1, 0, 8);
        assert_eq!(o.dominant(1, 0, 2, 5), LABEL_BIT | 3);
        // a draw nobody noted has no owner
        assert_eq!(o.dominant(2, 0, 2, 5), 0);
        // a re-recording from offset 0 replaces the earlier notes
        o.note(1, 0, 0);
        assert_eq!(o.dominant(1, 0, 2, 5), 7);
    }
}
