use proc_macro::{Delimiter, TokenStream, TokenTree};

/// `#[derive(Shader)] #[repr(C)] struct Name { #[deref] base: Ty, .. }`: the shape of makepad's
/// DrawShader derive (platform/script/derive_impl/src/rr.rs). Every shader type also gets a
/// macro under its own name (macro namespace), its ancestry: a derived type invokes its base's
/// ancestry macro through `use Base as __anc_base_..`, which calls back into the runtime's
/// `chain!`, which defines this type's ancestry macro with one more depth level.
#[proc_macro_derive(Shader, attributes(deref))]
pub fn derive_shader(input: TokenStream) -> TokenStream {
    let toks: Vec<TokenTree> = input.into_iter().collect();
    let mut name = String::new();
    let mut base: Option<(String, String)> = None;
    let mut i = 0;
    while i < toks.len() {
        if let TokenTree::Ident(id) = &toks[i] {
            if id.to_string() == "struct" {
                name = toks[i + 1].to_string();
                if let TokenTree::Group(g) = &toks[i + 2] {
                    if g.delimiter() == Delimiter::Brace {
                        let body: Vec<TokenTree> = g.stream().into_iter().collect();
                        let mut j = 0;
                        while j < body.len() {
                            // `# [deref] [pub] field : Ty ,`
                            if body[j].to_string() == "#" && body.get(j + 1).map_or(false, |t| t.to_string() == "[deref]") {
                                let mut fi = j + 2;
                                if body[fi].to_string() == "pub" {
                                    fi += 1;
                                }
                                let f = body[fi].to_string();
                                let mut ty = String::new();
                                let mut k = fi + 2;
                                while k < body.len() && body[k].to_string() != "," {
                                    ty.push_str(&body[k].to_string());
                                    k += 1;
                                }
                                base = Some((f, ty));
                                j = k;
                            }
                            j += 1;
                        }
                    }
                }
                break;
            }
        }
        i += 1;
    }
    let lname = name.to_ascii_lowercase();
    let h = name.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let rt = format!("__anc_rt_{lname}_{h:x}");
    let module = format!("__anc_mod_{lname}_{h:x}");
    let meta = format!("__anc_meta_{name}_{h:x}");
    let mut out = format!("use anc_draw::rr as {rt};");
    match base {
        Some((f, ty)) => {
            let alias = format!("__anc_base_{lname}_{h:x}");
            out.push_str(&format!(
                "impl anc_draw::rr::Layout for {name} {{ type Base = {ty}; fn base(&self) -> &{ty} {{ &self.{f} }} }}
                 use {ty} as {alias};
                 mod {module} {{
                     super::{alias}!(super::{rt}::chain, super::{name}, {meta}, $);
                     pub use {meta} as ancestry;
                 }}"
            ));
        }
        None => out.push_str(&format!(
            "mod {module} {{ super::{rt}::meta!({meta}, $, [()]); pub use {meta} as ancestry; }}"
        )),
    }
    out.push_str(&format!("pub use self::{module}::ancestry as {name};"));
    out.parse().unwrap()
}
