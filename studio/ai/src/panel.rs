use crate::{
    ai_agent_picker_label, ai_changed_files_markdown, ai_chat_markdown, ai_live_activity_markdown,
    ai_status_label, ai_subagent_role_label, ai_task_board_markdown, apply_local_prompt_echo,
    native_delegation_echo, non_empty_labels, AI_SUBAGENT_ROLES,
};
use makepad_studio_protocol::hub_protocol::{AiAgentId, AiBackendInfo, AiMountState, ClientToHub};

#[derive(Clone, Debug, Default)]
pub struct AiPanelViewModel {
    pub mount: String,
    pub has_state: bool,
    pub live_markdown: String,
    pub files_markdown: String,
    pub task_board_markdown: String,
    pub agent_labels: Vec<String>,
    pub agent_selected: usize,
    pub backend_labels: Vec<String>,
    pub backend_selected: usize,
    pub role_labels: Vec<String>,
    pub role_selected: usize,
    pub chat_markdown: String,
    pub status_label: String,
    pub run_button_enabled: bool,
    pub run_button_label: String,
    pub active_agent_pending: bool,
}

#[derive(Clone, Debug)]
pub enum AiPanelIntent {
    RequestState,
    CreateAgent,
    DeleteActiveAgent,
    SelectAgent(usize),
    SelectBackend(usize),
    ConfigureActiveBackend,
    SubmitPrompt { prompt: String, role_index: usize },
    CancelActivePrompt,
    SetRoleIndex(usize),
}

#[derive(Clone, Debug)]
pub enum AiPanelCommand {
    SendToHub(ClientToHub),
    OpenUrl(String),
    ClearPromptInput,
    FocusPromptInput,
    RefreshView,
    ScrollChatToBottom,
    OpenActivityFolds,
}

#[derive(Clone, Debug, Default)]
pub struct AiPanelController {
    mount: String,
    state: Option<AiMountState>,
    role_index: usize,
}

impl AiPanelViewModel {
    pub fn loading(mount: impl Into<String>) -> Self {
        let mount = mount.into();
        Self {
            mount,
            has_state: false,
            live_markdown: "_No live AI state yet._".to_string(),
            files_markdown: "_No files changed yet._".to_string(),
            task_board_markdown: "_No active tasks._".to_string(),
            agent_labels: vec!["Loading AI...".to_string()],
            agent_selected: 0,
            backend_labels: vec!["Loading...".to_string()],
            backend_selected: 0,
            role_labels: role_labels(),
            role_selected: 0,
            chat_markdown: "_No AI state yet._".to_string(),
            status_label: "Loading AI...".to_string(),
            run_button_enabled: false,
            run_button_label: "▶".to_string(),
            active_agent_pending: false,
        }
    }

    pub fn from_state(mount: impl Into<String>, state: &AiMountState, role_index: usize) -> Self {
        let mount = mount.into();
        let role_selected = clamp_role_index(role_index);
        let active_backend = active_backend(state);
        let active_backend_configured = active_backend
            .map(|backend| backend.configured)
            .unwrap_or(true);

        let agent_labels = non_empty_labels(
            state
                .agents
                .iter()
                .map(|agent| ai_agent_picker_label(agent, state))
                .collect(),
            "Chat 1",
        );
        let agent_selected = state
            .active_agent_id
            .and_then(|selected| {
                state
                    .agents
                    .iter()
                    .position(|agent| agent.agent_id == selected)
            })
            .unwrap_or(0);

        let backend_labels = non_empty_labels(
            state
                .backends
                .iter()
                .map(|backend| backend.label.clone())
                .collect(),
            "local",
        );
        let backend_selected = state
            .active_backend_id
            .as_ref()
            .and_then(|active_id| {
                state
                    .backends
                    .iter()
                    .position(|backend| &backend.id == active_id)
            })
            .unwrap_or(0);

        let (chat_markdown, status_label, run_button_enabled, run_button_label, pending) =
            if let Some(agent) = state.active_agent.as_ref() {
                let status_label = if active_backend_configured {
                    ai_status_label(agent, role_selected)
                } else {
                    "Configure backend".to_string()
                };
                (
                    ai_chat_markdown(agent),
                    status_label,
                    active_backend_configured,
                    if agent.pending { "■" } else { "▶" }.to_string(),
                    agent.pending,
                )
            } else {
                (
                    "_No AI chats for this mount._".to_string(),
                    "No active AI chat".to_string(),
                    false,
                    "▶".to_string(),
                    false,
                )
            };

        Self {
            mount: mount.clone(),
            has_state: true,
            live_markdown: ai_live_activity_markdown(&mount, state),
            files_markdown: ai_changed_files_markdown(&mount, state),
            task_board_markdown: ai_task_board_markdown(&mount, state),
            agent_labels,
            agent_selected,
            backend_labels,
            backend_selected,
            role_labels: role_labels(),
            role_selected,
            chat_markdown,
            status_label,
            run_button_enabled,
            run_button_label,
            active_agent_pending: pending,
        }
    }

