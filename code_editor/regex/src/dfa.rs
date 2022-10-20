use {
    crate::{
        program::{Instr, InstrPtr, Pred},
        Cursor, Program, SparseSet,
    },
    std::{collections::HashMap, rc::Rc},
};

const MAX_STATE_PTR: StatePtr = (1 << 30) - 1;
const MATCHED_FLAG: StatePtr = 1 << 30;
const UNKNOWN_STATE_PTR: StatePtr = 1 << 31;
const DEAD_STATE_PTR: StatePtr = (1 << 31) + 1;

#[derive(Clone, Debug)]
pub struct Dfa {
    start_state_cache: Box<[StatePtr]>,
    states: States,
    current_threads: Threads,
    next_threads: Threads,
    stack: Vec<InstrPtr>,
}

impl Dfa {
    pub(crate) fn new() -> Self {
        Self {
            start_state_cache: vec![UNKNOWN_STATE_PTR; 1 << 5].into_boxed_slice(),
            states: States {
                state_cache: HashMap::new(),
                state_ids: Vec::new(),
                next_states: Vec::new(),
            },
            current_threads: Threads::new(0),
            next_threads: Threads::new(0),
            stack: Vec::new(),
        }
    }

    pub(crate) fn run<C: Cursor>(
        &mut self,
        program: &Program,
        cursor: C,
        options: Options,
    ) -> Option<usize> {
        if !self.current_threads.instrs.capacity() != program.instrs.len() {
            self.current_threads = Threads::new(program.instrs.len());
            self.next_threads = Threads::new(program.instrs.len());
        }
        RunContext {
            start_state_cache: &mut self.start_state_cache,
            states: &mut self.states,
            current_threads: &mut self.current_threads,
            next_threads: &mut self.next_threads,
            stack: &mut self.stack,
            program,
            cursor,
            options,
        }
        .run()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub stop_after_first_match: bool,
    pub continue_until_last_match: bool,
}

struct RunContext<'a, C> {
    start_state_cache: &'a mut [StatePtr],
    states: &'a mut States,
    current_threads: &'a mut Threads,
    next_threads: &'a mut Threads,
    stack: &'a mut Vec<InstrPtr>,
    program: &'a Program,
    cursor: C,
    options: Options,
}

impl<'a, C: Cursor> RunContext<'a, C> {
    fn run(&mut self) -> Option<usize> {
        let mut matched = None;
        let mut current_state = UNKNOWN_STATE_PTR;
        let mut next_state = self.get_or_create_start_state();
        let mut byte = self.cursor.next_byte();
        loop {
            while next_state <= MAX_STATE_PTR && byte.is_some() {
                current_state = next_state;
                next_state = *self.states.next_state(current_state, byte);
                byte = self.cursor.next_byte();
            }
            if next_state & MATCHED_FLAG != 0 {
                self.cursor.prev_byte().unwrap();
                self.cursor.prev_byte().unwrap();
                matched = Some(self.cursor.byte_position());
                self.cursor.next_byte().unwrap();
                self.cursor.next_byte().unwrap();
                if self.options.stop_after_first_match {
                    return matched;
                }
                next_state &= !MATCHED_FLAG;
            } else if next_state == UNKNOWN_STATE_PTR {
                self.cursor.prev_byte().unwrap();
                let byte = Some(self.cursor.prev_byte().unwrap());
                self.cursor.next_byte().unwrap();
                self.cursor.next_byte().unwrap();
                next_state = self.get_or_create_next_state(current_state, byte);
                *self.states.next_state_mut(current_state, byte) = next_state;
            } else if next_state == DEAD_STATE_PTR {
                return matched;
            } else {
                break;
            }
        }
        next_state &= MAX_STATE_PTR;
        current_state = next_state;
        next_state = self.get_or_create_next_state(current_state, None);
        if next_state & MATCHED_FLAG != 0 {
            matched = Some(self.cursor.byte_position());
        }
        matched
    }

    fn get_or_create_start_state(&mut self) -> StatePtr {
        let preds = Preds {
            is_at_start_of_text: self.cursor.is_at_start_of_text(),
            is_at_end_of_text: self.cursor.is_at_end_of_text(),
        };
        let bits = preds.to_bits() as usize;
        match self.start_state_cache[bits] {
            UNKNOWN_STATE_PTR => {
                let mut flags = Flags::default();
                self.current_threads.add_thread(
                    self.program.start,
                    preds,
                    &mut flags,
                    &self.program.instrs,
                    &mut self.stack,
                );
                let state_id = StateId::new(flags, self.current_threads.instrs.as_slice());
                self.current_threads.instrs.clear();
                let state = self.states.get_or_create_state(state_id);
                self.start_state_cache[bits] = state;
                state
            }
            state => state,
        }
    }

