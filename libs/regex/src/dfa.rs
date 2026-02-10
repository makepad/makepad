use {
    super::{
        char::CharExt,
        input::Cursor,
        leb128,
        prog::{Inst, InstPtr, Pred, Prog},
        sparse_set::SparseSet,
    },
    std::{collections::HashMap, rc::Rc, result},
};

pub type Result<T> = result::Result<T, Error>;

#[derive(Debug)]
pub struct Error;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub want_first_match: bool,
    pub want_last_match: bool,
    pub max_heap_usage: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            want_first_match: false,
            want_last_match: false,
            max_heap_usage: 1 << 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cache {
    heap_usage: usize,
    clear_count: usize,
    states: States,
    start_state_cache: Box<[StatePtr]>,
    state_cache: HashMap<StateKey, StatePtr>,
    curr_insts: SparseSet,
    next_insts: SparseSet,
    add_inst_stack: Vec<InstPtr>,
}

impl Cache {
    pub fn new(prog: &Prog) -> Self {
        Self {
            heap_usage: 0,
            clear_count: 0,
            states: States::new(prog.byte_class_count() + 1),
            start_state_cache: vec![UNKNOWN_STATE; 1 << 5].into_boxed_slice(),
            state_cache: HashMap::new(),
            curr_insts: SparseSet::new(prog.insts.len()),
            next_insts: SparseSet::new(prog.insts.len()),
            add_inst_stack: Vec::new(),
        }
    }
}

pub fn run<C: Cursor>(
    prog: &Prog,
    cursor: C,
    options: Options,
    cache: &mut Cache,
) -> Result<Option<usize>> {
    Dfa {
        prog,
        cursor,
        last_clear_pos: 0,
        want_first_match: options.want_first_match,
        want_last_match: options.want_last_match,
        max_heap_usage: options.max_heap_usage,
        heap_usage: &mut cache.heap_usage,
        clear_count: &mut cache.clear_count,
        states: &mut cache.states,
        start_state_cache: &mut cache.start_state_cache,
        state_cache: &mut cache.state_cache,
        curr_insts: &mut cache.curr_insts,
        next_insts: &mut cache.next_insts,
        add_inst_stack: &mut cache.add_inst_stack,
    }
    .run()
}

#[derive(Debug)]
struct Dfa<'a, C> {
    prog: &'a Prog,
    cursor: C,
    last_clear_pos: usize,
    want_first_match: bool,
    want_last_match: bool,
    max_heap_usage: usize,
    heap_usage: &'a mut usize,
    clear_count: &'a mut usize,
    states: &'a mut States,
    start_state_cache: &'a mut Box<[StatePtr]>,
    state_cache: &'a mut HashMap<StateKey, StatePtr>,
    curr_insts: &'a mut SparseSet,
    next_insts: &'a mut SparseSet,
    add_inst_stack: &'a mut Vec<InstPtr>,
}

impl<'a, C: Cursor> Dfa<'a, C> {
    fn run(&mut self) -> Result<Option<usize>> {
        let mut matched = None;
        let start_state = self.start_state()?;
        let mut curr_state = start_state;
        let mut next_state = start_state;
        let mut b = self.cursor.current_byte();
        loop {
            while next_state <= MAX_STATE && b.is_some() {
                self.cursor.move_next_byte();
                curr_state = next_state;
                let byte_class = self.prog.byte_classes[b.unwrap() as usize] as u16;
                next_state = *self.states.next_state(curr_state, byte_class);
                b = self.cursor.current_byte();
            }
            if next_state & MATCH_STATE != 0 {
                self.cursor.move_prev_byte();
                matched = Some(self.cursor.index());
                self.cursor.move_next_byte();
                if self.want_first_match {
                    return Ok(matched);
                }
                next_state &= !MATCH_STATE;
                continue;
            }
            if next_state == UNKNOWN_STATE {
                let b = self.cursor.prev_byte();
                next_state = self.next_state(&mut curr_state, b)?;
                let byte_class = match b {
                    Some(b) => self.prog.byte_classes[b as usize] as u16,
                    None => self.prog.byte_class_count() as u16,
                };
                *self.states.next_state_mut(curr_state, byte_class) = next_state;
                continue;
            } else if next_state == DEAD_STATE {
                return Ok(matched);
            } else if next_state == ERROR_STATE {
                return Err(Error);
            }
            break;
        }
        next_state &= MAX_STATE;
        curr_state = next_state;
        next_state = self.next_state(&mut curr_state, None)?;
        if next_state & MATCH_STATE != 0 {
            matched = Some(self.cursor.index());
        }
        Ok(matched)
    }

