// Exercise the shipping preference codec without the private Sandbox checkout.
use makepad_asset_chat::authoring::{ChatPrefs, AuthorPolicy, AuthorScope, scope, doctrine};
use makepad_asset_client::ChatProviderKind;

#[test]
fn modes_persist_and_old_files_keep_local_authorization_defaults() {
    for policy in AuthorPolicy::ALL {
        let prefs = ChatPrefs { author_policy: policy, ..Default::default() };
        assert_eq!(ChatPrefs::parse(&prefs.render()), prefs);
        assert!(prefs.local_only);
        assert_eq!(prefs.provider, ChatProviderKind::FleetQwen);
    }
    let old = ChatPrefs::parse("provider=claude-cli\nlocal_only=true\n");
    assert!(old.local_only);
    assert_eq!(old.provider, ChatProviderKind::FleetQwen);
    assert_eq!(old.author_policy, AuthorPolicy::Auto);
    let explicit = ChatPrefs::parse("provider=claude-cli\nlocal_only=false\nauthor_policy=expert\n");
    assert_eq!(explicit.provider, ChatProviderKind::ClaudeCli);
    assert!(!explicit.local_only);
    assert_eq!(ChatPrefs::parse(&explicit.render()), explicit);
    assert_eq!(ChatPrefs::parse("author_policy=bad"), ChatPrefs::default());
}

#[test]
fn composite_routing_and_history_keep_scoped_creation_tools() {
    for text in ["model and rig a dragon for this village", "a banked racetrack in an alpine valley",
        "make cars faster and add woods", "add a dragon with a custom controller to the ridge",
        "make a village with a train", "an RTS map with armies and units", "zombies in the woods",
        "a village with a quest", "move it beside the village"] {
        assert_eq!(scope(text, None), AuthorScope::Staged, "{text}");
    }
    assert_eq!(scope("add woods", None), AuthorScope::Map);
    assert_eq!(scope("move it beside the village", Some("model a dragon")), AuthorScope::Staged);
    assert_eq!(scope("make it taller", Some("add a mountain")), AuthorScope::Map);
    let expert = doctrine(AuthorPolicy::Expert, AuthorScope::Map);
    assert!(expert.contains("active: expert") && expert.contains("model.build"));
    assert!(doctrine(AuthorPolicy::Auto, AuthorScope::Map).contains("active: guided"));
}
