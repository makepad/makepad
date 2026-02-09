use {
    super::{
        compile, dfa,
        error::Result,
        input::{Cursor as _, Input},
        nfa, parse,
        prog::Prog,
    },
    std::{cell::RefCell, sync::Arc},
};

#[derive(Clone, Debug)]
pub struct Regex {
    shared: Arc<Shared>,
    dfa_cache: RefCell<dfa::Cache>,
    rev_dfa_cache: RefCell<dfa::Cache>,
    nfa_cache: RefCell<nfa::Allocs>,
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Self> {
        Self::new_with_options(
            pattern,
            parse::Options {
                multiline: true,
                dot_all: true,
                ..parse::Options::default()
            },
        )
    }

    pub fn new_with_options(pattern: &str, options: parse::Options) -> Result<Self> {
        let mut parse_cache = parse::Allocs::new();
        let ast = parse::parse(pattern, options, &mut parse_cache)?;
        let mut compile_cache = compile::Allocs::new();
        let dfa_prog = compile::compile(
            &ast,
            compile::Options {
                dot_star: true,
                ignore_caps: true,
                byte_based: true,
                ..compile::Options::default()
            },
            &mut compile_cache,
        );
        let rev_dfa_prog = compile::compile(
            &ast,
            compile::Options {
                ignore_caps: true,
                byte_based: true,
                reversed: true,
                ..compile::Options::default()
            },
            &mut compile_cache,
        );
        let nfa_prog = compile::compile(&ast, compile::Options::default(), &mut compile_cache);
        let dfa_cache = dfa::Cache::new(&dfa_prog);
        let rev_dfa_cache = dfa::Cache::new(&rev_dfa_prog);
        let nfa_cache = nfa::Allocs::new(&nfa_prog);
        Ok(Self {
            shared: Arc::new(Shared {
                dfa_prog,
                rev_dfa_prog,
                nfa_prog,
            }),
            dfa_cache: RefCell::new(dfa_cache),
            rev_dfa_cache: RefCell::new(rev_dfa_cache),
            nfa_cache: RefCell::new(nfa_cache),
        })
    }

    pub fn run<'a, I: Input<'a>>(&self, input: I, slots: &mut [Option<usize>]) -> bool {
        let mut dfa_cache = self.dfa_cache.borrow_mut();
        match dfa::run(
            &self.shared.dfa_prog,
            input.cursor_start(),
            dfa::Options {
                want_first_match: slots.is_empty(),
                ..dfa::Options::default()
            },
            &mut *dfa_cache,
        ) {
            Ok(Some(end)) => {
                if slots.is_empty() {
                    return true;
                }
                let mut rev_dfa_cache = self.rev_dfa_cache.borrow_mut();
                let start = dfa::run(
                    &self.shared.rev_dfa_prog,
                    input.slice(0..end).cursor_end().rev(),
                    dfa::Options {
                        want_last_match: true,
                        ..dfa::Options::default()
                    },
                    &mut *rev_dfa_cache,
                )
                .unwrap()
                .unwrap();
                if slots.len() == 2 {
                    slots[0] = Some(start);
                    slots[1] = Some(end);
                } else if slots.len() > 2 {
                    let mut nfa_cache = self.nfa_cache.borrow_mut();
                    nfa::run(
                        &self.shared.nfa_prog,
                        input.slice(start..end).cursor_start(),
                        nfa::Options::default(),
                        slots,
                        &mut *nfa_cache,
                    );
                    for slot in slots {
                        if let Some(slot) = slot {
                            *slot += start;
                        }
                    }
                }
                true
            }
            Ok(None) => false,
            Err(_) => {
                let mut nfa_cache = self.nfa_cache.borrow_mut();
                nfa::run(
                    &self.shared.nfa_prog,
                    input.cursor_start(),
                    nfa::Options {
                        want_first_match: slots.is_empty(),
                        ..nfa::Options::default()
                    },
                    slots,
                    &mut *nfa_cache,
                )
            }
        }
    }
}

#[derive(Debug)]
struct Shared {
    dfa_prog: Prog,
    rev_dfa_prog: Prog,
    nfa_prog: Prog,
}
