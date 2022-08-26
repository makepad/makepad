use {
    crate::{compiler, dfa, Compiler, Cursor, Dfa, Parser, Program, StrCursor},
    std::{cell::RefCell, ops::Range, sync::Arc},
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
        Self {
            unique: Box::new(RefCell::new(Unique {
                dfa: Dfa::new(),
                reverse_dfa: Dfa::new(),
            })),
            shared: Arc::new(Shared {
                dfa_program,
                reverse_dfa_program,
            }),
        }
    }

    pub fn test(&self, string: &str) -> bool {
        self.test_with_cursor(StrCursor::new(string))
    }

    pub fn test_with_cursor<C: Cursor>(&self, cursor: C) -> bool {
        let mut unique = self.unique.borrow_mut();
        unique
            .dfa
            .run(
                &self.shared.dfa_program,
                cursor,
                dfa::Options {
                    stop_after_first_match: true,
                    ..dfa::Options::default()
                },
            )
            .is_some()
    }

    pub fn find(&self, string: &str) -> Option<Range<usize>> {
        self.find_with_cursor(StrCursor::new(string))
    }

    pub fn find_with_cursor<C: Cursor>(&self, mut cursor: C) -> Option<Range<usize>> {
        let mut unique = self.unique.borrow_mut();
        let end = unique.dfa.run(
            &self.shared.dfa_program,
            &mut cursor,
            dfa::Options::default(),
        )?;
        cursor.move_to(end);
        let start = unique.reverse_dfa.run(
            &self.shared.reverse_dfa_program,
            cursor.rev(),
            dfa::Options {
                continue_until_last_match: true,
                ..dfa::Options::default()
            },
        )?;
        Some(start..end)
    }
}

#[derive(Clone, Debug)]
struct Unique {
    dfa: Dfa,
    reverse_dfa: Dfa,
}

#[derive(Debug)]
struct Shared {
    dfa_program: Program,
    reverse_dfa_program: Program,
}
