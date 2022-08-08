pub struct Nfa {
    current_threads: Threads,
    new_threads: Threads,
}

impl Nfa {
    fn run(program: &Program) {

    }
}

struct Threads {
    instr: SparseSet
}

impl Threads {
    fn add_thread(&mut self, instr: InstrPtr, stack: &mut Vec<InstPtr>) {
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
}