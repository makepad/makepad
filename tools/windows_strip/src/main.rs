use std::fs;
use makepad_rust_tokenizer::{Cursor, State, FullToken, Delim, LiveId, live_id, id};
use std::{
    ops::Deref,
    ops::DerefMut,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TokenWithString {
    pub token: FullToken,
    pub value: String
}

impl Deref for TokenWithString {
    type Target = FullToken;
    fn deref(&self) -> &Self::Target {&self.token}
}

impl DerefMut for TokenWithString {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.token}
}

pub trait TokenSliceApi {
    fn find_tokens_index(&self, tokens: &[TokenWithString]) -> Option<usize>;
    fn find_str_index(&self, what: &str) -> Option<usize>;
    fn after(&self, what: &str) -> Option<&[TokenWithString]>;
    fn at(&self, what: &str) -> Option<(&[TokenWithString], &[TokenWithString])>;
    fn find_close(&self, delim: Delim) -> Option<&[TokenWithString]>;
    fn find_token(&self, token: FullToken) -> Option<&[TokenWithString]>;
    fn find_strs_rev(&self, what: &[Vec<TokenWithString>]) -> Option<usize>;
    fn parse_use(&self) -> Vec<Vec<LiveId >>;
    fn to_string(&self) -> String;
}

impl<T> TokenSliceApi for T where T: AsRef<[TokenWithString]> {
    fn to_string(&self) -> String {
        let mut out = String::new();
        for token in self.as_ref() {
            out.push_str(&token.value);
        }
        out
    }
    
    fn find_tokens_index(&self, what: &[TokenWithString]) -> Option<usize> {
        let source = self.as_ref();
        let mut depth = 0;
        for i in 0..source.len() {
            if source[i].is_open() {
                depth += 1;
            }
            else if source[i].is_close() {
                if depth == 0 { // unexpected end
                    panic!()
                }
                depth -= 1;
            }
            if depth == 0{
                for j in 0..what.len() {
                    if source[i + j].token != what[j].token {
                        break;
                    }
                    if j == what.len() - 1 {
                        return Some(i)
                    }
                }
            }
        }
        None
    }
    
    fn find_strs_rev(&self, what: &[Vec<TokenWithString>]) -> Option<usize> {
        let source = self.as_ref();
        for i in (0..source.len()).rev() {
            for (index, what) in what.iter().enumerate() {
                if what.len() <= source.len() - i {
                    for j in 0..what.len() {
                        if source[i + j].token != what[j].token {
                            break;
                        }
                        if j == what.len() - 1 {
                            return Some(index)
                        }
                    }
                }
            }
        }
        None
    }
    
    fn find_str_index(&self, what: &str) -> Option<usize> {
        self.find_tokens_index(&parse_to_tokens(what))
    }
    
    fn after(&self, what: &str) -> Option<&[TokenWithString]> {
        let source = self.as_ref();
        let what = &parse_to_tokens(what);
        if let Some(pos) = source.find_tokens_index(what) {
            return Some(&source[pos + what.len()..])
        }
        None
    }
    
    fn at(&self, what: &str) -> Option<(&[TokenWithString], &[TokenWithString])> {
        let source = self.as_ref();
        let what = &parse_to_tokens(what);
        if let Some(pos) = source.find_tokens_index(what) {
            return Some((&source[..pos], &source[pos..]))
        }
        None
    }
    
    fn find_close(&self, delim: Delim) -> Option<&[TokenWithString]> {
        let source = self.as_ref();
        let mut depth = 0;
        for i in 0..source.len() {
            if source[i].is_open_delim(delim) {
                depth += 1;
            }
            else if source[i].is_close_delim(delim) {
                if depth == 0 { // unexpected end
                    panic!()
                }
                depth -= 1;
                if depth == 0 {
                    return Some(&source[0..i + 1])
                }
            }
        }
        None
    }
    
