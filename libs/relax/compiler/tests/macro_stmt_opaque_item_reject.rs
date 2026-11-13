// rejects: expected item, found statement metavariable
// A `$s:stmt` fragment is a statement nonterminal even when it holds an item: transcribed where an
// item is expected it is no item (rustc: "expected item, found `stmt` metavariable").
macro_rules! as_item {
    ($s:stmt) => { $s };
}
as_item!(const VALUE: u32 = 7;);
