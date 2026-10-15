//! `thread_local!` (boot lane B25), expanded while parsing like the other built-ins.
//!
//! std's macro (library/std/src/thread/local.rs, `thread_local_process_attrs!`) turns each
//! `$(#[$attr])* $vis static $name: $t = [const] $init;` into
//! `$(#[$attr])* $vis const $name: LocalKey<$t> = thread_local_inner!(@key $t, [const] $init);`,
//! where the key is a value whose `with` reaches this thread's slot, initialised on first use
//! (`= expr`) or from the constant (`= const { .. }`).
//! Relax's std (std/thread/local.rs, coord.md F-OS2) keeps a lazily created OS key inside the
//! `LocalKey`, so the key must be one place: the expansion is a `static` holding
//! `LocalKey::new(init)`, with `init` a plain fn returning the initial value. Both forms initialise
//! on the thread's first access, which is observably the same for `const` initialisers (they have
//! no side effects).

use crate::lexer::T;

/// Paths by crate identity (resolve from any crate, std included).
const STD: &str = "::__relax_crate_std";

/// Expansion text for the tokens between `thread_local!`'s delimiters.
pub fn expand(toks: &[(T, &[u8])]) -> Result<String, String> {
    let text = |r: &[(T, &[u8])]| -> String {
        let mut s = String::new();
        for (_, t) in r {
            s.push_str(&String::from_utf8_lossy(t));
            s.push(' ');
        }
        s
    };
    let mut out = String::new();
    let mut i = 0;
    while i < toks.len() {
        // attributes and visibility, up to `static`
        let start = i;
        let mut depth = 0i32;
        while i < toks.len() && !(depth == 0 && toks[i].0 == T::KwStatic) {
            match toks[i].0 {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        if i >= toks.len() {
            if toks[start..].iter().all(|t| t.0 == T::Semi) {
                break;
            }
            return Err("expected `static NAME: Type = init;`".to_string());
        }
        let head = text(&toks[start..i]);
        if i + 2 >= toks.len() || toks[i + 1].0 != T::Ident || toks[i + 2].0 != T::Colon {
            return Err("expected `static NAME: Type = init;`".to_string());
        }
        let name = String::from_utf8_lossy(toks[i + 1].1).into_owned();
        // the type: up to `=` outside brackets and generic arguments
        // a joint `>=` / `>>=` closes the type's generic arguments and is the `=` (rustc's
        // macro matcher splits joint tokens: `static X:RefCell<Vec<u32>>=..` is valid)
        let mut j = i + 3;
        let mut depth = 0i32;
        let mut closes = "";
        while j < toks.len() {
            match toks[j].0 {
                T::Lt | T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::Gt | T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Shl => depth += 2,
                T::Shr => depth -= 2,
                T::Eq if depth == 0 => break,
                T::Ge if depth == 1 => { closes = ">"; break }
                T::ShrEq if depth == 2 => { closes = ">>"; break }
                _ => {}
            }
            j += 1;
        }
        if j >= toks.len() {
            return Err(format!("`{}`: expected `= init`", name));
        }
        let ty = text(&toks[i + 3..j]) + closes;
        let mut k = j + 1;
        if k < toks.len() && toks[k].0 == T::KwConst {
            k += 1;
        }
        let e0 = k;
        let mut depth = 0i32;
        while k < toks.len() {
            match toks[k].0 {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Semi if depth == 0 => break,
                _ => {}
            }
            k += 1;
        }
        let init = text(&toks[e0..k]);
        out.push_str(&format!(
            "{} static {}: {std}::thread::LocalKey<{}> = {std}::thread::LocalKey::new({{ fn __relax_thread_local_init() -> {} {{ {} }} __relax_thread_local_init }});\n",
            head,
            name,
            ty,
            ty,
            init,
            std = STD
        ));
        i = k + 1;
    }
    Ok(out)
}