    fn find_token(&self, token: FullToken) -> Option<&[TokenWithString]> {
        let source = self.as_ref();
        let mut depth = 0;
        for i in 0..source.len() {
            if source[i].is_open() {
                depth += 1;
            }
            else if source[i].is_close() {
                if depth == 0 { // unexpected end
                    panic!()
                }
                depth -= 1;
            }
            else if depth == 0 && source[i].token == token {
                return Some(&source[0..i + 1])
            }
        }
        None
    }
    
    fn parse_use(&self) -> Vec<Vec<LiveId >> {
        // fetch use { }
        let after_use = self.after("use").unwrap();
        let source = after_use.find_close(Delim::Brace).unwrap();
        
        // now we have to flatten the use tree
        let mut stack = Vec::new();
        let mut ident = Vec::new();
        let mut deps = Vec::new();
        for i in 0..source.len() {
            match source[i].token {
                FullToken::Ident(id) => {
                    ident.push(id);
                }
                FullToken::Punct(live_id!(::)) => {}
                FullToken::Punct(live_id!(,)) => {
                    let len = *stack.last().unwrap();
                    if ident.len()>len {
                        deps.push(ident.clone());
                    }
                    ident.truncate(len);
                }
                FullToken::Open(Delim::Brace) => {
                    stack.push(ident.len());
                }
                FullToken::Close(Delim::Brace) => {
                    let len = stack.pop().unwrap();
                    if ident.len()>len {
                        deps.push(ident.clone());
                        ident.truncate(*stack.last().unwrap());
                    }
                }
                _ => {
                    // unexpected
                }
            }
        }
        // we should parse all our use things into a fully qualified list.
        deps
    }
    
}

fn parse_to_tokens(source: &str) -> Vec<TokenWithString> {
    let mut tokens = Vec::new();
    let mut total_chars = Vec::new();
    let mut state = State::default();
    let mut scratch = String::new();
    let mut last_token_start = 0;
    for line_str in source.lines() {
        let start = total_chars.len();
        total_chars.extend(line_str.chars());
        let mut cursor = Cursor::new(&total_chars[start..], &mut scratch);
        loop {
            let (next_state, full_token) = state.next(&mut cursor);
            if let Some(full_token) = full_token {
                let next_token_start = last_token_start + full_token.len;
                let value: String = total_chars[last_token_start..next_token_start].into_iter().collect();
                if !full_token.is_ws_or_comment() {
                    tokens.push(TokenWithString {
                        token: full_token.token,
                        value
                    });
                }
                else {
                    if let Some(last) = tokens.last_mut() {
                        last.value.push_str(&value);
                    }
                }
                last_token_start = next_token_start;
            }
            else {
                break;
            }
            state = next_state;
        }
        if let Some(last) = tokens.last_mut() {
            last.value.push_str("\n");
        }
    }
    tokens
}

fn parse_file<'a>(file: &str, cache: &'a mut Vec<(String, Vec<TokenWithString>)>) -> Result<&'a [TokenWithString],
Box<dyn std::error::Error >> {
    if let Some(index) = cache.iter().position( | v | v.0 == file) {
        return Ok(&cache[index].1)
    }
    else {
        let source = fs::read_to_string(file) ?;
        let source = parse_to_tokens(&source);
        cache.push((file.to_string(), source));
        Ok(&cache.last().unwrap().1)
    }
}

fn filter_symbols(inp: Vec<Vec<LiveId >>, filter: &[LiveId]) -> Vec<Vec<LiveId >> {
    let mut out = Vec::new();
    'outer: for sym in inp {
        if sym.len() >= filter.len() {
            for i in 0..filter.len() {
                if sym[i] != filter[i] {
                    continue 'outer;
                }
            }
            out.push(sym[filter.len()..sym.len()].to_vec());
        }
    }
    out
}

enum Node {
    Sub(Vec<(LiveId, Node)>),
    Value(String)
}

