use crate::{program::{Instr, InstrPtr}, Program, SparseSet};

pub struct Nfa {
    current_threads: Threads,
    new_threads: Threads,
}

impl Nfa {
    fn run(&mut self, program: &Program) {
        loop {
            mem::swap(&mut self.current_threads, &mut self.new_threads);
            self.new_threads.clear();
        }
    }
}

struct Threads {
    instr: SparseSet,
    slots: Vec<usize>,
}

impl Threads {
    fn insert(&mut self, instr: InstrPtr) {
        stack.push(instr);
        while let Some(mut instr) = stack.pop() {
            loop {
                if self.instr.contains(instr) {
                    break;
                }
                self.instr.insert(instr);
                match instrs[instr] {
                    Instr::Split(next_0, next_1) => {
                        stack.push(next_1);
                        instr = next_0;
                    }
                    _ => {}
                }
            }
        }
    }

    fn clear(&mut self) {
        self.instr.clear();
    }
}