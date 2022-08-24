use {
    crate::{
        ast::Quant,
        program,
        program::{Instr, InstrPtr},
        utf8, Ast, CharClass, Program, Range,
    },
    std::collections::HashMap,
};

#[derive(Clone, Debug)]
pub(crate) struct Compiler {
    encoder: utf8::Encoder,
    states: Vec<State>,
    instr_cache: HashMap<Instr, InstrPtr>,
}

impl Compiler {
    pub(crate) fn new() -> Self {
        Self {
            encoder: utf8::Encoder::new(),
            states: Vec::new(),
            instr_cache: HashMap::new(),
        }
    }

    pub(crate) fn compile(&mut self, ast: &Ast, options: Options) -> Program {
        CompileContext {
            encoder: &mut self.encoder,
            states: &mut self.states,
            instr_cache: &mut self.instr_cache,
            options,
            slot_count: 0,
            instrs: Vec::new(),
        }
        .compile(ast)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Options {
    pub(crate) dot_star: bool,
    pub(crate) bytewise: bool,
}

#[derive(Debug)]
struct CompileContext<'a> {
    encoder: &'a mut utf8::Encoder,
    states: &'a mut Vec<State>,
    instr_cache: &'a mut HashMap<Instr, InstrPtr>,
    options: Options,
    slot_count: usize,
    instrs: Vec<Instr>,
}

impl<'a> CompileContext<'a> {
    fn compile(mut self, ast: &Ast) -> Program {
        let mut dot_star_frag = Frag {
            start: program::NULL_INSTR_PTR,
            ends: HolePtrList::new(),
        };
        if self.options.dot_star {
            dot_star_frag = self.compile_recursive(&Ast::Rep(
                Box::new(Ast::CharClass(CharClass::any())),
                Quant::Star,
            ));
        }
        let frag = self.compile_recursive(ast);
        let instr = self.emit_instr(Instr::Match);
        frag.ends.fill(instr, &mut self.instrs);
        let mut start = frag.start;
        if self.options.dot_star {
            dot_star_frag.ends.fill(start, &mut self.instrs);
            start = dot_star_frag.start;
        }
        Program {
            slot_count: self.slot_count,
            instrs: self.instrs,
            start,
        }
    }

    fn compile_recursive(&mut self, ast: &Ast) -> Frag {
        match *ast {
            Ast::Char(ch) => self.compile_char(ch),
            Ast::CharClass(ref char_class) => self.compile_char_class(char_class),
            Ast::Cap(ref ast, index) => self.compile_cap(ast, index),
            Ast::Rep(ref ast, Quant::Quest) => self.compile_quest(ast),
            Ast::Rep(ref ast, Quant::Star) => self.compile_star(ast),
            Ast::Rep(ref ast, Quant::Plus) => self.compile_plus(ast),
            Ast::Cat(ref asts) => self.compile_cat(asts),
            Ast::Alt(ref asts) => self.compile_alt(asts),
        }
    }

    fn compile_byte_range(&mut self, byte_range: Range<u8>) -> Frag {
        let instr = self.emit_instr(Instr::ByteRange(byte_range, program::NULL_INSTR_PTR));
        Frag {
            start: instr,
            ends: HolePtrList::unit(HolePtr::next_0(instr)),
        }
    }

    fn compile_char(&mut self, ch: char) -> Frag {
        if self.options.bytewise {
            let mut bytes = [0; 4];
            let mut bytes = ch.encode_utf8(&mut bytes).bytes().rev();
            let byte = bytes.next().unwrap();
            let instr = self.emit_instr(Instr::ByteRange(
                Range::new(byte, byte),
                program::NULL_INSTR_PTR,
            ));
            let mut acc_instr = instr;
            for byte in bytes {
                acc_instr = self.emit_instr(Instr::ByteRange(Range::new(byte, byte), instr));
            }
            Frag {
                start: acc_instr,
                ends: HolePtrList::unit(HolePtr::next_0(instr)),
            }
        } else {
            let instr = self.emit_instr(Instr::Char(ch, program::NULL_INSTR_PTR));
            Frag {
                start: instr,
                ends: HolePtrList::unit(HolePtr::next_0(instr)),
            }
        }
    }