    fn start_state(&mut self) -> Result<StatePtr> {
        let next_is_word = self
            .cursor
            .current_byte()
            .map_or(false, |b| (b as char).is_ascii_word());
        let prev_is_word = self
            .cursor
            .prev_byte()
            .map_or(false, |b| (b as char).is_ascii_word());
        let mut flags = StateFlags::default();
        if prev_is_word {
            flags.set_word();
        }
        let preds = Preds {
            text_start: self.cursor.prev_byte().is_none(),
            text_end: self.cursor.current_byte().is_none(),
            line_start: self.cursor.prev_byte().map_or(true, |b| b == b'\n'),
            line_end: self.cursor.current_byte().map_or(true, |b| b == b'\n'),
            word_boundary: next_is_word != prev_is_word,
        };
        let index = preds.bits() as usize;
        match self.start_state_cache[index] {
            UNKNOWN_STATE => {}
            state => return Ok(state),
        };
        AddInstView {
            prog: &self.prog,
            stack: &mut self.add_inst_stack,
        }
        .add_inst(self.curr_insts, self.prog.start, preds);
        let key = CreateStateKeyView { prog: &self.prog }
            .create_state_key(flags, self.curr_insts.as_slice());
        self.curr_insts.clear();
        let state = self.get_or_add_state(key, None)?;
        self.start_state_cache[index] = state;
        Ok(state)
    }

    fn next_state(&mut self, state: &mut StatePtr, b: Option<u8>) -> Result<StatePtr> {
        use std::mem;

        for inst in self.states.key(*state).insts() {
            self.curr_insts.insert(inst);
        }
        if self.states.key(*state).flags.assert() {
            let next_is_word = b.map_or(false, |b| (b as char).is_ascii_word());
            let prev_is_word = self.states.key(*state).flags.word();
            let preds = Preds {
                text_end: b.is_none(),
                line_end: b.map_or(true, |b| b == b'\n'),
                word_boundary: next_is_word != prev_is_word,
                ..Preds::default()
            };
            for &inst in self.curr_insts.as_slice() {
                AddInstView {
                    prog: &self.prog,
                    stack: &mut self.add_inst_stack,
                }
                .add_inst(self.next_insts, inst, preds);
            }
            mem::swap(&mut self.curr_insts, &mut self.next_insts);
            self.next_insts.clear();
        }
        let mut flags = StateFlags::default();
        if b.map_or(false, |b| (b as char).is_ascii_word()) {
            flags.set_word();
        }
        let preds = Preds {
            line_start: b.map_or(true, |b| b as char == '\n'),
            ..Preds::default()
        };
        for &inst in self.curr_insts.as_slice() {
            match &self.prog.insts[inst] {
                Inst::Match => {
                    flags.set_matched();
                    if !self.want_last_match {
                        break;
                    }
                }
                Inst::ByteRange(inst_ref) => {
                    if b.map_or(false, |b| inst_ref.range.contains(&b)) {
                        AddInstView {
                            prog: &self.prog,
                            stack: &mut self.add_inst_stack,
                        }
                        .add_inst(self.next_insts, inst_ref.out, preds);
                    }
                }
                Inst::Char(_) | Inst::Class(_) => panic!(),
                _ => {}
            }
        }
        mem::swap(&mut self.curr_insts, &mut self.next_insts);
        self.next_insts.clear();
        if !flags.matched() && self.curr_insts.is_empty() {
            return Ok(DEAD_STATE);
        }
        let key = CreateStateKeyView { prog: &self.prog }
            .create_state_key(flags, self.curr_insts.as_slice());
        self.curr_insts.clear();
        let mut next_state = self.get_or_add_state(key, Some(state))?;
        if flags.matched() {
            next_state |= MATCH_STATE;
        }
        Ok(next_state)
    }

