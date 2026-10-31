// rustc_expand mbe/transcribe.rs: transcribe_pnr preserves invisible groups
// inside captured types/items and when forwarding them through tt.
// rustc_parse parser/nonterminal.rs: parse_nonterminal(TT) takes one whole tree.
macro_rules! choose {
    (true $then:tt $($else:tt)*) => { $then };
    (false $then:tt $else:tt) => { $else };
}

macro_rules! relay {
    ($item:tt) => { $item };
}

macro_rules! capture_item {
    ($item:item) => { relay!($item); };
}

macro_rules! alias {
    ($name:ident = $ty:ty) => { capture_item!(type $name = $ty;); };
}

macro_rules! select {
    ($name:ident, $flag:ident, $rev:path, $forward:ty) => {
        alias!($name = choose!($flag $rev $forward));
    };
}

select!(Forward, true, Option<u8>, Result<u16, u32>);
select!(Reverse, false, Option<u8>, Result<u16, u32>);

#[test]
fn nested_paths_remain_single_token_trees() {
    let a: Forward = Some(7);
    let b: Reverse = Ok(42);
    assert_eq!(a, Some(7u8));
    assert_eq!(b, Ok::<u16, u32>(42));
}
