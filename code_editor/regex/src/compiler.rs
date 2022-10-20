use crate::{
    ast::Quant,
    program,
    program::{Instr, InstrPtr},
    Ast, Program,
};

#[derive(Clone, Debug)]
pub(crate) struct Compiler;

impl Compiler {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn compile(&mut self, ast: &Ast) -> Program {
        CompileContext {
            slot_count: 0,
            instrs: Vec::new(),
        }
        .compile(ast)
    }
}

#[derive(Debug)]
struct CompileContext {
    slot_count: usize,
    instrs: Vec<Instr>,
}

impl CompileContext {
    fn compile(mut self, ast: &Ast) -> Program {
        let frag = self.compile_recursive(ast);
        let instr = self.emit_instr(Instr::Match);
        frag.ends.fill(instr, &mut self.instrs);
        Program {
            start: frag.start,
            slot_count: self.slot_count,
            instrs: self.instrs,
        }
    }

    fn compile_recursive(&mut self, ast: &Ast) -> Frag {
        match *ast {
            Ast::Char(ch) => self.compile_char(ch),
            Ast::Cap(ref ast, index) => self.compile_cap(ast, index),
            Ast::Rep(ref ast, Quant::Quest) => self.compile_quest(ast),
            Ast::Rep(ref ast, Quant::Star) => self.compile_star(ast),
            Ast::Rep(ref ast, Quant::Plus) => self.compile_plus(ast),
            Ast::Cat(ref asts) => self.compile_cat(asts),
            Ast::Alt(ref asts) => self.compile_alt(asts),
        }
    }

    fn compile_char(&mut self, ch: char) -> Frag {
        let start = self.emit_instr(Instr::Char(ch, program::NULL_INSTR_PTR));
        Frag {
            start,
            ends: HolePtrList::unit(HolePtr::next_0(start)),
        }
    }

    fn compile_cap(&mut self, ast: &Ast, cap_index: usize) -> Frag {
        let frag = self.compile_recursive(ast);
        let first_slot_index = cap_index * 2;
        self.slot_count = self.slot_count.max(first_slot_index + 2);
        let instr_0 = self.emit_instr(Instr::Save(first_slot_index, frag.start));
        let instr_1 = self.emit_instr(Instr::Save(first_slot_index + 1, program::NULL_INSTR_PTR));
        frag.ends.fill(instr_1, &mut self.instrs);
        Frag {
            start: instr_0,
            ends: HolePtrList::unit(HolePtr::next_0(instr_1)),
        }
    }

    fn compile_quest(&mut self, ast: &Ast) -> Frag {
        let frag = self.compile_recursive(ast);
        let instr = self.emit_instr(Instr::Split(frag.start, program::NULL_INSTR_PTR));
        Frag {
            start: instr,
            ends: frag.ends.append(HolePtr::next_1(instr), &mut self.instrs),
        }
    }

    fn compile_star(&mut self, ast: &Ast) -> Frag {
        let frag = self.compile_recursive(ast);
        let instr = self.emit_instr(Instr::Split(frag.start, program::NULL_INSTR_PTR));
        frag.ends.fill(instr, &mut self.instrs);
        Frag {
            start: instr,
            ends: HolePtrList::unit(HolePtr::next_1(instr)),
        }
    }

    fn compile_plus(&mut self, ast: &Ast) -> Frag {
        let frag = self.compile_recursive(ast);
        let instr = self.emit_instr(Instr::Split(frag.start, program::NULL_INSTR_PTR));
        frag.ends.fill(instr, &mut self.instrs);
        Frag {
            start: frag.start,
            ends: HolePtrList::unit(HolePtr::next_1(instr)),
        }
    }

    fn compile_cat(&mut self, asts: &[Ast]) -> Frag {
        let mut asts = asts.iter();
        let mut acc_frag = self.compile_recursive(asts.next().unwrap());
        for ast in asts {
            let frag = self.compile_recursive(ast);
            acc_frag.ends.fill(frag.start, &mut self.instrs);
            acc_frag.ends = frag.ends;
        }
        acc_frag
    }

    fn compile_alt(&mut self, asts: &[Ast]) -> Frag {
        let mut asts = asts.iter();
        let mut acc_frag = self.compile_recursive(asts.next().unwrap());
        for ast in asts {
            let frag = self.compile_recursive(ast);
            let instr = self.emit_instr(Instr::Split(acc_frag.start, frag.start));
            acc_frag = Frag {
                start: instr,
                ends: acc_frag.ends.concat(frag.ends, &mut self.instrs),
            };
        }
        acc_frag
    }

    fn emit_instr(&mut self, instr: Instr) -> InstrPtr {
        let instr_ptr = self.instrs.len();
        self.instrs.push(instr);
        instr_ptr
    }
}

#[derive(Debug)]
struct Frag {
    start: InstrPtr,
    ends: HolePtrList,
}

#[derive(Debug)]
struct HolePtrList {
    head: HolePtr,
    tail: HolePtr,
}

impl HolePtrList {
    fn empty() -> Self {
        Self {
            head: HolePtr::null(),
            tail: HolePtr::null(),
        }
    }

    fn unit(hole: HolePtr) -> Self {
        Self {
            head: hole,
            tail: hole,
        }
    }

    fn append(self, hole: HolePtr, instrs: &mut [Instr]) -> Self {
        self.concat(Self::unit(hole), instrs)
    }

    fn concat(self, other: Self, instrs: &mut [Instr]) -> Self {
        if self.tail.is_null() {
            return other;
        }
        if self.head.is_null() {
            return self;
        }
        *self.tail.get_mut(instrs) = other.head.0;
        Self {
            head: self.head,
            tail: other.tail,
        }
    }

    fn fill(self, instr: InstrPtr, instrs: &mut [Instr]) {
        let mut curr = self.head;
        while curr.0 != program::NULL_INSTR_PTR {
            let next = *curr.get(instrs);
            *curr.get_mut(instrs) = instr;
            curr = HolePtr(next);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HolePtr(usize);

impl HolePtr {
    fn null() -> Self {
        Self(program::NULL_INSTR_PTR)
    }

    fn next_0(instr: InstrPtr) -> Self {
        Self(instr << 1)
    }

    fn next_1(instr: InstrPtr) -> Self {
        Self(instr << 1 | 1)
    }

    fn is_null(self) -> bool {
        self.0 == program::NULL_INSTR_PTR
    }

    fn get(self, instrs: &[Instr]) -> &InstrPtr {
        let instr_ref = &instrs[self.0 >> 1];
        if self.0 & 1 == 0 {
            instr_ref.next_0()
        } else {
            instr_ref.next_1()
        }
    }

    fn get_mut(self, instrs: &mut [Instr]) -> &mut InstrPtr {
        let instr_ref = &mut instrs[self.0 >> 1];
        if self.0 & 1 == 0 {
            instr_ref.next_0_mut()
        } else {
            instr_ref.next_1_mut()
        }
    }
}
