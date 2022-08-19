use {crate::{program::{InstrPtr}, Cursor, Program}, std::{collections::HashMap, rc::Rc}};

pub struct Dfa {
    state_ids: Vec<StateId>,
    next_states: Vec<StatePtr>,
    state_ptrs_by_state_id: HashMap<StateId, StatePtr>,
}

impl Dfa {
    pub(crate) fn new() -> Self {
        Self {
            state_ids: Vec::new(),
            next_states: Vec::new(),
            state_ptrs_by_state_id: HashMap::new(),
        }
    }
    
    pub(crate) fn run<C: Cursor>(
        &mut self,
        program: &Program,
        mut cursor: C
    ) {

    }

    fn get_or_create_state(&mut self, state_id: StateId) -> StatePtr {
        use std::iter;

        *self.state_ptrs_by_state_id.entry(state_id.clone()).or_insert_with({
            let state_ids = &mut self.state_ids;
            let next_states = &mut self.next_states;
            move || {
                let state_ptr = state_ids.len();
                state_ids.push(state_id);
                next_states.extend(iter::repeat(UNKNOWN_STATE_PTR).take(256));
                state_ptr
            }
        })
    }
}

type StatePtr = usize;

#[derive(Clone, Eq, Hash, PartialEq)]
struct StateId {
    bytes: Rc<[u8]>
}

impl StateId {
    fn instrs(&self) -> Instrs<'_> {
        Instrs {
            prev_instr: 0,
            bytes: &self.bytes
        }
    }
}

struct Instrs<'a> {
    prev_instr: InstrPtr,
    bytes: &'a [u8]
}

impl<'a> Iterator for Instrs<'a> {
    type Item = InstrPtr;

    fn next(&mut self) -> Option<Self::Item> {
        unimplemented!()
    }
}

const UNKNOWN_STATE_PTR: StatePtr = 1 << 31;