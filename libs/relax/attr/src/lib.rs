//! `#[relax(O2)]` / `#[relax(O3)]` / `#[relax(O3, always)]` on a fn or an impl block: the
//! optimisation level Relax (makepad's JIT) compiles those functions at, ahead of their first call
//! on worker threads, so code that is hot before the first frame does not run at the cheapest
//! level (O1). `always`: a call made before the O3 code is ready waits for it. Relax reads the
//! attribute itself; under rustc it changes nothing: the item passes through as written.
//! `#[relax(keep_reachable)]`: a rare but required path (a GC, a growth path): a trace-shaken
//! export keeps the fn and everything it can call even when the trace never ran it.
extern crate proc_macro;
use proc_macro::{TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn relax(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args: Vec<TokenTree> = attr.into_iter().collect();
    match args.as_slice() {
        [TokenTree::Ident(level)] if matches!(level.to_string().as_str(), "O2" | "O3") => item,
        [TokenTree::Ident(keep)] if keep.to_string() == "keep_reachable" => item,
        [TokenTree::Ident(level), TokenTree::Punct(comma), TokenTree::Ident(always)]
            if level.to_string() == "O3" && comma.as_char() == ',' && always.to_string() == "always" => item,
        _ => {
            let mut out: TokenStream = "compile_error!(\"expected #[relax(O2)], #[relax(O3)], #[relax(O3, always)] or #[relax(keep_reachable)]\");".parse().unwrap();
            out.extend(item);
            out
        }
    }
}
