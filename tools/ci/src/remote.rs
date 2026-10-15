//! The app's remote commands (makepad-remote, over the app's Unix socket), with ownership checked
//! after every reply.
use makepad_remote::Client;

#[derive(Debug)]
pub enum Failure {
    Message(String),
    Interrupted(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(s) | Self::Interrupted(s) => f.write_str(s),
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

pub struct Remote {
    pub client: Client,
    pub user_seq: Option<u64>,
}

/// The `user_seq` of an answer's end line (every answer's end line carries it).
fn end_seq(end: &str) -> Option<u64> {
    let rest = end.split("\"user_seq\":").nth(1)?;
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

impl Remote {
    /// Runs one command (`argv`: its words and `k=v` arguments, as typed after `makepad-remote
    /// app <pid>`) and returns the answer's lines joined by newlines. A mutation carries the
    /// pinned user sequence, so the app refuses it once the person intervened; every answer's
    /// end line reports the sequence as it left the app, and a change means the person
    /// used the app.
    pub fn request(&self, argv: &[String], mutate: bool) -> Result<String> {
        let shown = argv.join(" ");
        let mut argv = argv.to_vec();
        if mutate {
            let seq = self
                .user_seq
                .ok_or_else(|| Failure::Message("refusing mutation before activity".into()))?;
            argv.push(format!("if_user_seq={seq}"));
        }
        let mut lines = Vec::new();
        let (_, end) = makepad_remote::app::run_end(&self.client, &argv, &mut |l| {
            lines.push(l.to_string());
            true
        })
        .map_err(|e| Failure::Message(format!("{shown}: {e}")))?;
        let text = lines.join("\n");
        if lines.iter().any(|l| l.starts_with("{\"err\":\"user_interacting\"") || l.starts_with("{\"err\":\"user_intervened\"")) {
            return Err(Failure::Interrupted(format!("{shown}: human intervention ({text}); leaving the app running")));
        }
        if let Some(seq) = self.user_seq {
            match end.as_deref().and_then(end_seq) {
                Some(now) if now == seq => {}
                Some(now) => return Err(Failure::Interrupted(format!("{shown}: human intervention (user_seq={now}); leaving the app running"))),
                // the app is gone after a quit
                None if matches!(argv.first().map(String::as_str), Some("gq" | "quit")) => {}
                None => return Err(Failure::Interrupted("missing ownership reading; leaving the instance running".into())),
            }
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_sequence_is_read_from_an_end_line() {
        assert_eq!(end_seq("{\"end\":\"snap\",\"ms\":3,\"user_seq\":12,\"user_seq_start\":11}"), Some(12));
        assert_eq!(end_seq("{\"end\":\"snap\",\"ms\":3,\"user_seq_start\":11}"), None);
        assert_eq!(end_seq("{\"err\":\"x\"}"), None);
    }
}
