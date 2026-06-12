use crate::{terminal_display_name, truncate_inline};
use makepad_studio_protocol::hub_protocol::AiAgentId;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug)]
pub struct AiTrackedTask {
    pub id: u64,
    pub agent_id: AiAgentId,
    pub goal: String,
    pub terminal_path: Option<String>,
    pub expected_paths: Vec<String>,
    pub touched_paths: Vec<String>,
    pub status: String,
    pub last_terminal_mode: String,
    pub last_terminal_summary: String,
    pub last_terminal_excerpt: String,
    pub last_codex_status: Option<String>,
    pub handled_followup_signatures: Vec<String>,
}

pub fn should_show_live_task(task: &AiTrackedTask) -> bool {
    !matches!(task.status.as_str(), "done" | "cancelled")
        && task
            .terminal_path
            .as_deref()
            .map(|_| task.last_terminal_mode != "idle" && task.last_terminal_mode != "done")
            .unwrap_or(true)
}

pub fn terminal_followup_signature(kind: &str, path: &str, task: &AiTrackedTask) -> String {
    format!(
        "terminal:{}:{}:{}:{}:{}:{}",
        path,
        kind,
        task.last_terminal_mode,
        task.last_terminal_summary,
        task.last_codex_status.as_deref().unwrap_or(""),
        stable_text_fingerprint(&task.last_terminal_excerpt)
    )
}

pub fn stable_text_fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub fn live_task_title(task: &AiTrackedTask) -> String {
    if task.goal.starts_with("Observe Codex terminal `") {
        let terminal = task
            .terminal_path
            .as_deref()
            .map(terminal_display_name)
            .unwrap_or_else(|| "terminal".to_string());
        let action = match task.last_terminal_mode.as_str() {
            "awaiting-input" => "Reply to",
            "needs-attention" => "Resolve",
            "working" => "Monitor",
            _ => "Review",
        };
        return format!("{} `{}`", action, terminal);
    }
    truncate_inline(&task.goal, 96)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> AiTrackedTask {
        AiTrackedTask {
            id: 1,
            agent_id: AiAgentId(1),
            goal: "Observe Codex terminal `makepad/studio`".to_string(),
            terminal_path: Some("makepad/studio".to_string()),
            expected_paths: Vec::new(),
            touched_paths: Vec::new(),
            status: "watching".to_string(),
            last_terminal_mode: "awaiting-input".to_string(),
            last_terminal_summary: "waiting".to_string(),
            last_terminal_excerpt: "hello".to_string(),
            last_codex_status: Some("Idle".to_string()),
            handled_followup_signatures: Vec::new(),
        }
    }

    #[test]
    fn live_task_visibility_hides_done_terminal_tasks() {
        let mut task = task();
        assert!(should_show_live_task(&task));

        task.last_terminal_mode = "done".to_string();
        assert!(!should_show_live_task(&task));
    }

    #[test]
    fn terminal_followup_signature_changes_with_excerpt() {
        let mut task = task();
        let first = terminal_followup_signature("awaiting-input", "term", &task);
        task.last_terminal_excerpt = "different".to_string();
        let second = terminal_followup_signature("awaiting-input", "term", &task);
        assert_ne!(first, second);
    }

    #[test]
    fn live_task_title_uses_terminal_action() {
        let task = task();
        assert_eq!(live_task_title(&task), "Reply to `studio`");
    }
}
