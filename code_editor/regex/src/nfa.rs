use crate::{
    cursor::Cursor,
    program::{Instr, InstrPtr},
    Program, SparseSet,
};

#[derive(Clone, Debug)]
pub(crate) struct Nfa {
    current_threads: Threads,
    new_threads: Threads,
    add_thread_stack: Vec<AddThreadStackFrame>,
}

impl Nfa {
    pub(crate) fn new() -> Self {
        Self {
            current_threads: Threads::new(0, 0),
            new_threads: Threads::new(0, 0),
            add_thread_stack: Vec::new(),
        }
    }

    pub(crate) fn run<C: Cursor>(
        &mut self,
        program: &Program,
        mut cursor: C,
        slots: &mut [Option<usize>],
    ) -> bool {
        use std::mem;

        if self.current_threads.instr.capacity() != program.instrs.len() {
            self.current_threads = Threads::new(program.instrs.len(), program.slot_count);
            self.new_threads = Threads::new(program.instrs.len(), program.slot_count);
        }
        let mut matched = false;
        while !matched {
            self.current_threads.add_thread(
                program.start,
                cursor.byte_position(),
                &program.instrs,
                slots,
                &mut self.add_thread_stack,
            );
            let ch = cursor.current_char().map_or(u32::MAX, |ch| ch as u32);
            if ch != u32::MAX {
                cursor.move_next_char();
            }
            for &instr in &self.current_threads.instr {
                match program.instrs[instr] {
                    Instr::Match => {
                        slots.copy_from_slice(self.current_threads.slots.get_mut(instr));
                        matched = true;
                        break;
                    }
                    Instr::Char(other_ch, next) => {
                        if other_ch as u32 == ch {
                            self.new_threads.add_thread(
                                next,
                                cursor.byte_position(),
                                &program.instrs,
                                self.current_threads.slots.get_mut(instr),
                                &mut self.add_thread_stack,
                            );
                        }
                    }
                    _ => {}
                }
            }
            if cursor.is_at_back() {
                break;
            }
            mem::swap(&mut self.current_threads, &mut self.new_threads);
            self.new_threads.instr.clear();
        }
        matched
    }
}

#[derive(Clone, Debug)]
struct Threads {
    instr: SparseSet,
    slots: Slots,
}

impl Threads {
    fn new(instr_count: usize, slot_count: usize) -> Self {
        Self {
            instr: SparseSet::new(instr_count),
            slots: Slots::new(instr_count, slot_count),
        }
    }

    fn add_thread(
        &mut self,
        instr: InstrPtr,
        byte_position: usize,
        instrs: &[Instr],
        slots: &mut [Option<usize>],
        stack: &mut Vec<AddThreadStackFrame>,
    ) {
        stack.push(AddThreadStackFrame::AddThread(instr));
        while let Some(frame) = stack.pop() {
            match frame {
                AddThreadStackFrame::AddThread(mut instr) => loop {
                    if self.instr.contains(instr) {
                        break;
                    }
                    self.instr.insert(instr);
                    match instrs[instr] {
                        Instr::Split(next_0, next_1) => {
                            stack.push(AddThreadStackFrame::AddThread(next_1));
                            instr = next_0;
                        }
                        Instr::Save(slot_index, next) => {
                            stack.push(AddThreadStackFrame::RestoreSlot(
                                slot_index,
                                slots[slot_index],
                            ));
                            slots[slot_index] = Some(byte_position);
                            instr = next;
                        }
                        _ => {
                            self.slots.get_mut(instr).copy_from_slice(slots);
                            break;
                        }
                    }
                },
                AddThreadStackFrame::RestoreSlot(slot_index, byte_position) => {
                    slots[slot_index] = byte_position;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum AddThreadStackFrame {
    AddThread(InstrPtr),
    RestoreSlot(usize, Option<usize>),
}

#[derive(Clone, Debug)]
struct Slots {
    slot_count_per_thread: usize,
    slots: Vec<Option<usize>>,
}

impl Slots {
    fn new(instr_count: usize, slot_count: usize) -> Self {
        Self {
            slot_count_per_thread: slot_count,
            slots: vec![None; instr_count * slot_count],
        }
    }

    fn get(&self, instr: InstrPtr) -> &[Option<usize>] {
        &self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }

    fn get_mut(&mut self, instr: InstrPtr) -> &mut [Option<usize>] {
        &mut self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }
}
