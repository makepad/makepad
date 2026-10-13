//! Exercise both sides of the actual proc-macro wire format, including non-current files.
#[allow(dead_code)]
#[path = "../../compiler/std/proc_macro/lib.rs"]
mod macro_side;

use relax_base::pm::{self, Delimiter, Group, Ident, LineColumn, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

fn echo_with_call_site(input: macro_side::TokenStream) -> macro_side::TokenStream {
    let mut trees: Vec<_> = input.into_iter().collect();
    assert!(trees[0].span().join(trees[1].span()).is_none());
    trees.push(macro_side::TokenTree::Ident(macro_side::Ident::new("call_site", macro_side::Span::call_site())));
    trees.into_iter().collect()
}

#[test]
fn file_spans_survive_the_proc_macro_bridge() {
    let call = Span { file: 17, lo: 11, hi: 19, start: LineColumn { line: 3, column: 5 }, end: LineColumn { line: 3, column: 13 } };
    let ident_span = Span { file: 23, ..call };
    let literal_span = Span { file: 42, ..call };
    let stream = TokenStream { trees: vec![
        TokenTree::Ident(Ident { name: "type".into(), raw: true, span: ident_span }),
        TokenTree::Group(Group { delim: Delimiter::Brace, span: call, stream: TokenStream { trees: vec![
            TokenTree::Punct(Punct { ch: ':', spacing: Spacing::Joint, span: call }),
            TokenTree::Literal(Literal { text: "123u32".into(), span: literal_span }),
        ] } }),
    ] };
    let input = pm::encode(call, &stream);
    assert_eq!(&input[24..28], &17_u32.to_le_bytes());
    let mut io = macro_side::__relax::BridgeIo {
        f: echo_with_call_site as *const () as usize,
        kind: 0,
        inp: input.as_ptr(),
        inp_len: input.len(),
        attr: std::ptr::null(),
        attr_len: 0,
        out: std::ptr::null_mut(),
        out_len: 0,
    };
    macro_side::__relax::bridge(&mut io);
    // The macro bridge owns (and leaks) this allocation, just as it does in the JIT.
    let bytes = unsafe { std::slice::from_raw_parts(io.out, io.out_len) };
    let output = pm::decode_stream(bytes).expect("macro-side output must decode on the host");
    let mut expected = stream;
    expected.trees.push(TokenTree::Ident(Ident { name: "call_site".into(), raw: false, span: call }));
    assert_eq!(pm::encode(call, &output), pm::encode(call, &expected));
    assert_eq!(output.trees[0].span(), ident_span);
    let TokenTree::Group(group) = &output.trees[1] else { panic!("expected nested group") };
    assert_eq!(group.stream.trees[1].span(), literal_span);
    assert_eq!(output.trees[2].span(), call);
}
