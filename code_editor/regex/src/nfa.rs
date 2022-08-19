use crate::{
    program::{Instr, InstrPtr},
    Cursor, Program, SparseSet,
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

        if self.current_threads.instrs.capacity() != program.instrs.len()
            || self.current_threads.slots.slot_count_per_thread != program.slot_count
        {
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
            let ch = cursor.current_char();
            if ch.is_some() {
                cursor.move_next_char();
            }
            for &instr in &self.current_threads.instrs {
                match program.instrs[instr] {
                    Instr::Match => {
                        slots.copy_from_slice(self.current_threads.slots.get(instr));
                        matched = true;
                        break;
                    }
                    Instr::Char(other_ch, next) => {
                        if ch.map_or(false, |ch| other_ch == ch) {
                            self.new_threads.add_thread(
                                next,
                                cursor.byte_position(),
                                &program.instrs,
                                self.current_threads.slots.get_mut(instr),
                                &mut self.add_thread_stack,
                            );
                        }
                    }
                    Instr::CharClass(ref char_class, next) => {
                        if ch.map_or(false, |ch| char_class.contains(ch)) {
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
            self.new_threads.instrs.clear();
        }
        matched
    }
}

#[derive(Clone, Debug)]
struct Threads {
    instrs: SparseSet,
    slots: Slots,
}

impl Threads {
    fn new(instr_count: usize, slot_count: usize) -> Self {
        Self {
            instrs: SparseSet::new(instr_count),
            slots: Slots {
                slot_count_per_thread: slot_count,
                slots: vec![None; instr_count * slot_count],
            }
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
                    if self.instrs.contains(instr) {
                        break;
                    }
                    self.instrs.insert(instr);
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

#[derive(Clone, Debug)]
struct Slots {
    slot_count_per_thread: usize,
    slots: Vec<Option<usize>>,
}

impl Slots {
    fn get(&self, instr: InstrPtr) -> &[Option<usize>] {
        &self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }

    fn get_mut(&mut self, instr: InstrPtr) -> &mut [Option<usize>] {
        &mut self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }
}

#[derive(Clone, Copy, Debug)]
enum AddThreadStackFrame {
    AddThread(InstrPtr),
    RestoreSlot(usize, Option<usize>),
}