fn push_unique(output: &mut Node, what: &[LiveId], value: String) {
    if what.len() == 1 {
        // terminator node
        if let Node::Sub(vec) = output {
            if vec.iter_mut().find( | v | v.0 == what[0]).is_none() {
                vec.push((what[0], Node::Value(value)));
            }
        }
        else {
            panic!();
        }
    }
    else {
        if let Node::Sub(vec) = output {
            if let Some(child) = vec.iter_mut().find( | v | v.0 == what[0]) {
                return push_unique(&mut child.1, &what[1..], value);
            }
            let mut child = Node::Sub(Vec::new());
            push_unique(&mut child, &what[1..], value);
            vec.push((what[0], child));
        }
        else {
            panic!();
        }
    }
}

fn remove_node(output: &mut Node, what: &[LiveId]) {
    if what.len() == 1 {
        if let Node::Sub(vec) = output {
            vec.retain( | v | v.0 != what[0]);
        }
        else{
            panic!();
        }
    }
    else {
        if let Node::Sub(vec) = output {
            if let Some(child) = vec.iter_mut().find( | v | v.0 == what[0]) {
                return remove_node(&mut child.1, &what[1..]);
            }
            return
        }
        else {
            panic!();
        }
    }
}

fn generate_outputs_from_file( file: &str, output: &mut Node, cache: &mut Vec<(String, Vec<TokenWithString>)>) {
    
    println!("processing {}",file);

    let source = parse_file(file, cache).unwrap();
    let symbols = source.parse_use();
    let symbols = filter_symbols(symbols, id!(crate.windows));
    

    fn add_impl(out: &mut String, input: &[TokenWithString], at: String,) -> bool {
        if let Some((_, is_impl)) = input.at(&at) {
            add_impl(out, &is_impl[1..], at);
            let is_impl = is_impl.find_close(Delim::Brace).unwrap();
            out.push_str(&is_impl.to_string());
            true
        }
        else {
            false
        }
    }
    
    
    let prefixes_str = ["#[repr(C)]", "#[repr(C, packed(1))]", "#[repr(transparent)]"];
    let prefixes_tok = [parse_to_tokens(prefixes_str[0]), parse_to_tokens(prefixes_str[1]), parse_to_tokens(prefixes_str[2])];
    
    for sym in symbols {
        if sym.len() == 0 || sym[0] == live_id!(core){
            continue;
        }
        // allright lets open the module
        let mut path = format!("./tools/windows_strip/windows/src/Windows/");
        // ok so everything is going to go into the module Win32
        // but how do we sort the substructure
        for i in 0..sym.len() - 1 {
            path.push_str(&format!("/{}", sym[i]));
        }
        
        let mod_tokens = parse_file(&format!("{}/mod.rs", path), cache).expect(&format!("{}", path));
        
        let sym_id = sym[sym.len() - 1];
        
        if let Some((_, is_fn)) = mod_tokens.at(&format!("pub unsafe fn {}", sym_id)) {
            let is_fun = is_fn.find_close(Delim::Brace).unwrap();
            //  ok so how do we do this
            push_unique(output, &sym, is_fun.to_string());
        }
        /*if let Some((_, is_fn)) = mod_tokens.at(&format!("pub fn {}", sym_id)) {
            let is_fun = is_fn.find_close(Delim::Brace).unwrap();
            //  ok so how do we do this
            push_unique(output, &sym, is_fun.to_string());
        }*/
        else if let Some((_, is_const)) = mod_tokens.at(&format!("pub const {}", sym_id)) {
            let is_const = is_const.find_token(FullToken::Punct(live_id!(;))).unwrap();
            push_unique(output, &sym, is_const.to_string());
        }
        else if let Some((_, is_type)) = mod_tokens.at(&format!("pub type {}", sym_id)) {
            let is_type = is_type.find_token(FullToken::Punct(live_id!(;))).unwrap();
            push_unique(output, &sym, is_type.to_string());
        }
        else if let Some((pre_union, is_union)) = mod_tokens.at(&format!("pub union {}", sym_id)) {
            let is_union = is_union.find_close(Delim::Brace).unwrap();
            let pre = if let Some(pre) = pre_union.find_strs_rev(&prefixes_tok) {prefixes_str[pre]}else {""};
            
            let mut out = String::new();
            out.push_str(pre);
            out.push_str(&is_union.to_string());
            
            add_impl(&mut out, mod_tokens, format!("impl ::core::marker::Copy for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::cmp::Eq for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::cmp::PartialEq for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::clone::Clone for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::default::Default for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows::core::Abi for {}", sym_id));
            push_unique(output, &sym, out);
        }
        else if let Some((pre_struct, is_struct)) = mod_tokens.at(&format!("struct {}", sym_id)) {
            let mut out = String::new();
            let is_struct = if let FullToken::Open(Delim::Paren) = is_struct[2].token {
                is_struct.find_token(FullToken::Punct(live_id!(;))).unwrap()
            }
            else {
                is_struct.find_close(Delim::Brace).unwrap()
            };
            
            let pre = if let Some(pre) = pre_struct.find_strs_rev(&prefixes_tok) {prefixes_str[pre]}else {""};
            
            out.push_str(&pre);
            out.push_str("pub ");
            out.push_str(&is_struct.to_string());
            
            add_impl(&mut out, mod_tokens, format!("impl {}", sym_id));
            
            add_impl(&mut out, mod_tokens, format!("impl ::core::marker::Copy for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::cmp::Eq for {}", sym_id));
            if !add_impl(&mut out, mod_tokens, format!("impl ::core::cmp::PartialEq for {}", sym_id)) {
                if let FullToken::Open(Delim::Paren) = is_struct[2].token {
                    out.insert_str(0, "#[derive(PartialEq, Eq)]")
                }
            }
            add_impl(&mut out, mod_tokens, format!("impl ::core::clone::Clone for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::default::Default for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows::core::Abi for {}", sym_id));
            
            add_impl(&mut out, mod_tokens, format!("impl ::core::fmt::Debug for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::ops::BitOr for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::ops::BitAnd for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::ops::BitOrAssign for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::ops::BitAndAssign for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::ops::Not for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl::core::convert::From<::core::option::Option<{}>> for {}", sym_id, sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::convert::TryFrom<{}> for", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::convert::TryFrom<&{}> for", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::windows_core::CanInto<{}> for", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::convert::From<{}> for", sym_id));

            add_impl(&mut out, mod_tokens, format!("impl ::core::iter::IntoIterator for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::core::iter::IntoIterator for &{}", sym_id));

            add_impl(&mut out, mod_tokens, format!("unsafe impl ::core::marker::Send for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::core::marker::Sync for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows_core::Vtable for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::windows_core::TypeKind for {}", sym_id));

            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static> ::core::marker::Send for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static> ::core::marker::Sync for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static> ::windows_core::Interface for {}", sym_id));

            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static, TProgress: ::windows::core::RuntimeType + 'static> ::core::marker::Send for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static, TProgress: ::windows::core::RuntimeType + 'static> ::core::marker::Sync for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl<TResult: ::windows_core::RuntimeType + 'static, TProgress: ::windows::core::RuntimeType + 'static> ::windows::core::Vtable for {}", sym_id));
            
            add_impl(&mut out, mod_tokens, format!("impl<K: ::windows_core::RuntimeType + 'static, V: ::windows_core::RuntimeType + 'static> ::windows_core::CanInto<::windows_core::IUnknown> for {}", sym_id));
             
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows_core::Interface for {}", sym_id)); 
            add_impl(&mut out, mod_tokens, format!("impl ::windows_core::RuntimeName for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows_core::RuntimeType for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("impl ::windows_core::RuntimeType for {}", sym_id));
            add_impl(&mut out, mod_tokens, format!("unsafe impl ::windows_core::ComInterface for {}", sym_id));
            
            push_unique(output, &sym, out);
        } 
        
        if let Some((_, is_com)) = mod_tokens.at(&format!("pub struct {}_Vtbl", sym_id)) {
            
            let mut sym = sym.clone();
            let sym_end = sym.len() - 1;
            
            if let Some((_, is_hier)) = mod_tokens.at(&format!("::windows_core::imp::interface_hierarchy!({}", sym_id)) { 
                let is_hier = is_hier.find_token(FullToken::Punct(live_id!(;))).unwrap();
                sym[sym_end] = LiveId::from_str_with_lut(&format!("{}_hierarchy", sym_id)).unwrap();
                push_unique(output, &sym, is_hier.to_string());
            }
            
            let is_com = is_com.find_close(Delim::Brace).unwrap();
            sym[sym_end] = LiveId::from_str_with_lut(&format!("{}_Vtbl", sym_id)).unwrap();
            push_unique(output, &sym, format!("#[repr(C)]\n{}", is_com.to_string()));
            
            // fetch impl tokens
            
            let impl_tokens = parse_file(&format!("{}/impl.rs", path), cache).unwrap();
 
            if let Some((_, is_trait)) = impl_tokens.at(&format!("pub trait {}_Impl", sym_id)){
                let is_trait = is_trait.find_close(Delim::Brace).unwrap();
                sym[sym_end] = LiveId::from_str_with_lut(&format!("{}_Impl", sym_id)).unwrap();
                push_unique(output, &sym, is_trait.to_string());
            }
            if let Some((_, is_runtime_name)) = impl_tokens.at(&format!("impl ::windows_core::RuntimeName for {}", sym_id)){
                let is_runtime_name = is_runtime_name.find_close(Delim::Brace).unwrap();
                sym[sym_end] = LiveId::from_str_with_lut(&format!("{}_RuntimeName", sym_id)).unwrap();
                push_unique(output, &sym, is_runtime_name.to_string());
            }
            
            if let Some((_, is_impl)) = impl_tokens.at(&format!("impl {}_Vtbl", sym_id)){
                let is_impl = is_impl.find_close(Delim::Brace).unwrap();
                sym[sym_end] = LiveId::from_str_with_lut(&format!("{}_Vtbl2", sym_id)).unwrap();
                push_unique(output, &sym, is_impl.to_string());
            }
            
        }
    }
}

fn main() {
    let mut output = Node::Sub(Vec::new());
    let mut cache = Vec::new();
    generate_outputs_from_file("./platform/src/os/windows/win32_app.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/win32_window.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/d3d11.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/wasapi.rs", &mut output, &mut cache);
    //generate_outputs_from_file("./platform/src/os/mswindows/win32_midi.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/winrt_midi.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/media_foundation.rs", &mut output, &mut cache);
    generate_outputs_from_file("./tools/windows_strip/dep_of_deps.rs", &mut output, &mut cache);

    generate_outputs_from_file("./platform/src/os/windows/droptarget.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/dropsource.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/dataobject.rs", &mut output, &mut cache);
    generate_outputs_from_file("./platform/src/os/windows/enumformatetc.rs", &mut output, &mut cache);
    
    fn generate_string_from_outputs(node: &Node, output: &mut String) {
        match node {
            Node::Sub(vec) => {
                for (sub, node) in vec {
                    if let Node::Value(v) = node {
                        output.push_str(&v);
                        output.push_str("\n");
                    }
                    else {
                        output.push_str(&format!("pub mod {}{{\n", sub));
                        generate_string_from_outputs(node, output);
                        output.push_str("}\n");
                    }
                }
            }
            _ => panic!()
        }
    }

    // lets just copy in collections 
    remove_node(&mut output, id!(Foundation));
    
    // ok lets recursively walk the tree now
    let mut gen = String::new();
    gen.push_str("#![allow(non_camel_case_types)]#![allow(non_upper_case_globals)]\n pub mod Foundation;");
    generate_string_from_outputs(&output, &mut gen);
    // lets write the output file
    fs::write("./libs/windows/src/Windows/mod.rs", gen).unwrap();
}