    pub fn from_optional_state(
        mount: impl Into<String>,
        state: Option<&AiMountState>,
        role_index: usize,
    ) -> Self {
        let mount = mount.into();
        state
            .map(|state| Self::from_state(mount.clone(), state, role_index))
            .unwrap_or_else(|| Self::loading(mount))
    }
}

impl AiPanelController {
    pub fn new(mount: impl Into<String>) -> Self {
        Self {
            mount: mount.into(),
            state: None,
            role_index: 0,
        }
    }

    pub fn mount(&self) -> &str {
        &self.mount
    }

    pub fn state(&self) -> Option<&AiMountState> {
        self.state.as_ref()
    }

    pub fn state_mut(&mut self) -> Option<&mut AiMountState> {
        self.state.as_mut()
    }

    pub fn set_mount(&mut self, mount: impl Into<String>) {
        self.mount = mount.into();
        self.state = None;
        self.role_index = 0;
    }

    pub fn receive_state(&mut self, state: AiMountState) -> bool {
        let should_scroll = state
            .active_agent
            .as_ref()
            .map(|agent| !agent.messages.is_empty())
            .unwrap_or(false);
        self.state = Some(state);
        should_scroll
    }

    pub fn view_model(&self) -> AiPanelViewModel {
        AiPanelViewModel::from_optional_state(
            self.mount.clone(),
            self.state.as_ref(),
            self.role_index,
        )
    }

    pub fn handle_intent(&mut self, intent: AiPanelIntent) -> Vec<AiPanelCommand> {
        match intent {
            AiPanelIntent::RequestState => vec![self.send(ClientToHub::AiGetState {
                mount: self.mount.clone(),
            })],
            AiPanelIntent::CreateAgent => vec![self.send(ClientToHub::AiCreateAgent {
                mount: self.mount.clone(),
                title: None,
            })],
            AiPanelIntent::DeleteActiveAgent => self
                .active_agent_id()
                .map(|agent_id| {
                    vec![self.send(ClientToHub::AiDeleteAgent {
                        mount: self.mount.clone(),
                        agent_id,
                    })]
                })
                .unwrap_or_default(),
            AiPanelIntent::SelectAgent(index) => self
                .state
                .as_ref()
                .and_then(|state| state.agents.get(index))
                .map(|agent| {
                    vec![self.send(ClientToHub::AiSelectAgent {
                        mount: self.mount.clone(),
                        agent_id: agent.agent_id,
                    })]
                })
                .unwrap_or_default(),
            AiPanelIntent::SelectBackend(index) => self
                .state
                .as_ref()
                .and_then(|state| state.backends.get(index))
                .map(|backend| {
                    vec![self.send(ClientToHub::AiSetBackend {
                        mount: self.mount.clone(),
                        backend_id: backend.id.clone(),
                    })]
                })
                .unwrap_or_default(),
            AiPanelIntent::ConfigureActiveBackend => self.configure_active_backend_commands(),
            AiPanelIntent::SubmitPrompt { prompt, role_index } => {
                self.submit_prompt_commands(&prompt, role_index)
            }
            AiPanelIntent::CancelActivePrompt => self
                .active_agent_id()
                .map(|agent_id| {
                    vec![self.send(ClientToHub::AiCancelPrompt {
                        mount: self.mount.clone(),
                        agent_id,
                    })]
                })
                .unwrap_or_default(),
            AiPanelIntent::SetRoleIndex(index) => {
                self.role_index = clamp_role_index(index);
                vec![AiPanelCommand::RefreshView]
            }
        }
    }

    fn configure_active_backend_commands(&self) -> Vec<AiPanelCommand> {
        let Some((backend_id, configuration_url)) = self.active_backend_config() else {
            return Vec::new();
        };
        let mut commands = vec![self.send(ClientToHub::AiConfigureBackend {
            mount: self.mount.clone(),
            backend_id,
        })];
        if let Some(url) = configuration_url {
            commands.push(AiPanelCommand::OpenUrl(url));
        }
        commands
    }

