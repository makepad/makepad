use crate::{
    cursor::Cursor,
    program::{Instr, InstrPtr},
    Program, SparseSet,
};

pub(crate) struct Nfa {
    current_threads: SparseSet,
    new_threads: SparseSet,
    stack: Vec<usize>,
}

impl Nfa {
    pub(crate) fn new() -> Self {
        Self {
            current_threads: SparseSet::new(),
            new_threads: SparseSet::new(),
            stack: Vec::new(),
        }
    }

    pub(crate) fn run<C: Cursor>(&mut self, program: &Program, mut cursor: C) -> bool {
        use std::mem;

        if self.current_threads.capacity() != program.instrs.len() {
            self.current_threads = SparseSet::with_capacity(program.instrs.len());
            self.new_threads = SparseSet::with_capacity(program.instrs.len());
        }
        let mut matched = false;
        while !matched {
            add_thread(
                &mut self.current_threads,
                program.start,
                &program.instrs,
                &mut self.stack,
            );
            let ch = cursor.current().map_or(u32::MAX, |ch| ch as u32);
            println!("{:?} {:?}", self.current_threads, cursor.byte_position());
            for instr in &self.current_threads {
                match program.instrs[instr] {
                    Instr::Match => {
                        matched = true;
                        break;
                    }
                    Instr::Char(other_ch, next) => {
                        if other_ch as u32 == ch {
                            add_thread(&mut self.new_threads, next, &program.instrs, &mut self.stack);
                        }
                    }
                    _ => {}
                }
            }
            if cursor.is_at_back() {
                break;
            }
            cursor.move_next();
            mem::swap(&mut self.current_threads, &mut self.new_threads);
            self.new_threads.clear();
        }
        matched
    }
}

fn add_thread(threads: &mut SparseSet, instr: InstrPtr, instrs: &[Instr], stack: &mut Vec<usize>) {
    stack.push(instr);
    while let Some(mut instr) = stack.pop() {
        loop {
            if threads.contains(instr) {
                break;
            }
            threads.insert(instr);
            match instrs[instr] {
                Instr::Split(next_0, next_1) => {
                    stack.push(next_1);
                    instr = next_0;
                }
                _ => break,
            }
        }
    }
}
