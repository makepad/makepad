use std::{
    collections::HashSet,
    sync::{Arc, Mutex, OnceLock},
};

pub trait Intern {
    fn intern(&self) -> Arc<str>;
}

impl Intern for str {
    fn intern(&self) -> Arc<str> {
        INTERNER
            .get_or_init(|| Mutex::new(Interner::new()))
            .lock()
            .unwrap()
            .intern(self)
    }
}

static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();

#[derive(Debug)]
struct Interner {
    cached_strings: HashSet<Arc<str>>,
}

impl Interner {
    fn new() -> Self {
        Self {
            cached_strings: HashSet::new(),
        }
    }

    fn intern(&mut self, string: &str) -> Arc<str> {
        if !self.cached_strings.contains(string) {
            self.cached_strings.insert(string.into());
        }
        self.cached_strings.get(string).unwrap().clone()
    }
}
