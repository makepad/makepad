use crate::{
    program,
    program::{Inst, InstPtr},
    Ast, Program,
};

pub fn generate(regex: &Ast) -> Program {
    GenerateContext { insts: Vec::new() }.generate(regex)
}

struct GenerateContext {
    insts: Vec<Inst>,
}

impl GenerateContext {
    fn generate(mut self, ast: &Ast) -> Program {
        self.emit_inst(Inst::Nop);
        let fragment = self.generate_recursive(ast);
        let inst = self.emit_inst(Inst::Match);
        fragment.ends.fill(inst, &mut self.insts);
        Program {
            insts: self.insts,
            start: fragment.start,
        }
    }

    fn generate_recursive(&mut self, ast: &Ast) -> Fragment {
        match *ast {
            Ast::Alt(ref asts) => {
                let mut asts = asts.into_iter();
                let mut acc = self.generate_recursive(asts.next().unwrap());
                for ast in asts {
                    let fragment = self.generate_recursive(ast);
                    acc = self.generate_alt(acc, fragment);
                }
                acc
            }
            Ast::Cat(ref asts) => {
                let mut asts = asts.into_iter();
                let mut acc = self.generate_recursive(asts.next().unwrap());
                for ast in asts {
                    let fragment = self.generate_recursive(ast);
                    acc = self.generate_cat(acc, fragment);
                }
                acc
            }
            Ast::Quest(ref asts) => {
                let fragment = self.generate_recursive(asts);
                self.generate_quest(fragment)
            }
            Ast::Star(ref ast) => {
                let fragment = self.generate_recursive(ast);
                self.generate_star(fragment)
            }
            Ast::Plus(ref ast) => {
                let fragment = self.generate_recursive(ast);
                self.generate_plus(fragment)
            }
            Ast::Char(c) => self.generate_char(c),
        }
    }

    fn generate_alt(&mut self, fragment_0: Fragment, fragment_1: Fragment) -> Fragment {
        let inst = self.emit_inst(Inst::split(fragment_0.start, fragment_1.start));
        Fragment {
            start: inst,
            ends: fragment_0.ends.concat(fragment_1.ends, &mut self.insts),
        }
    }

    fn generate_cat(&mut self, fragment_0: Fragment, fragment_1: Fragment) -> Fragment {
        fragment_1.ends.fill(fragment_0.start, &mut self.insts);
        Fragment {
            start: fragment_1.start,
            ends: fragment_0.ends,
        }
    }

    fn generate_quest(&mut self, fragment: Fragment) -> Fragment {
        let inst = self.emit_inst(Inst::split(fragment.start, program::NULL_INST_PTR));
        Fragment {
            start: inst,
            ends: fragment.ends.append(HolePtr::next_1(inst), &mut self.insts),
        }
    }

    fn generate_star(&mut self, fragment: Fragment) -> Fragment {
        let inst = self.emit_inst(Inst::split(fragment.start, program::NULL_INST_PTR));
        fragment.ends.fill(inst, &mut self.insts);
        Fragment {
            start: inst,
            ends: HolePtrList::unit(HolePtr::next_1(inst)),
        }
    }

    fn generate_plus(&mut self, fragment: Fragment) -> Fragment {
        let inst = self.emit_inst(Inst::split(fragment.start, program::NULL_INST_PTR));
        fragment.ends.fill(inst, &mut self.insts);
        Fragment {
            start: fragment.start,
            ends: HolePtrList::unit(HolePtr::next_1(inst)),
        }
    }

    fn generate_char(&mut self, c: char) -> Fragment {
        let inst = self.emit_inst(Inst::char(program::NULL_INST_PTR, c));
        Fragment {
            start: inst,
            ends: HolePtrList::unit(HolePtr::next_0(inst)),
        }
    }

    fn emit_inst(&mut self, inst: Inst) -> InstPtr {
        let ptr = self.insts.len();
        self.insts.push(inst);
        ptr
    }
}

struct Fragment {
    start: InstPtr,
    ends: HolePtrList,
}

struct HolePtrList {
    head: HolePtr,
    tail: HolePtr,
}

impl HolePtrList {
    fn unit(hole: HolePtr) -> Self {
        Self {
            head: hole,
            tail: hole,
        }
    }

    fn append(self, hole: HolePtr, insts: &mut [Inst]) -> Self {
        self.concat(Self::unit(hole), insts)
    }

    fn concat(self, other: Self, insts: &mut [Inst]) -> Self {
        if self.tail.is_null() {
            return other;
        }
        if self.head.is_null() {
            return self;
        }
        *self.tail.get_mut(insts) = other.head.0;
        Self {
            head: self.head,
            tail: other.tail,
        }
    }

    fn fill(self, inst: InstPtr, insts: &mut [Inst]) {
        let mut hole = self.head;
        while hole.0 != program::NULL_INST_PTR {
            let next = *hole.get(insts);
            *hole.get_mut(insts) = inst;
            hole = HolePtr(next);
        }
    }
}

#[derive(Clone, Copy)]
struct HolePtr(usize);

impl HolePtr {
    fn next_0(inst: InstPtr) -> Self {
        Self(inst << 1)
    }

    fn next_1(inst: InstPtr) -> Self {
        Self(inst << 1 | 1)
    }

    fn is_null(self) -> bool {
        self.0 == program::NULL_INST_PTR
    }

    fn get(self, insts: &[Inst]) -> &InstPtr {
        let inst_ref = &insts[self.0 >> 1];
        if self.0 & 1 == 0 {
            inst_ref.next_0()
        } else {
            inst_ref.next_1()
        }
    }

    fn get_mut(self, insts: &mut [Inst]) -> &mut InstPtr {
        let inst_ref = &mut insts[self.0 >> 1];
        if self.0 & 1 == 0 {
            inst_ref.next_0_mut()
        } else {
            inst_ref.next_1_mut()
        }
    }
}
