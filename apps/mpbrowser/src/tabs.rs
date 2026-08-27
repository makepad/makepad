//! The tab model. Each tab owns one CEF browser (created lazily when the
//! view first knows its size), the texture that browser paints into, and the
//! navigation state mirrored from CEF's display/load handlers.

use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabId(pub u64);

pub struct Tab {
    pub id: TabId,
    pub browser: Option<makepad_cef::Browser>,
    /// The URL to load once the browser exists.
    pub initial_url: String,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// What the quad samples: an IOSurface-backed texture on the GPU path,
    /// a BGRA upload texture on the software path.
    pub texture: Option<Texture>,
    pub accel_target_size: Option<(usize, usize)>,
    pub accel_frame_counter: u64,
    pub nav_generation: u64,
    pub favicon: Option<Texture>,
    pub init_error: Option<String>,
    pub render_mode: makepad_cef::RenderMode,
}

impl Tab {
    fn new(id: TabId, url: &str) -> Self {
        Self {
            id,
            browser: None,
            initial_url: url.to_string(),
            title: String::new(),
            url: url.to_string(),
            loading: true,
            can_go_back: false,
            can_go_forward: false,
            texture: None,
            accel_target_size: None,
            accel_frame_counter: 0,
            nav_generation: 0,
            favicon: None,
            init_error: None,
            render_mode: makepad_cef::RenderMode::None,
        }
    }

    /// Title for the strip: the page title, else the host, else "New Tab".
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.clone();
        }
        if crate::theme::is_new_tab_url(&self.url) || self.url.is_empty() {
            return "New Tab".to_string();
        }
        host_of(&self.url).unwrap_or_else(|| self.url.clone())
    }
}

/// What the tab strip needs to draw one tab.
#[derive(Clone, Debug)]
pub struct TabSummary {
    pub id: TabId,
    pub title: String,
    pub loading: bool,
    pub active: bool,
    pub favicon: Option<Texture>,
}

#[derive(Default)]
pub struct TabModel {
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u64,
}

impl TabModel {
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.active().map(|t| t.id)
    }

    /// Insert a tab right after the active one (Chrome's placement), or at
    /// the end when there is none.
    pub fn insert(&mut self, url: &str, activate: bool) -> TabId {
        self.next_id += 1;
        let id = TabId(self.next_id);
        let at = if self.tabs.is_empty() {
            0
        } else {
            (self.active + 1).min(self.tabs.len())
        };
        self.tabs.insert(at, Tab::new(id, url));
        if activate || self.tabs.len() == 1 {
            self.active = at;
        } else if at <= self.active {
            self.active += 1;
        }
        id
    }

    /// Remove a tab. Returns the removed tab (dropping it closes its
    /// browser) and whether it was the active one.
    pub fn remove(&mut self, id: TabId) -> Option<(Tab, bool)> {
        let index = self.index_of(id)?;
        let was_active = index == self.active;
        let tab = self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if index < self.active {
            self.active -= 1;
        } else if was_active {
            // Chrome activates the tab to the right, else the new last one.
            self.active = index.min(self.tabs.len() - 1);
        }
        Some((tab, was_active))
    }

    pub fn activate(&mut self, id: TabId) -> bool {
        if let Some(index) = self.index_of(id) {
            self.active = index;
            true
        } else {
            false
        }
    }

    pub fn activate_offset(&mut self, delta: isize) {
        if self.tabs.is_empty() {
            return;
        }
        let len = self.tabs.len() as isize;
        let next = ((self.active as isize + delta) % len + len) % len;
        self.active = next as usize;
    }

    pub fn activate_index(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn summaries(&self) -> Vec<TabSummary> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| TabSummary {
                id: t.id,
                title: t.display_title(),
                loading: t.loading,
                active: i == self.active,
                favicon: t.favicon.clone(),
            })
            .collect()
    }
}

pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.trim_start_matches("www.").to_string())
}

/// Turn omnibox input into a URL: keep explicit schemes, prefix `https://`
/// for things that look like hosts, otherwise search.
pub fn resolve_omnibox(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }
    let lower = input.to_ascii_lowercase();
    if lower.contains("://")
        || lower.starts_with("about:")
        || lower.starts_with("data:")
        || lower.starts_with("chrome:")
        || lower.starts_with("file:")
        || lower.starts_with("javascript:")
        || lower.starts_with("view-source:")
    {
        return input.to_string();
    }
    let has_space = input.contains(char::is_whitespace);
    let first = input.split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) = match first.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (first, None),
    };
    let looks_like_host = !has_space
        && !host.is_empty()
        && (host == "localhost"
            || host.parse::<std::net::IpAddr>().is_ok()
            || (host.contains('.')
                && !host.starts_with('.')
                && !host.ends_with('.')
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
                && host
                    .rsplit('.')
                    .next()
                    .map(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
                    .unwrap_or(false)));
    let _ = port;
    if looks_like_host {
        format!("https://{input}")
    } else {
        format!(
            "https://www.google.com/search?q={}",
            crate::theme::percent_encode(input)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omnibox_resolution() {
        assert_eq!(resolve_omnibox("makepad.nl"), "https://makepad.nl");
        assert_eq!(resolve_omnibox("http://x.org/a b"), "http://x.org/a b");
        assert_eq!(resolve_omnibox("localhost:8080/x"), "https://localhost:8080/x");
        assert_eq!(
            resolve_omnibox("rust async traits"),
            "https://www.google.com/search?q=rust%20async%20traits"
        );
        assert_eq!(
            resolve_omnibox("hello"),
            "https://www.google.com/search?q=hello"
        );
        assert_eq!(resolve_omnibox("about:blank"), "about:blank");
    }

    #[test]
    fn model_insert_remove() {
        let mut m = TabModel::default();
        let a = m.insert("a", true);
        let b = m.insert("b", true);
        assert_eq!(m.active_id(), Some(b));
        let c = m.insert("c", false);
        assert_eq!(m.active_id(), Some(b));
        assert_eq!(m.index_of(c), Some(2));
        m.remove(b);
        assert_eq!(m.active_id(), Some(c));
        m.remove(c);
        assert_eq!(m.active_id(), Some(a));
        m.activate_offset(1);
        assert_eq!(m.active_id(), Some(a));
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://www.makepad.nl/x"), Some("makepad.nl".into()));
        assert_eq!(host_of("nope"), None);
    }
}
