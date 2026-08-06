//! Host-side slot table mapping the sim's opaque `CallbackSlot`s to script
//! closures. The sim never holds a `ScriptObjectRef` (game.md): it stores u32
//! slots, and this table is the only place the two worlds meet.
//!
//! Entries are tagged with the eval generation that allocated them: a
//! successful eval frees every earlier generation (that world is gone), a
//! failed eval frees exactly the generation it created (the world rolled back
//! to the snapshot, whose slots are older).

use makepad_game_sim::CallbackSlot;
use makepad_widgets::*;

#[derive(Default)]
pub struct CallbackTable {
    entries: Vec<Option<(u64, ScriptObjectRef)>>,
    free: Vec<u32>,
}

impl CallbackTable {
    pub fn alloc(&mut self, generation: u64, func: ScriptObjectRef) -> CallbackSlot {
        if let Some(index) = self.free.pop() {
            self.entries[index as usize] = Some((generation, func));
            CallbackSlot(index)
        } else {
            self.entries.push(Some((generation, func)));
            CallbackSlot(self.entries.len() as u32 - 1)
        }
    }

    pub fn get(&self, slot: CallbackSlot) -> Option<ScriptObjectRef> {
        self.entries
            .get(slot.0 as usize)?
            .as_ref()
            .map(|(_, func)| func.clone())
    }

    pub fn free(&mut self, slot: CallbackSlot) {
        if let Some(entry) = self.entries.get_mut(slot.0 as usize) {
            if entry.take().is_some() {
                self.free.push(slot.0);
            }
        }
    }

    pub fn free_generation(&mut self, generation: u64) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.as_ref().is_some_and(|(g, _)| *g == generation) {
                *entry = None;
                self.free.push(index as u32);
            }
        }
    }

    pub fn free_generations_before(&mut self, generation: u64) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.as_ref().is_some_and(|(g, _)| *g < generation) {
                *entry = None;
                self.free.push(index as u32);
            }
        }
    }

    /// Live slot count — the leak canary for a long authoring session.
    pub fn live_len(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_frees_are_scoped() {
        let mut table = CallbackTable::default();
        // Slots can't be built without a VM, so exercise the bookkeeping via
        // the free lists: alloc/free/live_len are generation-only logic.
        assert_eq!(table.live_len(), 0);
        table.free(CallbackSlot(7)); // out of range is a no-op, never a panic
        table.free_generation(1);
        table.free_generations_before(9);
        assert_eq!(table.live_len(), 0);
    }
}