    fn get_or_create_next_state(&mut self, state: StatePtr, byte: Option<u8>) -> StatePtr {
        use std::mem;

        let state_id = &self.states.state_ids[state as usize];
        for instr in state_id.instrs() {
            self.current_threads.instrs.insert(instr);
        }
        let mut flags = Flags::default();
        if state_id.flags.assert() {
            let preds = Preds {
                is_at_end_of_text: byte.is_none(),
                ..Preds::default()
            };
            for &instr in &self.current_threads.instrs {
                self.next_threads.add_thread(
                    instr,
                    preds,
                    &mut flags,
                    &self.program.instrs,
                    &mut self.stack,
                );
            }
            mem::swap(&mut self.current_threads, &mut self.next_threads);
            self.next_threads.instrs.clear();
        }
        for &instr in self.current_threads.instrs.as_slice() {
            match self.program.instrs[instr] {
                Instr::Match => {
                    flags.set_matched();
                    if !self.options.continue_until_last_match {
                        break;
                    }
                }
                Instr::ByteRange(byte_range, next) => {
                    if byte.map_or(false, |byte| byte_range.contains(&byte)) {
                        self.next_threads.add_thread(
                            next,
                            Preds::default(),
                            &mut flags,
                            &self.program.instrs,
                            &mut self.stack,
                        );
                    }
                }
                _ => {}
            }
        }
        self.current_threads.instrs.clear();
        if !flags.matched() && self.next_threads.instrs.is_empty() {
            return DEAD_STATE_PTR;
        }
        let next_state_id = StateId::new(flags, self.next_threads.instrs.as_slice());
        self.next_threads.instrs.clear();
        let mut next_state = self.states.get_or_create_state(next_state_id);
        if flags.matched() {
            next_state |= MATCHED_FLAG;
        }
        next_state
    }
}

#[derive(Clone, Debug)]
struct States {
    state_cache: HashMap<StateId, StatePtr>,
    state_ids: Vec<StateId>,
    next_states: Vec<StatePtr>,
}

impl States {
    fn next_state(&self, state: StatePtr, byte: Option<u8>) -> &StatePtr {
        &self.next_states[state as usize * 257 + byte.map_or(256, |byte| byte as usize)]
    }

    fn next_state_mut(&mut self, state: StatePtr, byte: Option<u8>) -> &mut StatePtr {
        &mut self.next_states[state as usize * 257 + byte.map_or(256, |byte| byte as usize)]
    }

    fn get_or_create_state(&mut self, state_id: StateId) -> StatePtr {
        use std::iter;

        *self.state_cache.entry(state_id.clone()).or_insert_with({
            let state_ids = &mut self.state_ids;
            let next_states = &mut self.next_states;
            move || {
                let state_ptr = state_ids.len() as StatePtr;
                state_ids.push(state_id);
                next_states.extend(iter::repeat(UNKNOWN_STATE_PTR).take(257));
                state_ptr
            }
        })
    }
}

type StatePtr = u32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StateId {
    flags: Flags,
    bytes: Rc<[u8]>,
}

impl StateId {
    fn new(flags: Flags, instrs: &[InstrPtr]) -> Self {
        use makepad_varint::WriteVarint;

        let mut bytes = Vec::new();
        let mut prev_instr = 0;
        for &instr in instrs {
            let instr = instr as i32;
            bytes.write_vari32(instr - prev_instr).unwrap();
            prev_instr = instr;
        }
        Self {
            flags,
            bytes: Rc::from(bytes),
        }
    }

    fn instrs(&self) -> Instrs<'_> {
        Instrs {
            prev_instr: 0,
            bytes: &self.bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Flags(u8);

impl Flags {
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
        self.0 |= 1 << 1
    }
}

#[derive(Debug)]
struct Instrs<'a> {
    prev_instr: i32,
    bytes: &'a [u8],
}

impl<'a> Iterator for Instrs<'a> {
    type Item = InstrPtr;

    fn next(&mut self) -> Option<Self::Item> {
        use makepad_varint::ReadVarint;

        if self.bytes.is_empty() {
            return None;
        }
        let instr = self.prev_instr + (&mut self.bytes).read_vari32().unwrap();
        self.prev_instr = instr;
        Some(instr as InstrPtr)
    }
}

#[derive(Clone, Debug)]
struct Threads {
    instrs: SparseSet,
}

impl Threads {
    fn new(thread_count: usize) -> Self {
        Self {
            instrs: SparseSet::new(thread_count),
        }
    }

    fn add_thread(
        &mut self,
        instr: InstrPtr,
        preds: Preds,
        flags: &mut Flags,
        instrs: &[Instr],
        stack: &mut Vec<InstrPtr>,
    ) {
        stack.push(instr);
        while let Some(mut instr) = stack.pop() {
            loop {
                if !self.instrs.insert(instr) {
                    break;
                }
                match instrs[instr] {
                    Instr::Nop(next) | Instr::Save(_, next) => instr = next,
                    Instr::Assert(pred, next) => {
                        if match pred {
                            Pred::IsAtStartOfText => preds.is_at_start_of_text,
                            Pred::IsAtEndOfText => preds.is_at_end_of_text,
                        } {
                            instr = next;
                        } else {
                            flags.set_assert();
                        }
                    }
                    Instr::Split(next_0, next_1) => {
                        stack.push(next_1);
                        instr = next_0;
                    }
                    _ => break,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Preds {
    is_at_start_of_text: bool,
    is_at_end_of_text: bool,
}

impl Preds {
    fn to_bits(self) -> u8 {
        let mut bits = 0;
        bits |= (self.is_at_start_of_text as u8) << 0;
        bits |= (self.is_at_end_of_text as u8) << 1;
        bits
    }
}