    fn submit_prompt_commands(&mut self, prompt: &str, role_index: usize) -> Vec<AiPanelCommand> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Vec::new();
        }
        self.role_index = clamp_role_index(role_index);

        let Some((agent_id, pending)) = self.active_agent_id_and_pending() else {
            return Vec::new();
        };
        if pending {
            return Vec::new();
        }

        if prompt.starts_with('/') {
            self.apply_prompt_echo(agent_id, prompt);
            return self.after_local_send(vec![self.send(ClientToHub::AiSendPrompt {
                mount: self.mount.clone(),
                agent_id,
                text: prompt.to_string(),
            })]);
        }

        let role = AI_SUBAGENT_ROLES
            .get(self.role_index)
            .copied()
            .unwrap_or("coder");
        let echo = native_delegation_echo(role, prompt);
        self.apply_prompt_echo(agent_id, &echo);
        self.after_local_send(vec![self.send(ClientToHub::AiSpawnSubagent {
            mount: self.mount.clone(),
            parent_agent_id: agent_id,
            role: role.to_string(),
            task: prompt.to_string(),
        })])
    }

    fn apply_prompt_echo(&mut self, agent_id: AiAgentId, prompt: &str) {
        if let Some(state) = self.state.as_mut() {
            apply_local_prompt_echo(state, agent_id, prompt);
        }
    }

    fn after_local_send(&self, mut commands: Vec<AiPanelCommand>) -> Vec<AiPanelCommand> {
        commands.push(AiPanelCommand::ClearPromptInput);
        commands.push(AiPanelCommand::RefreshView);
        commands.push(AiPanelCommand::FocusPromptInput);
        commands.push(AiPanelCommand::OpenActivityFolds);
        commands.push(AiPanelCommand::ScrollChatToBottom);
        commands
    }

    fn active_agent_id(&self) -> Option<AiAgentId> {
        self.state
            .as_ref()
            .and_then(|state| state.active_agent_id)
            .or_else(|| {
                self.state
                    .as_ref()
                    .and_then(|state| state.active_agent.as_ref())
                    .map(|agent| agent.agent_id)
            })
    }

    fn active_agent_id_and_pending(&self) -> Option<(AiAgentId, bool)> {
        let state = self.state.as_ref()?;
        state
            .active_agent
            .as_ref()
            .map(|agent| (agent.agent_id, agent.pending))
            .or_else(|| {
                state.active_agent_id.map(|agent_id| {
                    let pending = state
                        .agents
                        .iter()
                        .find(|agent| agent.agent_id == agent_id)
                        .map(|agent| agent.pending)
                        .unwrap_or(false);
                    (agent_id, pending)
                })
            })
    }

    fn active_backend_config(&self) -> Option<(String, Option<String>)> {
        let state = self.state.as_ref()?;
        let backend_id = state
            .active_backend_id
            .clone()
            .or_else(|| state.backends.first().map(|backend| backend.id.clone()))?;
        let configuration_url = state
            .backends
            .iter()
            .find(|backend| backend.id == backend_id)
            .and_then(|backend| backend.configuration_url.clone());
        Some((backend_id, configuration_url))
    }

    fn send(&self, message: ClientToHub) -> AiPanelCommand {
        AiPanelCommand::SendToHub(message)
    }
}

pub fn role_labels() -> Vec<String> {
    AI_SUBAGENT_ROLES
        .iter()
        .map(|role| ai_subagent_role_label(role))
        .collect()
}

fn active_backend(state: &AiMountState) -> Option<&AiBackendInfo> {
    state.active_backend_id.as_ref().and_then(|active_id| {
        state
            .backends
            .iter()
            .find(|backend| &backend.id == active_id)
    })
}

