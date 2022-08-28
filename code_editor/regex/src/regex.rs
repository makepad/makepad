use {
    crate::{compiler, dfa, Compiler, Cursor, Dfa, Nfa, Parser, Program, StrCursor},
    std::{cell::RefCell, sync::Arc},
};

#[derive(Clone, Debug)]
pub struct Regex {
    unique: Box<RefCell<Unique>>,
    shared: Arc<Shared>,
}

impl Regex {
    pub fn new(pattern: &str) -> Self {
        let mut parser = Parser::new();
        let ast = parser.parse(pattern);
        let mut compiler = Compiler::new();
        let dfa_program = compiler.compile(
            &ast,
            compiler::Options {
                dot_star: true,
                bytes: true,
                ..compiler::Options::default()
            },
        );
        let reverse_dfa_program = compiler.compile(
            &ast,
            compiler::Options {
                dot_star: true,
                bytes: true,
                reverse: true,
                ..compiler::Options::default()
            },
        );
        let nfa_program = compiler.compile(
            &ast,
            compiler::Options::default()
        );
        Self {
            unique: Box::new(RefCell::new(Unique {
                dfa: Dfa::new(),
                reverse_dfa: Dfa::new(),
                nfa: Nfa::new(),
            })),
            shared: Arc::new(Shared {
                dfa_program,
                reverse_dfa_program,
                nfa_program,
            }),
        }
    }

    pub fn run(&self, string: &str, slots: &mut [Option<usize>]) -> bool {
        self.run_with_cursor(StrCursor::new(string), slots)
    }

    pub fn run_with_cursor<C: Cursor>(&self, mut cursor: C, slots: &mut [Option<usize>]) -> bool {
        let mut unique = self.unique.borrow_mut();
        let end = match unique.dfa.run(
            &self.shared.dfa_program,
            &mut cursor,
            dfa::Options {
                stop_after_first_match: slots.is_empty(),
                ..dfa::Options::default()
            }
        ) {
            Some(end) => end,
            None => return false,
        };
        cursor.move_to(end);
        let start = unique.reverse_dfa.run(
            &self.shared.reverse_dfa_program,
            (&mut cursor).rev(),
            dfa::Options {
                continue_until_last_match: true,
                ..dfa::Options::default()
            },
        ).unwrap();
        cursor.move_to(start);
        if slots.len() == 2 {
            slots[0] = Some(start);
            slots[1] = Some(end);
        } else {
            unique.nfa.run(&self.shared.nfa_program, cursor, slots);
        }
        true
    }
}

#[derive(Clone, Debug)]
struct Unique {
    dfa: Dfa,
    reverse_dfa: Dfa,
    nfa: Nfa,
}

#[derive(Debug)]
struct Shared {
    dfa_program: Program,
    reverse_dfa_program: Program,
    nfa_program: Program,
}