    fn get_or_add_state(
        &mut self,
        key: StateKey,
        retained_state: Option<&mut StatePtr>,
    ) -> Result<StatePtr> {
        if let Some(&state) = self.state_cache.get(&key) {
            return Ok(state);
        }
        if *self.heap_usage > self.max_heap_usage {
            match retained_state {
                Some(retained_state) => {
                    let key = self.states.key(*retained_state).clone();
                    self.clear()?;
                    *retained_state = self.add_state(key.clone());
                }
                None => self.clear()?,
            }
        }
        Ok(self.add_state(key))
    }

    fn add_state(&mut self, key: StateKey) -> StatePtr {
        use std::mem;

        *self.heap_usage += mem::size_of::<StateKey>()
            + self.states.byte_class_count * mem::size_of::<StatePtr>()
            + mem::size_of::<u64>()
            + mem::size_of::<StateKey>()
            + mem::size_of::<StatePtr>()
            + key.bytes.len();
        let state = self.states.add(key.clone());
        if self.prog.has_word_boundary {
            for b in 128..256 {
                *self.states.next_state_mut(state, b) = ERROR_STATE;
            }
        }
        self.state_cache.insert(key, state);
        state
    }

    fn clear(&mut self) -> Result<()> {
        let byte_count = self.cursor.index() - self.last_clear_pos;
        if *self.clear_count >= 3 && byte_count <= 10 * self.prog.insts.len() {
            return Err(Error);
        }
        self.last_clear_pos = self.cursor.index();
        *self.heap_usage = 0;
        *self.clear_count += 1;
        self.states.clear();
        for state in self.start_state_cache.iter_mut() {
            *state = UNKNOWN_STATE;
        }
        self.state_cache.clear();
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct States {
    key: Vec<StateKey>,
    transitions: Vec<StatePtr>,
    byte_class_count: usize,
}

impl States {
    #[inline]
    fn new(byte_class_count: usize) -> Self {
        Self {
            key: Vec::new(),
            transitions: Vec::new(),
            byte_class_count,
        }
    }

    fn key(&self, state: StatePtr) -> &StateKey {
        &self.key[state]
    }

    #[inline]
    fn next_state(&self, state: StatePtr, b: u16) -> &StatePtr {
        self.transitions
            .get(self.byte_class_count * state + b as usize)
            .unwrap()
    }

    #[inline]
    fn next_state_mut(&mut self, state: StatePtr, b: u16) -> &mut StatePtr {
        self.transitions
            .get_mut(self.byte_class_count * state + b as usize)
            .unwrap()
    }

    fn add(&mut self, key: StateKey) -> StatePtr {
        use std::iter;

        let ptr = self.key.len();
        self.key.push(key);
        self.transitions
            .extend(iter::repeat(UNKNOWN_STATE).take(self.byte_class_count));
        ptr
    }

    fn clear(&mut self) {
        self.key.clear();
        self.transitions.clear();
    }
}

type StatePtr = usize;

const UNKNOWN_STATE: StatePtr = 1 << 31;
const DEAD_STATE: StatePtr = UNKNOWN_STATE + 1;
const ERROR_STATE: StatePtr = DEAD_STATE + 1;
const MATCH_STATE: StatePtr = 1 << 30;
const MAX_STATE: StatePtr = MATCH_STATE - 1;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StateKey {
    flags: StateFlags,
    bytes: Rc<[u8]>,
}

impl StateKey {
    fn insts(&self) -> Insts<'_> {
        Insts {
            prev_inst: 0,
            bytes: self.bytes.as_ref(),
        }
    }
}

struct Insts<'a> {
    prev_inst: InstPtr,
    bytes: &'a [u8],
}