fn clamp_role_index(index: usize) -> usize {
    index.min(AI_SUBAGENT_ROLES.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_studio_protocol::hub_protocol::{
        AiAgentState, AiAgentSummary, AiBackendInfo, AiMessageRole,
    };

    fn backend(id: &str, configured: bool) -> AiBackendInfo {
        AiBackendInfo {
            id: id.to_string(),
            label: id.to_string(),
            detail: String::new(),
            configured,
            configuration_url: Some("https://example.test/configure".to_string()),
            configuration_hint: None,
        }
    }

    fn summary(agent_id: AiAgentId, pending: bool) -> AiAgentSummary {
        AiAgentSummary {
            agent_id,
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: if pending { "thinking..." } else { "ready" }.to_string(),
            pending,
            updated_at: 0.0,
            message_count: 0,
            parent_agent_id: None,
            role: None,
            current_action: None,
            files_touched: Vec::new(),
            state_changed_at: 0.0,
            workflow_step_name: None,
            workflow_step_status: None,
            blocked_reason: None,
        }
    }

    fn agent(agent_id: AiAgentId, pending: bool) -> AiAgentState {
        AiAgentState {
            agent_id,
            title: "Chat 1".to_string(),
            backend_id: "local".to_string(),
            status: if pending { "thinking..." } else { "ready" }.to_string(),
            pending,
            messages: Vec::new(),
            parent_agent_id: None,
            role: None,
            subagents: Vec::new(),
            current_action: None,
            files_touched: Vec::new(),
            state_changed_at: 0.0,
            workflow_step_name: None,
            workflow_step_status: None,
            blocked_reason: None,
        }
    }

    fn state() -> AiMountState {
        let agent_id = AiAgentId(7);
        AiMountState {
            backends: vec![backend("local", true)],
            active_backend_id: Some("local".to_string()),
            active_agent_id: Some(agent_id),
            agents: vec![summary(agent_id, false)],
            active_agent: Some(agent(agent_id, false)),
            live_markdown: String::new(),
            active_workflow: None,
            visibility_events: Vec::new(),
        }
    }

    #[test]
    fn view_model_projects_ready_state() {
        let vm = AiPanelViewModel::from_state("makepad", &state(), 1);

        assert!(vm.has_state);
        assert_eq!(vm.agent_selected, 0);
        assert_eq!(vm.backend_selected, 0);
        assert_eq!(vm.role_selected, 1);
        assert!(vm.run_button_enabled);
        assert_eq!(vm.run_button_label, "▶");
    }

    #[test]
    fn slash_prompt_sends_to_active_agent_and_echoes_locally() {
        let mut controller = AiPanelController::new("makepad");
        controller.receive_state(state());

        let commands = controller.handle_intent(AiPanelIntent::SubmitPrompt {
            prompt: "/help me".to_string(),
            role_index: 0,
        });

        assert!(matches!(
            commands.first(),
            Some(AiPanelCommand::SendToHub(ClientToHub::AiSendPrompt { .. }))
        ));
        let messages = &controller
            .state()
            .unwrap()
            .active_agent
            .as_ref()
            .unwrap()
            .messages;
        assert!(matches!(
            messages.first().map(|message| &message.role),
            Some(AiMessageRole::User)
        ));
        assert_eq!(
            messages.first().map(|message| message.text.as_str()),
            Some("/help me")
        );
        assert!(commands
            .iter()
            .any(|command| matches!(command, AiPanelCommand::ClearPromptInput)));
    }

    #[test]
    fn normal_prompt_spawns_selected_role_subagent() {
        let mut controller = AiPanelController::new("makepad");
        controller.receive_state(state());

        let commands = controller.handle_intent(AiPanelIntent::SubmitPrompt {
            prompt: "implement tests".to_string(),
            role_index: 2,
        });

        assert!(matches!(
            commands.first(),
            Some(AiPanelCommand::SendToHub(ClientToHub::AiSpawnSubagent {
                role,
                task,
                ..
            })) if role == "explorer" && task == "implement tests"
        ));
        let messages = &controller
            .state()
            .unwrap()
            .active_agent
            .as_ref()
            .unwrap()
            .messages;
        assert!(messages
            .first()
            .map(|message| message.text.contains("Native explorer subagent"))
            .unwrap_or(false));
    }

    #[test]
    fn pending_agent_blocks_submit() {
        let mut mount_state = state();
        mount_state.active_agent = Some(agent(AiAgentId(7), true));
        mount_state.agents = vec![summary(AiAgentId(7), true)];
        let mut controller = AiPanelController::new("makepad");
        controller.receive_state(mount_state);

        let commands = controller.handle_intent(AiPanelIntent::SubmitPrompt {
            prompt: "do it".to_string(),
            role_index: 0,
        });

        assert!(commands.is_empty());
    }

    #[test]
    fn configure_backend_emits_hub_command_and_url() {
        let mut controller = AiPanelController::new("makepad");
        controller.receive_state(state());

        let commands = controller.handle_intent(AiPanelIntent::ConfigureActiveBackend);

        assert!(matches!(
            commands.first(),
            Some(AiPanelCommand::SendToHub(
                ClientToHub::AiConfigureBackend { .. }
            ))
        ));
        assert!(matches!(
            commands.get(1),
            Some(AiPanelCommand::OpenUrl(url)) if url == "https://example.test/configure"
        ));
    }
}
