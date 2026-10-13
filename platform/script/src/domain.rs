//! Script domains: one table of what a VM of each kind gives the documents it runs.
//!
//! - `default_uses`: the `use` lines every document of the domain gets without writing them.
//! - `modules`: the system modules the domain's VM installs; anything else is a `use` away
//!   (a slide may `use mod.widgets.*`): these are defaults, not walls. A name a document reaches
//!   for without its module in scope fails with the `use` to add (`ScriptHeap::import_hint`).
//! - `lazy`: which of them are lazily expanded (lazy_modules.rs): registered on first touch.
//!
//! Every VM constructor names its domain here; docs and the AI references list these sets, so
//! generated code adds only the imports beyond them.
pub struct ScriptDomain {
    pub name: &'static str,
    pub what: &'static str,
    pub default_uses: &'static [&'static str],
    pub modules: &'static [&'static str],
    pub lazy: &'static [&'static str],
}

impl ScriptDomain {
    /// The default `use` lines, one per line, as a document's prefix.
    pub fn use_prefix(&self) -> String {
        self.default_uses.iter().map(|u| format!("use {u}\n")).collect()
    }
}

/// The language core every VM has: math, std, pod, shader, gc, regex, html (native).
pub const CORE: &[&str] = &["math", "std", "pod", "shader", "gc", "regex", "html"];

/// UI Splash: panels, the WM startup program and mini-apps, Stage editor UIs, app isolates.
pub const UI: ScriptDomain = ScriptDomain {
    name: "ui",
    what: "UI Splash (Splash widget bodies, panels, mini-apps)",
    default_uses: &["mod.prelude.widgets.*"],
    modules: &["cx", "net", "draw", "host", "fs (jailed)", "prelude", "theme", "themes", "widgets", "turtle", "sdf", "text", "geom", "animator", "tween"],
    lazy: &["prelude", "theme", "themes", "widgets", "turtle", "sdf", "text", "geom", "animator", "tween", "helper"],
};

/// Motion documents: slides and deck plates, CUT graphics.
pub const MOTION: ScriptDomain = ScriptDomain {
    name: "motion",
    what: "motion documents (SHOW plates, CUT graphics)",
    default_uses: &[],
    modules: &["motion", "forms", "shared", "post", "scene", "kernel"],
    lazy: &[],
};

/// Cut and show documents (the edit, the deck).
pub const CUT: ScriptDomain = ScriptDomain {
    name: "cut",
    what: "cut and show documents",
    default_uses: &[],
    modules: &["flow", "cut", "show"],
    lazy: &[],
};

pub const SCORE: ScriptDomain = ScriptDomain {
    name: "score",
    what: "score documents",
    default_uses: &[],
    modules: &["score"],
    lazy: &[],
};

pub const FLOW: ScriptDomain = ScriptDomain {
    name: "flow",
    what: "flow graphs",
    default_uses: &[],
    modules: &["flow"],
    lazy: &[],
};

pub const SCENE3D: ScriptDomain = ScriptDomain {
    name: "scene3d",
    what: "3D scenes (motion3d, sandbox scenes)",
    default_uses: &[],
    modules: &["forms", "shared", "post", "kernel", "scene"],
    lazy: &[],
};

/// Every domain, for docs, the AI references and tests.
pub const DOMAINS: &[&ScriptDomain] = &[&UI, &MOTION, &CUT, &SCORE, &FLOW, &SCENE3D];