impl<'a> Iterator for Insts<'a> {
    type Item = InstPtr;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        let delta = leb128::decode_isize(&mut self.bytes);
        let inst = (self.prev_inst as isize + delta) as usize;
        self.prev_inst = inst as usize;
        Some(inst)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct StateFlags(u8);

impl StateFlags {
    fn matched(&self) -> bool {
        self.0 & 1 << 0 != 0
    }

    fn set_matched(&mut self) {
        self.0 |= 1 << 0
    }

    fn assert(&self) -> bool {
        self.0 & 1 << 1 != 0
    }

    fn set_assert(&mut self) {
        self.0 |= 1 << 1;
    }

    fn word(&self) -> bool {
        self.0 & 1 << 2 != 0
    }

    fn set_word(&mut self) {
        self.0 |= 1 << 2;
    }
}

#[derive(Debug)]
struct CreateStateKeyView<'a> {
    prog: &'a Prog,
}

impl<'a> CreateStateKeyView<'a> {
    fn create_state_key(&self, flags: StateFlags, insts: &[InstPtr]) -> StateKey {
        let mut flags = flags;
        let mut bytes = Vec::new();
        let mut prev_inst = 0;
        for &inst in insts {
            match self.prog.insts[inst] {
                Inst::Assert(_) => {
                    flags.set_assert();
                }
                _ => {}
            }
            let delta = (inst as isize) - (prev_inst as isize);
            prev_inst = inst;
            leb128::encode_isize(&mut bytes, delta);
        }
        StateKey {
            flags,
            bytes: Rc::from(bytes),
        }
    }
}

struct AddInstView<'a> {
    prog: &'a Prog,
    stack: &'a mut Vec<InstPtr>,
}

impl<'a> AddInstView<'a> {
    fn add_inst(&mut self, insts: &mut SparseSet, inst: InstPtr, preds: Preds) {
        self.stack.push(inst);
        while let Some(inst) = self.stack.pop() {
            let mut inst = inst;
            loop {
                if !insts.insert(inst) {
                    break;
                }
                match &self.prog.insts[inst] {
                    Inst::Match | Inst::ByteRange(_) | Inst::Char(_) | Inst::Class(_) => {}
                    Inst::Nop(inst_ref) => {
                        inst = inst_ref.out;
                        continue;
                    }
                    Inst::Save(inst_ref) => {
                        inst = inst_ref.out;
                        continue;
                    }
                    Inst::Assert(inst_ref) => {
                        if match inst_ref.pred {
                            Pred::TextStart => preds.text_start,
                            Pred::TextEnd => preds.text_end,
                            Pred::LineStart => preds.line_start,
                            Pred::LineEnd => preds.line_end,
                            Pred::WordBoundary => preds.word_boundary,
                            Pred::NotWordBoundary => !preds.word_boundary,
                        } {
                            inst = inst_ref.out;
                            continue;
                        }
                        insts.insert(inst);
                        break;
                    }
                    Inst::Split(inst_ref) => {
                        self.stack.push(inst_ref.out_1);
                        inst = inst_ref.out_0;
                        continue;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Preds {
    text_start: bool,
    text_end: bool,
    line_start: bool,
    line_end: bool,
    word_boundary: bool,
}

impl Preds {
    fn bits(self) -> u8 {
        let mut bits = 0;
        bits |= (self.text_start as u8) << 0;
        bits |= (self.text_end as u8) << 1;
        bits |= (self.line_start as u8) << 2;
        bits |= (self.line_end as u8) << 3;
        bits |= (self.word_boundary as u8) << 4;
        bits
    }
}
