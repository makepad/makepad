// rejects: macro expansion ignores `stmt` metavariable and any tokens following
// rejects-at: 6:20 macro expansion ignores `stmt` metavariable
// A `$s:stmt` fragment is a statement nonterminal even when it holds an item: transcribed where an
// item is expected it is no item, and the expansion's items end before it.
macro_rules! as_item {
    ($s:stmt) => { $s };
}
as_item!(const VALUE: u32 = 7;);
