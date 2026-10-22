//! `thread_local!` (boot lane B25), expanded while parsing like the other built-ins.
//!
//! std's macro (library/std/src/thread/local.rs, `thread_local_process_attrs!`) turns each
//! `$(#[$attr])* $vis static $name: $t = [const] $init;` into
//! `$(#[$attr])* $vis const $name: LocalKey<$t> = thread_local_inner!(@key $t, [const] $init);`,
//! where the key is a value whose `with` reaches this thread's slot, initialised on first use
//! (`= expr`) or from the constant (`= const { .. }`).
//! Relax's expansion has the same shape: a `const` key whose fn returns the thread's value. A
//! `const` initialiser of a type without drop glue is a `#[thread_local]` static, as std's
//! (the compiler places it at a fixed offset from the thread pointer where it can); every other
//! key keeps a lazily created runtime key (std/thread/local.rs `LazyKey`) in a `static` of its fn,
//! initialised on the thread's first access, which is observably the same for `const`
//! initialisers (they have no side effects).

use crate::lexer::T;
use crate::tg;
use crate::tokbuf::TokBuf;

/// The tokens between `thread_local!`'s delimiters, expanded.
pub fn expand(toks: &[(T, &[u8])], edition: u16) -> Result<TokBuf, String> {
    let copy = |g: &mut TokBuf, r: &[(T, &[u8])]| {
        for (k, t) in r {
            g.put(*k, &String::from_utf8_lossy(t));
        }
    };
    let mut out = TokBuf::new(edition);
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
        if i + 2 >= toks.len() || toks[i + 1].0 != T::Ident || toks[i + 2].0 != T::Colon {
            return Err("expected `static NAME: Type = init;`".to_string());
        }
        let name = String::from_utf8_lossy(toks[i + 1].1).into_owned();
        // the type: up to `=` outside brackets and generic arguments
        // a joint `>=` / `>>=` closes the type's generic arguments and is the `=` (rustc's
        // macro matcher splits joint tokens: `static X:RefCell<Vec<u32>>=..` is valid)
        let mut j = i + 3;
        let mut depth = 0i32;
        let mut closes = 0;
        while j < toks.len() {
            match toks[j].0 {
                T::Lt | T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::Gt | T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Shl => depth += 2,
                T::Shr => depth -= 2,
                T::Eq if depth == 0 => break,
                T::Ge if depth == 1 => { closes = 1; break }
                T::ShrEq if depth == 2 => { closes = 2; break }
                _ => {}
            }
            j += 1;
        }
        if j >= toks.len() {
            return Err(format!("`{}`: expected `= init`", name));
        }
        let mut ty = TokBuf::new(edition);
        copy(&mut ty, &toks[i + 3..j]);
        for _ in 0..closes {
            ty.punct(T::Gt);
        }
        let mut k = j + 1;
        let konst = k < toks.len() && toks[k].0 == T::KwConst;
        if konst {
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
        let mut init = TokBuf::new(edition);
        copy(&mut init, &toks[e0..k]);
        copy(&mut out, &toks[start..i]);
        // (several short `tg!`s: one long template exceeded Relax's own macro expansion when this
        // file runs under the JIT -- see TAILTLS.REPORT.md)
        // `Option<*const T>`: what the key's fn returns
        let mut ret = TokBuf::new(edition);
        tg!(&mut ret; #std() :: option :: Option < * const #append(&ty) >);
        let mut lazy_key = TokBuf::new(edition);
        tg!(&mut lazy_key; static __RELAX_TLS_KEY : #std() :: thread :: LazyKey < #append(&ty) > = #std() :: thread :: LazyKey :: new () ;);
        tg!(&mut out; const #word(&name) : #std() :: thread :: LocalKey < #append(&ty) > =);
        let mut body = TokBuf::new(edition);
        if konst {
            // std: `const` + no drop glue is the value itself in a `#[thread_local]` static; with
            // drop glue (EagerStorage there) a runtime key whose value starts as the constant
            tg!(&mut body; const __RELAX_TLS_INIT : #append(&ty) = #append(&init) ;);
            let mut keyed = TokBuf::new(edition);
            tg!(&mut keyed; fn __relax_tls_init () -> #append(&ty) { __RELAX_TLS_INIT });
            keyed.append(&lazy_key);
            tg!(&mut keyed; __RELAX_TLS_KEY . get ( __relax_tls_init ));
            let mut plain = TokBuf::new(edition);
            tg!(&mut plain; #[thread_local] static __RELAX_TLS_VAL : #append(&ty) = __RELAX_TLS_INIT ;);
            tg!(&mut plain; #std() :: option :: Option :: Some ( & raw const __RELAX_TLS_VAL ));
            tg!(&mut body; fn __relax_tls_get () -> #append(&ret) {
                if #std() :: mem :: needs_drop :: < #append(&ty) > () { #append(&keyed) } else { #append(&plain) }
            });
        } else {
            tg!(&mut body; fn __relax_tls_init () -> #append(&ty) { #append(&init) });
            let mut get = TokBuf::new(edition);
            get.append(&lazy_key);
            tg!(&mut get; __RELAX_TLS_KEY . get ( __relax_tls_init ));
            tg!(&mut body; fn __relax_tls_get () -> #append(&ret) { #append(&get) });
        }
        tg!(&mut body; #std() :: thread :: LocalKey :: new ( __relax_tls_get ));
        tg!(&mut out; { #append(&body) } ;);
        i = k + 1;
    }
    Ok(out)
}
