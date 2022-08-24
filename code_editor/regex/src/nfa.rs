use crate::{
    program::{Instr, InstrPtr},
    Cursor, Program, SparseSet,
};

#[derive(Clone, Debug)]
pub(crate) struct Nfa {
    current_threads: Threads,
    new_threads: Threads,
    stack: Vec<Frame>,
}

impl Nfa {
    pub(crate) fn new() -> Self {
        Self {
            current_threads: Threads::new(0, 0),
            new_threads: Threads::new(0, 0),
            stack: Vec::new(),
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
                &mut self.stack,
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
                                &mut self.stack,
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
                                &mut self.stack,
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
    fn new(thread_count: usize, slot_count_per_thread: usize) -> Self {
        Self {
            instrs: SparseSet::new(thread_count),
            slots: Slots {
                slot_count_per_thread,
                slots: vec![None; thread_count * slot_count_per_thread].into_boxed_slice(),
            },
        }
    }

    fn add_thread(
        &mut self,
        instr: InstrPtr,
        byte_position: usize,
        instrs: &[Instr],
        slots: &mut [Option<usize>],
        stack: &mut Vec<Frame>,
    ) {
        stack.push(Frame::AddThread(instr));
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::AddThread(mut instr) => loop {
                    if self.instrs.contains(instr) {
                        break;
                    }
                    self.instrs.insert(instr);
                    match instrs[instr] {
                        Instr::Nop(next) => {
                            instr = next;
                        }
                        Instr::Save(slot_index, next) => {
                            stack.push(Frame::RestoreSlot(slot_index, slots[slot_index]));
                            slots[slot_index] = Some(byte_position);
                            instr = next;
                        }
                        Instr::Split(next_0, next_1) => {
                            stack.push(Frame::AddThread(next_1));
                            instr = next_0;
                        }
                        _ => {
                            self.slots.get_mut(instr).copy_from_slice(slots);
                            break;
                        }
                    }
                },
                Frame::RestoreSlot(slot_index, byte_position) => {
                    slots[slot_index] = byte_position;
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Slots {
    slot_count_per_thread: usize,
    slots: Box<[Option<usize>]>,
}

impl Slots {
    fn get(&self, instr: InstrPtr) -> &[Option<usize>] {
        &self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }

    fn get_mut(&mut self, instr: InstrPtr) -> &mut [Option<usize>] {
        &mut self.slots[instr * self.slot_count_per_thread..][..self.slot_count_per_thread]
    }
}

#[derive(Clone, Debug)]
enum Frame {
    AddThread(InstrPtr),
    RestoreSlot(usize, Option<usize>),
}
