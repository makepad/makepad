// Mail (apps/mail/src/engine.rs:318): a closure calling a captured opaque callable (`impl Fn(&Message) -> bool + '_`)
// inside `.filter(..)`. rustc accepts; the tip failed with "capture analysis incomplete: unresolved call-trait adjustment".
// N01 resolves the opaque callee through the native Fn-family selection before capture analysis.
struct Message { unread: bool }

fn mailbox_predicate(messages: &[Message]) -> impl Fn(&Message) -> bool + '_ {
    let nonempty = !messages.is_empty();
    move |_message: &Message| nonempty
}

fn mailbox_messages(messages: &[Message], unread_only: bool) -> usize {
    let pred = mailbox_predicate(messages);
    messages.iter().filter(|m| pred(m) && (!unread_only || m.unread)).count()
}

#[test]
fn opaque_callable_captured_by_filter_closure() {
    assert_eq!(mailbox_messages(&[Message { unread: true }, Message { unread: false }], true), 1);
    assert_eq!(mailbox_messages(&[Message { unread: true }, Message { unread: false }], false), 2);
}
