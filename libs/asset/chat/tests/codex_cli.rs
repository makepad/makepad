//! Public Codex contract checks without compiling unrelated GPU unit tests.
use makepad_asset_chat::codex_cli::build_args;
use makepad_asset_chat::claude::build_prompt_only;
use makepad_asset_chat::provider::TurnInput;
use makepad_asset_chat::wire::{ChatMessage, ChatRole};

#[test]
fn codex_policy_flags_precede_resume_and_prompt_stays_on_stdin() {
    let args = build_args(&Some("test-model".into()), &Some("thread-village".into()), "game protocol", "/tmp/test");
    let resume_at = args.iter().position(|arg| arg == "resume").unwrap();
    let sandbox_at = args.iter().position(|arg| arg == "--sandbox").unwrap();
    assert!(sandbox_at < resume_at);
    assert_eq!(args[sandbox_at + 1], "read-only");
    for flag in ["--json", "--ignore-user-config", "--ignore-rules"] {
        assert!(args.iter().position(|arg| arg == flag).unwrap() < resume_at);
    }
    assert!(args.iter().any(|arg| arg == "shell_environment_policy.inherit=none"));
    assert_eq!(args[resume_at + 1], "thread-village");
    assert_eq!(args.last().map(String::as_str), Some("-"));
    assert!(args.windows(2).any(|pair| pair[0] == "-m" && pair[1] == "test-model"));
    let fresh = build_args(&None, &None, "", "/tmp/test");
    assert!(!fresh.iter().any(|arg| arg == "resume"));
    assert_eq!(fresh.last().map(String::as_str), Some("-"));
}

#[test]
fn resumed_codex_prompt_keeps_world_tool_results_and_current_context() {
    let mut input = TurnInput::new("game tool protocol", vec![
        ChatMessage::new(ChatRole::User, "inspect village"),
        ChatMessage::new(ChatRole::Assistant, "<<tool>>{\"name\":\"world.get_plan\",\"args\":{}}"),
        ChatMessage::new(ChatRole::Tool, "{\"revision\":17,\"title\":\"Village\"}"),
    ]);
    input.dynamic_context = "WORLD MANIFEST: Village".into();
    let prompt = build_prompt_only(&input, true);
    assert!(prompt.contains("[tool result]\n{\"revision\":17"));
    assert!(!prompt.contains("inspect village"));
    let args = build_args(&None, &Some("thread-village".into()), &input.system_with_dynamic(), "/tmp/test");
    assert!(args.iter().any(|arg| arg.contains("WORLD MANIFEST: Village")));
    assert!(!args.iter().any(|arg| arg.contains("revision")));
    assert!(build_prompt_only(&input, false).contains("inspect village"),
        "a newly opened game session receives the bounded full transcript");
}
