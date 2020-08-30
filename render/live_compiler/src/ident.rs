use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::Once;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Ident(usize);
impl Ident {
    pub fn new<'a, S>(string: S) -> Ident
    where
        S: Into<Cow<'a, str>>,
    {
        let string = string.into();
        Interner::with(|interner| {
            Ident(
                if let Some(index) = interner.indices.get(string.as_ref()).cloned() {
                    index
                } else {
                    let string = string.into_owned();
                    let string_index = interner.strings.len();
                    interner.strings.push(string.clone());
                    interner.indices.insert(string.clone(), string_index);
                    string_index
                },
            )
        })
    }

    pub fn with<F, R>(self, f: F) -> R
    where
        F: FnOnce(&str) -> R,
    {
        Interner::with(|interner| f(&interner.strings[self.0]))
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.with(|string| write!(f, "{}", string))
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.with(|string| write!(f, "{}", string))
    }
}

impl Ord for Ident {
    fn cmp(&self, other: &Ident) -> Ordering {
        Interner::with(|interner| interner.strings[self.0].cmp(&interner.strings[other.0]))
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Ident) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct Interner {
    strings: Vec<String>,
    indices: HashMap<String, usize>,
}

impl Interner {
    fn with<F, R>(f: F) -> R
    where
        F: FnOnce(&mut Interner) -> R,
    {
        static mut INTERNER: Option<Interner> = None;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            INTERNER = Some(Interner {
                strings: Vec::new(),
                indices: HashMap::new(),
            })
        });
        f(unsafe { INTERNER.as_mut().unwrap() })
    }
}
