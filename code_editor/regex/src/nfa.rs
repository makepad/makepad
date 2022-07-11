use crate::{
    program::{Inst, InstPtr},
    Cursor, Program, SparseSet,
};

pub struct Nfa {
    current_threads: Threads,
    new_threads: Threads,
    add_thread_stack: Vec<InstPtr>,
}

impl Nfa {
    pub fn new(program: &Program) -> Self {
        Self {
            current_threads: Threads::new(program.insts.len()),
            new_threads: Threads::new(program.insts.len()),
            add_thread_stack: Vec::new(),
        }
    }

    pub fn run<C: Cursor>(&mut self, program: &Program, mut cursor: C) {
        use std::mem;

        let mut matched = false;
        loop {
            if !matched {
                self.new_threads.add_thread(
                    program.start,
                    &program.insts,
                    &mut self.add_thread_stack,
                );
            }
            mem::swap(&mut self.current_threads, &mut self.new_threads);
            self.new_threads.inst.clear();
            if self.current_threads.inst.is_empty() {
                break;
            }
            let c0 = cursor.peek_char();
            cursor.skip_char();
            for &inst in &self.current_threads.inst {
                match program.insts[inst] {
                    Inst::Match => {
                        matched = true;
                        break;
                    }
                    Inst::Char(next, c1) if c0.map_or(false, |c0| c0 == c1) => {
                        self.new_threads.add_thread(
                            next,
                            &program.insts,
                            &mut self.add_thread_stack,
                        );
                    }
                    _ => {}
                }
            }
            if c0.is_none() {
                break;
            }
        }
    }
}

struct Threads {
    inst: SparseSet,
}

impl Threads {
    fn new(count: usize) -> Self {
        Self {
            inst: SparseSet::new(count),
        }
    }

    fn add_thread(&mut self, inst: InstPtr, insts: &[Inst], stack: &mut Vec<InstPtr>) {
        stack.push(inst);
        while let Some(mut inst) = stack.pop() {
            while self.inst.insert(inst) {
                match insts[inst] {
                    Inst::Match | Inst::Char(_, _) => break,
                    Inst::Split(next_0, next_1) => {
                        stack.push(next_1);
                        inst = next_0;
                    }
                }
            }
        }
    }
}
