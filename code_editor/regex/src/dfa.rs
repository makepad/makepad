use {
    crate::{
        program::{Instr, InstrPtr},
        Cursor, Program, SparseSet,
    },
    std::{collections::HashMap, rc::Rc},
};

#[derive(Clone, Debug)]
pub struct Dfa {
    start_state_cache: Option<StatePtr>,
    states: States,
    current_threads: Threads,
    next_threads: Threads,
    stack: Vec<InstrPtr>,
}

impl Dfa {
    pub(crate) fn new() -> Self {
        Self {
            start_state_cache: None,
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

    pub(crate) fn run<C: Cursor>(&mut self, program: &Program, cursor: C) -> Option<usize> {
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
        }
        .run()
    }
}

pub struct RunContext<'a, C> {
    start_state_cache: &'a mut Option<StatePtr>,
    states: &'a mut States,
    current_threads: &'a mut Threads,
    next_threads: &'a mut Threads,
    stack: &'a mut Vec<InstrPtr>,
    program: &'a Program,
    cursor: C,
}

impl<'a, C: Cursor> RunContext<'a, C> {
    fn run(&mut self) -> Option<usize> {
        let mut matched = None;
        let mut current_state = UNKNOWN_STATE_PTR;
        let mut next_state = self.get_or_create_start_state();
        let mut byte = self.cursor.current_byte();
        while !self.cursor.is_at_back() {
            while next_state <= MAX_STATE_PTR && !self.cursor.is_at_back() {
                self.cursor.move_next_byte();
                current_state = next_state;
                next_state = *self.states.next_state(current_state, byte);
                byte = self.cursor.current_byte();
            }
            if next_state & MATCH_STATE_FLAG != 0 {
                matched = Some(self.cursor.byte_position() - 1);
                next_state &= !MATCH_STATE_FLAG;
            } else if next_state == UNKNOWN_STATE_PTR {
                self.cursor.move_prev_byte();
                let byte = self.cursor.current_byte();
                self.cursor.move_next_byte();
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
        if next_state & MATCH_STATE_FLAG != 0 {
            matched = Some(self.cursor.byte_position());
        }
        matched
    }

    fn get_or_create_start_state(&mut self) -> StatePtr {
        *self.start_state_cache.get_or_insert_with({
            let states = &mut self.states;
            let current_threads = &mut self.current_threads;
            let stack = &mut self.stack;
            let program = &self.program;
            move || {
                current_threads.add_thread(program.start, &program.instrs, stack);
                let state_id = StateId::new(current_threads.instrs.as_slice());
                current_threads.instrs.clear();
                states.get_or_create_state(state_id)
            }
        })
    }

    fn get_or_create_next_state(&mut self, state: StatePtr, byte: Option<u8>) -> StatePtr {
        for instr in self.states.state_ids[state as usize].instrs() {
            self.current_threads.instrs.insert(instr);
        }
        let mut matched = false;
        for &instr in self.current_threads.instrs.as_slice() {
            match self.program.instrs[instr] {
                Instr::Match => {
                    matched = true;
                    break;
                }
                Instr::ByteRange(byte_range, next) => {
                    if byte.map_or(false, |byte| byte_range.contains(&byte)) {
                        self.next_threads
                            .add_thread(next, &self.program.instrs, &mut self.stack);
                    }
                }
                _ => {}
            }
        }
        self.current_threads.instrs.clear();
        if !matched && self.next_threads.instrs.is_empty() {
            return DEAD_STATE_PTR;
        }
        let state_id = StateId::new(self.next_threads.instrs.as_slice());
        self.next_threads.instrs.clear();
        let mut next_state = self.states.get_or_create_state(state_id);
        if matched {
            next_state |= MATCH_STATE_FLAG;
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
    bytes: Rc<[u8]>,
}

impl StateId {
    fn new(instrs: &[InstrPtr]) -> Self {
        use makepad_varint::WriteVarint;

        let mut bytes = Vec::new();
        let mut prev_instr = 0;
        for &instr in instrs {
            let instr = instr as i32;
            bytes.write_vari32(instr - prev_instr).unwrap();
            prev_instr = instr;
        }
        Self {
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

    fn add_thread(&mut self, instr: InstrPtr, instrs: &[Instr], stack: &mut Vec<InstrPtr>) {
        stack.push(instr);
        while let Some(mut instr) = stack.pop() {
            loop {
                if self.instrs.contains(instr) {
                    break;
                }
                self.instrs.insert(instr);
                match instrs[instr] {
                    Instr::Save(_, next) => instr = next,
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

const MAX_STATE_PTR: StatePtr = (1 << 30) - 1;
const MATCH_STATE_FLAG: StatePtr = 1 << 30;
const UNKNOWN_STATE_PTR: StatePtr = 1 << 31;
const DEAD_STATE_PTR: StatePtr = (1 << 31) + 1;