    fn compile_char_class(&mut self, char_class: &CharClass) -> Frag {
        if self.options.bytewise {
            let mut suffix_tree = SuffixTree {
                ends: HolePtrList::new(),
                states: self.states,
                suffix_cache: SuffixCache {
                    instr_cache: self.instr_cache,
                    instrs: &mut self.instrs,
                },
            };
            for char_range in char_class {
                for byte_ranges in self.encoder.encode(char_range) {
                    suffix_tree.add_byte_ranges(&byte_ranges);
                }
            }
            suffix_tree.compile()
        } else {
            let instr = self.emit_instr(Instr::CharClass(
                char_class.clone(),
                program::NULL_INSTR_PTR,
            ));
            Frag {
                start: instr,
                ends: HolePtrList::unit(HolePtr::next_0(instr)),
            }
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
struct SuffixTree<'a> {
    ends: HolePtrList,
    states: &'a mut Vec<State>,
    suffix_cache: SuffixCache<'a>,
}

impl<'a> SuffixTree<'a> {
    fn compile(mut self) -> Frag {
        let start = self.compile_suffix(0);
        self.suffix_cache.instr_cache.clear();
        if start == program::NULL_INSTR_PTR {
            let instr = self
                .suffix_cache
                .emit_instr(Instr::Nop(program::NULL_INSTR_PTR));
            Frag {
                start: instr,
                ends: HolePtrList::unit(HolePtr::next_0(instr)),
            }
        } else {
            Frag {
                start,
                ends: self.ends,
            }
        }
    }

    fn add_byte_ranges(&mut self, byte_ranges: &[Range<u8>]) {
        let index = self.prefix_len(byte_ranges);
        let instr = self.compile_suffix(index);
        self.extend_suffix(instr, &byte_ranges[index..]);
    }

    fn prefix_len(&self, byte_ranges: &[Range<u8>]) -> usize {
        byte_ranges
            .iter()
            .zip(self.states.iter())
            .take_while(|&(&byte_range, state)| byte_range == state.byte_range)
            .count()
    }

    fn compile_suffix(&mut self, start: usize) -> InstrPtr {
        use std::mem;

        let mut acc_instr = program::NULL_INSTR_PTR;
        for state in self.states.drain(start..).rev() {
            let has_hole = acc_instr == program::NULL_INSTR_PTR;
            let (instr, is_new) = self
                .suffix_cache
                .get_or_emit_instr(Instr::ByteRange(state.byte_range, acc_instr));
            acc_instr = instr;
            if is_new && has_hole {
                let ends = mem::replace(&mut self.ends, HolePtrList::new());
                self.ends = ends.append(HolePtr::next_0(instr), &mut self.suffix_cache.instrs);
            }
            if state.instr != program::NULL_INSTR_PTR {
                let (instr, _) = self
                    .suffix_cache
                    .get_or_emit_instr(Instr::Split(state.instr, acc_instr));
                acc_instr = instr;
            }
        }
        acc_instr
    }

    fn extend_suffix(&mut self, compiled_instr: InstrPtr, byte_ranges: &[Range<u8>]) {
        let mut byte_ranges = byte_ranges.iter();
        self.states.push(State {
            instr: compiled_instr,
            byte_range: *byte_ranges.next().unwrap(),
        });
        for &byte_range in byte_ranges {
            self.states.push(State {
                instr: program::NULL_INSTR_PTR,
                byte_range,
            });
        }
    }
}

#[derive(Debug)]
struct SuffixCache<'a> {
    instr_cache: &'a mut HashMap<Instr, InstrPtr>,
    instrs: &'a mut Vec<Instr>,
}

impl<'a> SuffixCache<'a> {
    fn get_or_emit_instr(&mut self, instr: Instr) -> (InstrPtr, bool) {
        match self.instr_cache.get(&instr) {
            Some(&ptr) => (ptr, false),
            None => {
                let ptr = self.emit_instr(instr.clone());
                self.instr_cache.insert(instr, ptr);
                (ptr, true)
            }
        }
    }

    fn emit_instr(&mut self, instr: Instr) -> InstrPtr {
        let instr_ptr = self.instrs.len();
        self.instrs.push(instr);
        instr_ptr
    }
}

#[derive(Clone, Debug)]
struct State {
    instr: InstrPtr,
    byte_range: Range<u8>,
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
    fn new() -> Self {
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
