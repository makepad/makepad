use super::{
    input::Cursor,
    prog::{InstPtr, Pred, Prog},
    sparse_set::SparseSet,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub want_first_match: bool,
}

#[derive(Clone, Debug)]
pub struct Allocs {
    current_threads: Threads,
    next_threads: Threads,
    add_thread_stack: Vec<AddThreadFrame>,
}

impl Allocs {
    pub fn new(prog: &Prog) -> Self {
        Self {
            current_threads: Threads::new(prog.insts.len(), prog.slot_count),
            next_threads: Threads::new(prog.insts.len(), prog.slot_count),
            add_thread_stack: Vec::new(),
        }
    }
}

pub fn run<C: Cursor>(
    prog: &Prog,
    cursor: C,
    options: Options,
    slots: &mut [Option<usize>],
    allocs: &mut Allocs,
) -> bool {
    Nfa {
        prog,
        cursor,
        want_first_match: options.want_first_match,
        curr_threads: &mut allocs.current_threads,
        next_threads: &mut allocs.next_threads,
        add_thread_stack: &mut allocs.add_thread_stack,
    }
    .run(slots)
}

#[derive(Debug)]
struct Nfa<'a, C: Cursor> {
    prog: &'a Prog,
    cursor: C,
    want_first_match: bool,
    curr_threads: &'a mut Threads,
    next_threads: &'a mut Threads,
    add_thread_stack: &'a mut Vec<AddThreadFrame>,
}

impl<'a, C: Cursor> Nfa<'a, C> {
    fn run(&mut self, slots: &mut [Option<usize>]) -> bool {
        use {super::prog::Inst, std::mem};

        let mut matched = false;
        loop {
            if !matched {
                AddThreadView {
                    prog: &self.prog,
                    cursor: &mut self.cursor,
                    stack: &mut self.add_thread_stack,
                }
                .add_thread(&mut self.next_threads, self.prog.start, slots);
            }
            mem::swap(&mut self.curr_threads, &mut self.next_threads);
            self.next_threads.inst.clear();
            if self.curr_threads.inst.is_empty() {
                break;
            }
            let c = self.cursor.current_char();
            if c.is_some() {
                self.cursor.move_next_char();
            }
            let mut view = AddThreadView {
                prog: &self.prog,
                cursor: &mut self.cursor,
                stack: &mut self.add_thread_stack,
            };
            for &inst in self.curr_threads.inst.as_slice() {
                match &self.prog.insts[inst] {
                    Inst::Match => {
                        let len = slots.len().min(self.curr_threads.slots.get(inst).len());
                        (&mut slots[..len])
                            .copy_from_slice(&self.curr_threads.slots.get(inst)[..len]);
                        if self.want_first_match {
                            return true;
                        }
                        matched = true;
                        break;
                    }
                    Inst::ByteRange(_) => panic!(),
                    Inst::Char(inst_ref) => {
                        if c.map_or(false, |c| c == inst_ref.c) {
                            view.add_thread(
                                &mut self.next_threads,
                                inst_ref.out,
                                self.curr_threads.slots.get_mut(inst),
                            );
                        }
                    }
                    Inst::Class(inst_ref) => {
                        if c.map_or(false, |c| inst_ref.class.contains(c)) {
                            view.add_thread(
                                &mut self.next_threads,
                                inst_ref.out,
                                self.curr_threads.slots.get_mut(inst),
                            );
                        }
                    }
                    _ => {}
                }
            }
            if c.is_none() {
                break;
            }
        }
        matched
    }
}

#[derive(Clone, Debug)]
struct Threads {
    inst: SparseSet,
    slots: Slots,
}

impl Threads {
    fn new(inst_count: usize, slot_count: usize) -> Self {
        Self {
            inst: SparseSet::new(inst_count),
            slots: Slots {
                slots: (0..inst_count * slot_count).map(|_| None).collect(),
                slot_count,
            },
        }
    }
}

#[derive(Clone, Debug)]
struct Slots {
    slots: Vec<Option<usize>>,
    slot_count: usize,
}

impl Slots {
    fn get(&self, inst: InstPtr) -> &[Option<usize>] {
        &self.slots[inst * self.slot_count..][..self.slot_count]
    }

    fn get_mut(&mut self, inst: InstPtr) -> &mut [Option<usize>] {
        &mut self.slots[inst * self.slot_count..][..self.slot_count]
    }
}

#[derive(Clone, Debug)]
enum AddThreadFrame {
    AddThread(InstPtr),
    UnsaveSlots(usize, Option<usize>),
}

#[derive(Debug)]
struct AddThreadView<'a, C: Cursor> {
    prog: &'a Prog,
    cursor: &'a mut C,
    stack: &'a mut Vec<AddThreadFrame>,
}

impl<'a, C: Cursor> AddThreadView<'a, C> {
    fn add_thread(&mut self, threads: &mut Threads, inst: InstPtr, slots: &mut [Option<usize>]) {
        use super::prog::Inst;

        self.stack.push(AddThreadFrame::AddThread(inst));
        while let Some(frame) = self.stack.pop() {
            match frame {
                AddThreadFrame::AddThread(inst) => {
                    let mut inst = inst;
                    loop {
                        if !threads.inst.insert(inst) {
                            break;
                        }
                        match &self.prog.insts[inst] {
                            Inst::Match | Inst::ByteRange(_) | Inst::Char(_) | Inst::Class(_) => {
                                let len = threads.slots.get(inst).len().min(slots.len());
                                (&mut threads.slots.get_mut(inst)[..len])
                                    .copy_from_slice(&slots[..len]);
                                break;
                            }
                            Inst::Nop(inst_ref) => {
                                inst = inst_ref.out;
                                continue;
                            }
                            Inst::Save(inst_ref) => {
                                if inst_ref.slot_index < slots.len() {
                                    self.stack.push(AddThreadFrame::UnsaveSlots(
                                        inst_ref.slot_index,
                                        slots[inst_ref.slot_index],
                                    ));
                                    slots[inst_ref.slot_index] = Some(self.cursor.index());
                                }
                                inst = inst_ref.out;
                                continue;
                            }
                            Inst::Assert(inst_ref) => {
                                if match inst_ref.pred {
                                    Pred::TextStart => self.cursor.is_start(),
                                    Pred::TextEnd => self.cursor.is_end(),
                                    Pred::LineStart => self.cursor.is_line_start(),
                                    Pred::LineEnd => self.cursor.is_line_end(),
                                    Pred::WordBoundary => self.cursor.is_word_boundary(),
                                    Pred::NotWordBoundary => !self.cursor.is_word_boundary(),
                                } {
                                    inst = inst_ref.out;
                                    continue;
                                }
                                break;
                            }
                            Inst::Split(inst_ref) => {
                                self.stack.push(AddThreadFrame::AddThread(inst_ref.out_1));
                                inst = inst_ref.out_0;
                                continue;
                            }
                        }
                    }
                }
                AddThreadFrame::UnsaveSlots(slot_index, old_pos) => slots[slot_index] = old_pos,
            }
        }
    }
}
