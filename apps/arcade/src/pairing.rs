//! BYO API keys, and the LAN paste flow for devices with no keyboard
//! (game.md §"AI tiers": no QR, no third-party site — the target device serves
//! the page itself and the key crosses only the local network).
//!
//! Security notes, deliberately modest and written down rather than implied:
//! - The key is stored in a file under the app config dir at mode 0600. That
//!   is not a keystore; on Android/iOS this must move to the platform keystore
//!   before those ship.
//! - The pairing page is plaintext HTTP on the LAN. Arcade already assumes a
//!   trusted LAN (multiplayer runs on it). The 4-digit code is there to stop
//!   pasting into the WRONG device in a room with several, not to stop an
//!   attacker who is already on the wire.
//! - The key is never logged, never replicated to peers, and never written
//!   into a game package.

use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    ClaudeCode,
    Anthropic,
    OpenAi,
    Gemini,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code (CLI)",
            Provider::Anthropic => "Anthropic API",
            Provider::OpenAi => "OpenAI API",
            Provider::Gemini => "Gemini API",
        }
    }

    /// Only the direct-HTTP providers need a key; a CLI agent carries its own
    /// auth, which is why a PC with the CLI never has to pair at all.
    pub fn needs_key(self) -> bool {
        !matches!(self, Provider::ClaudeCode)
    }

    pub fn slug(self) -> &'static str {
        match self {
            Provider::ClaudeCode => "claude_code",
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        Some(match s {
            "claude_code" => Provider::ClaudeCode,
            "anthropic" => Provider::Anthropic,
            "openai" => Provider::OpenAi,
            "gemini" => Provider::Gemini,
            _ => return None,
        })
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ARCADE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config").join("makepad-arcade")
}

fn key_path(provider: Provider) -> PathBuf {
    config_dir().join(format!("{}.key", provider.slug()))
}

/// Persist a key 0600. Returns an error rather than logging the key on any
/// failure path — an error message that quotes the secret is a leak.
pub fn store_key(provider: Provider, key: &str) -> std::io::Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = key_path(provider);
    let mut file = std::fs::File::create(&path)?;
    file.write_all(key.trim().as_bytes())?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn load_key(provider: Provider) -> Option<String> {
    // An env var wins, so CI and headless runs never touch the config dir.
    if let Ok(k) = std::env::var(match provider {
        Provider::Anthropic => "ANTHROPIC_API_KEY",
        Provider::OpenAi => "OPENAI_API_KEY",
        Provider::Gemini => "GOOGLE_API_KEY",
        Provider::ClaudeCode => "ARCADE_UNUSED_KEY",
    }) {
        if !k.is_empty() {
            return Some(k);
        }
    }
    std::fs::read_to_string(key_path(provider))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn has_key(provider: Provider) -> bool {
    !provider.needs_key() || load_key(provider).is_some()
}

/// A pairing session: the device shows `code`, the browser must echo it.
pub struct Pairing {
    pub code: String,
    pub provider: Provider,
}

impl Pairing {
    /// The code only has to be unguessable-enough for a room, and it must not
    /// depend on a RNG crate — tick-derived entropy is plenty here.
    pub fn new(provider: Provider, seed: u64) -> Self {
        let n = (seed.wrapping_mul(2654435761) >> 16) % 10_000;
        Self {
            code: format!("{n:04}"),
            provider,
        }
    }

    pub fn page_html(&self) -> String {
        format!(
            "<!doctype html><meta name=viewport content=\"width=device-width\">\
<title>Pair Arcade</title>\
<style>body{{font:16px system-ui;margin:40px auto;max-width:30rem}}\
input{{font:inherit;width:100%;padding:.6rem;margin:.4rem 0}}\
button{{font:inherit;padding:.6rem 1.2rem}}</style>\
<h2>Connect an API key</h2>\
<p>Provider: <b>{}</b></p>\
<p>Type the 4-digit code shown on the device, then paste the key.</p>\
<form method=POST action=/pair>\
<input name=code placeholder=\"4-digit code\" inputmode=numeric autocomplete=off>\
<input name=key placeholder=\"paste API key\" autocomplete=off>\
<button type=submit>Connect</button></form>",
            self.provider.label()
        )
    }

    /// Parse `code=..&key=..` (form or query). Returns the key only when the
    /// code matches, so a stray POST to the wrong device stores nothing.
    pub fn accept(&self, body: &str) -> Result<String, PairError> {
        let mut code = None;
        let mut key = None;
        for pair in body.split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            let v = url_decode(v);
            match k {
                "code" => code = Some(v),
                "key" => key = Some(v),
                _ => {}
            }
        }
        let (Some(code), Some(key)) = (code, key) else {
            return Err(PairError::Malformed);
        };
        if code.trim() != self.code {
            return Err(PairError::WrongCode);
        }
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(PairError::Malformed);
        }
        Ok(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairError {
    WrongCode,
    Malformed,
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b as char);
                        i += 3;
                    }
                    Err(_) => {
                        out.push('%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `ARCADE_CONFIG_DIR` is process-global, so every test that touches the
    /// key store must hold this or they clobber each other in parallel.
    pub(crate) static KEY_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn wrong_code_is_refused_and_stores_nothing() {
        let p = Pairing::new(Provider::Anthropic, 12345);
        assert_eq!(p.accept("code=0000&key=sk-test"), Err(PairError::WrongCode));
        assert_eq!(p.accept("key=sk-test"), Err(PairError::Malformed));
        assert_eq!(
            p.accept(&format!("code={}&key=", p.code)),
            Err(PairError::Malformed)
        );
    }

    #[test]
    fn right_code_yields_the_key_url_decoded() {
        let p = Pairing::new(Provider::Anthropic, 999);
        let body = format!("code={}&key=sk-ant%2Dabc+def", p.code);
        assert_eq!(p.accept(&body).unwrap(), "sk-ant-abc def");
    }

    #[test]
    fn code_is_four_digits() {
        for seed in [0u64, 1, 7, 12345, u64::MAX] {
            let p = Pairing::new(Provider::OpenAi, seed);
            assert_eq!(p.code.len(), 4, "{}", p.code);
            assert!(p.code.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn cli_providers_need_no_key() {
        assert!(!Provider::ClaudeCode.needs_key());
        assert!(has_key(Provider::ClaudeCode));
        assert!(Provider::Anthropic.needs_key());
    }

    #[test]
    fn keys_round_trip_at_0600_and_never_appear_in_the_page() {
        let _guard = KEY_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("arcade-key-test-{}", std::process::id()));
        std::env::set_var("ARCADE_CONFIG_DIR", &dir);
        store_key(Provider::OpenAi, "sk-secret-value").unwrap();
        assert_eq!(load_key(Provider::OpenAi).as_deref(), Some("sk-secret-value"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("openai.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let page = Pairing::new(Provider::OpenAi, 1).page_html();
        assert!(!page.contains("sk-secret-value"));
        std::fs::remove_dir_all(&dir).ok();
        std::env::remove_var("ARCADE_CONFIG_DIR");
    }
}
