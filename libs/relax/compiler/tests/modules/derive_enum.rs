// A sandboxed module derives the comparison and hash traits on its enums. The front's builtin
// derives read an enum's discriminant in an `unsafe` block of their own writing; the module's
// source check passes that text and nothing else: an `unsafe` the module wrote beside it, in the
// same macro expansion as a derived enum, is refused as before.
use std::relax::Module;

const SOURCE: &str = r#"
use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Suit { Clubs, Diamonds, Hearts, Spades }

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Token { Word(String), Number { value: u64, digits: u8 }, End }

macro_rules! flags {
    ($name:ident { $($v:ident),* }) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($v),* }
    };
}
flags!(Mode { Read, Write, Both });

fn hash_of<T: Hash>(x: &T) -> u64 {
    let mut h = DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

pub fn run() -> u64 {
    let mut n = 0u64;
    n += (Suit::Hearts == Suit::Hearts) as u64;
    n += (Suit::Clubs != Suit::Spades) as u64 * 2;
    n += (Suit::Diamonds < Suit::Hearts) as u64 * 4;
    n += (Token::Word("a".to_string()) < Token::Number { value: 1, digits: 1 }) as u64 * 8;
    n += (Token::Number { value: 7, digits: 1 } == Token::Number { value: 7, digits: 1 }) as u64 * 16;
    n += (Token::End != Token::Word(String::new())) as u64 * 32;
    n += (hash_of(&Suit::Clubs) != hash_of(&Suit::Spades)) as u64 * 64;
    n += (hash_of(&Token::End) == hash_of(&Token::End)) as u64 * 128;
    n += (Mode::Read != Mode::Both && Mode::Write == Mode::Write) as u64 * 256;
    let set: BTreeSet<Suit> = [Suit::Spades, Suit::Clubs, Suit::Spades, Suit::Hearts].into_iter().collect();
    n += set.len() as u64 * 1000 + (set.iter().next() == Some(&Suit::Clubs)) as u64 * 512;
    n
}
"#;

// the same call site makes a derived enum and an `unsafe` block of the module's own
const HIDDEN: &str = r#"
macro_rules! Both {
    () => {
        #[derive(PartialEq)]
        pub enum Both { A, B }
        pub fn peek(x: &u64) -> u64 { unsafe { *(x as *const u64) } }
    };
}
Both!();
pub fn run() -> u64 { peek(&1) }
"#;

#[test]
fn derive_enum() {
    match Module::load_source("cards", "cards.rs", SOURCE, &[]) {
        Ok(m) => {
            let run: fn() -> u64 = m.get("run").unwrap();
            println!("out derived traits on enums: {}", run());
            m.unload();
        }
        Err(e) => println!("out derived traits on enums: refused: {}", e.lines().next().unwrap_or("")),
    }
    let hidden = Module::load_source("hidden", "hidden.rs", HIDDEN, &[]);
    println!("out the module's own unsafe beside a derive: {}", match hidden {
        Ok(_) => "LOADED".to_string(),
        Err(e) => e.lines().filter(|l| l.contains("`unsafe`")).count().to_string() + " refusal(s)",
    });
}
