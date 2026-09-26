//! What Director adds to a coding agent's command line, chosen per provider in
//! Settings → Coding agents and saved as `agent_arguments.ron` in the state
//! directory beside `settings.ron`.
//!
//! By default nothing is added: an agent asks before it acts, exactly as its
//! own CLI does. The person may tick "act without asking", which adds the
//! provider's own permission-bypass flag, and may type custom arguments, which
//! follow it. The worker reads the file at every launch, resume and recovery,
//! so a change applies to the next start of a lane, never to a running one.
use crate::agent_session::AgentProvider;
use makepad_widgets::makepad_micro_serde::*;
use std::path::Path;

const FILE: &str = "agent_arguments.ron";
/// Longest custom argument text accepted per provider.
pub const MAX_ARGUMENT_BYTES: usize = 4096;

/// The providers that take launch arguments, in Settings order.
pub const PROVIDERS: [AgentProvider; 3] =
    [AgentProvider::Fable, AgentProvider::Codex, AgentProvider::Grok];

/// One provider's choices.
#[derive(Clone, Debug, Default, PartialEq, SerRon, DeRon)]
pub struct AgentLaunch {
    /// Add the provider's own flag that lets it act without asking.
    pub bypass_permissions: bool,
    /// Extra words, split like a shell splits them but never run through one.
    pub arguments: String,
}

/// Every provider's choices. A missing provider has the default: no bypass
/// and no custom arguments.
#[derive(Clone, Debug, Default, PartialEq, SerRon, DeRon)]
pub struct AgentLaunchSettings {
    pub claude: Option<AgentLaunch>,
    pub codex: Option<AgentLaunch>,
    pub grok: Option<AgentLaunch>,
}

impl AgentLaunchSettings {
    pub fn load(state_dir: &Path) -> Self {
        std::fs::read_to_string(state_dir.join(FILE))
            .ok()
            .and_then(|text| Self::deserialize_ron(&text).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file and renamed over the old one.
    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let temporary = state_dir.join(format!("{FILE}.tmp"));
        std::fs::write(&temporary, self.serialize_ron())?;
        std::fs::rename(&temporary, state_dir.join(FILE))
    }

    fn slot(&self, provider: AgentProvider) -> Option<&Option<AgentLaunch>> {
        match provider {
            AgentProvider::Fable => Some(&self.claude),
            AgentProvider::Codex => Some(&self.codex),
            AgentProvider::Grok => Some(&self.grok),
            _ => None,
        }
    }

    fn slot_mut(&mut self, provider: AgentProvider) -> Option<&mut Option<AgentLaunch>> {
        match provider {
            AgentProvider::Fable => Some(&mut self.claude),
            AgentProvider::Codex => Some(&mut self.codex),
            AgentProvider::Grok => Some(&mut self.grok),
            _ => None,
        }
    }

    pub fn get(&self, provider: AgentProvider) -> AgentLaunch {
        self.slot(provider)
            .and_then(Option::clone)
            .unwrap_or_default()
    }

    pub fn set(&mut self, provider: AgentProvider, launch: AgentLaunch) {
        if let Some(slot) = self.slot_mut(provider) {
            *slot = (launch != AgentLaunch::default()).then_some(launch);
        }
    }

    /// The words Director adds for `provider`: its bypass flag when chosen,
    /// then the custom arguments. Unreadable custom arguments fail the launch
    /// rather than start the agent without them.
    pub fn launch_words(&self, provider: AgentProvider) -> Result<Vec<String>, String> {
        let launch = self.get(provider);
        let mut words = Vec::new();
        if launch.bypass_permissions {
            words.extend(bypass_flag(provider).map(str::to_owned));
        }
        words.extend(split_arguments(&launch.arguments).map_err(|error| {
            format!(
                "The custom arguments for {} cannot be used ({error}); fix them in Settings → Coding agents",
                provider_label(provider)
            )
        })?);
        Ok(words)
    }
}

/// `AgentLaunchSettings::launch_words` from the saved file in `state_dir`.
pub fn launch_words(state_dir: &Path, provider: AgentProvider) -> Result<Vec<String>, String> {
    AgentLaunchSettings::load(state_dir).launch_words(provider)
}

/// Each provider's own flag for acting without asking. The documented long
/// form is used for Codex; `--yolo` is an alias its help does not list. Grok's
/// help describes `--always-approve` as "Auto-approve all tool executions".
pub fn bypass_flag(provider: AgentProvider) -> Option<&'static str> {
    match provider {
        AgentProvider::Fable => Some("--dangerously-skip-permissions"),
        AgentProvider::Codex => Some("--dangerously-bypass-approvals-and-sandbox"),
        AgentProvider::Grok => Some("--always-approve"),
        _ => None,
    }
}

pub fn provider_label(provider: AgentProvider) -> &'static str {
    match provider {
        AgentProvider::Fable => "Claude",
        AgentProvider::Codex => "Codex",
        AgentProvider::Grok => "Grok",
        AgentProvider::Shell => "the shell",
        AgentProvider::Unknown => "this program",
    }
}

/// Split `text` into words the way a POSIX shell splits a simple command,
/// with no shell involved and nothing expanded: blanks separate words,
/// `'…'` keeps everything literally, `"…"` keeps everything except that a
/// backslash escapes `"` or `\`, and a backslash outside quotes takes the next
/// character literally (a backslash before a line break joins the lines).
/// `$`, `~`, `*` and the like are ordinary characters.
pub fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    if text.len() > MAX_ARGUMENT_BYTES {
        return Err(format!("longer than {MAX_ARGUMENT_BYTES} bytes"));
    }
    if text.contains('\0') {
        return Err("contains a NUL character".into());
    }
    let mut words = Vec::new();
    let mut word = String::new();
    // A word exists once any of its characters or quotes is seen, so `''`
    // is an empty argument rather than nothing.
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("a ' quote is not closed".into()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("a \" quote is not closed".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("a \" quote is not closed".into()),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(c) => {
                    started = true;
                    word.push(c);
                }
                None => return Err("it ends with a lone backslash".into()),
            },
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    Ok(words)
}
