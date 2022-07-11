use crate::{
    ast::Regex,
    program,
    program::{Inst, InstPtr},
};

pub struct Generator {
    insts: Vec<Inst>,
}

impl Generator {
    fn generate(&mut self, regex: &Regex) -> Frag {
        match *regex {
            Regex::Alt(ref regexes) => {
                let mut regexes = regexes.into_iter();
                let mut acc = self.generate(regexes.next().unwrap());
                for regex in regexes {
                    let frag = self.generate(regex);
                    acc = self.generate_alt(acc, frag);
                }
                acc
            }
            Regex::Cat(ref regexes) => {
                let mut regexes = regexes.into_iter();
                let mut acc = self.generate(regexes.next().unwrap());
                for regex in regexes {
                    let frag = self.generate(regex);
                    acc = self.generate_cat(acc, frag);
                }
                acc
            }
            Regex::Quest(ref regex) => {
                let frag = self.generate(regex);
                self.generate_quest(frag)
            }
            Regex::Star(ref regex) => {
                let frag = self.generate(regex);
                self.generate_star(frag)
            }
            Regex::Plus(ref regex) => {
                let frag = self.generate(regex);
                self.generate_plus(frag)
            }
            Regex::Char(c) => self.generate_char(c),
            _ => unimplemented!(),
        }
    }

    fn generate_alt(&mut self, frag_0: Frag, frag_1: Frag) -> Frag {
        let inst = self.emit_inst(Inst::split(frag_0.start, frag_1.start));
        Frag {
            start: inst,
            ends: frag_0.ends.concat(frag_1.ends, &mut self.insts),
        }
    }

    fn generate_cat(&mut self, frag_0: Frag, frag_1: Frag) -> Frag {
        frag_1.ends.fill(frag_0.start, &mut self.insts);
        Frag {
            start: frag_1.start,
            ends: frag_0.ends,
        }
    }

    fn generate_quest(&mut self, frag: Frag) -> Frag {
        let inst = self.emit_inst(Inst::split(frag.start, program::NULL_INST_PTR));
        Frag {
            start: inst,
            ends: frag.ends.append(HolePtr::next_1(inst), &mut self.insts),
        }
    }

    fn generate_star(&mut self, frag: Frag) -> Frag {
        let inst = self.emit_inst(Inst::split(frag.start, program::NULL_INST_PTR));
        frag.ends.fill(inst, &mut self.insts);
        Frag {
            start: inst,
            ends: HolePtrList::unit(HolePtr::next_1(inst)),
        }
    }

    fn generate_plus(&mut self, frag: Frag) -> Frag {
        let inst = self.emit_inst(Inst::split(frag.start, program::NULL_INST_PTR));
        frag.ends.fill(inst, &mut self.insts);
        Frag {
            start: frag.start,
            ends: HolePtrList::unit(HolePtr::next_1(inst)),
        }
    }

    fn generate_char(&mut self, c: char) -> Frag {
        let inst = self.emit_inst(Inst::char(program::NULL_INST_PTR, c));
        Frag {
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

struct Frag {
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
    fn null() -> Self {
        Self(program::NULL_INST_PTR)
    }

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
