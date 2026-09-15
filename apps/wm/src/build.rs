//! What a BINARY decides about the window manager it links.
//!
//! The desk is a library: the desktop `wm` binary and the all-in-one
//! builds (`apps/wm-all`: the iOS build, and its desktop twin in the iOS
//! skin) are the same `App` with a different [`WmBuild`] — which app
//! crates are linked in as modules, whether processes exist at all, the
//! style the desk comes up in, and whether the assistant is seated
//! in-process. The binary sets it on `Cx` in its `app_main!` `configure:`
//! clause; `App::script_mod` reads it through the VM's `Cx` and startup
//! reads it from `cx`. No statics: the build rides on the one `Cx`.

use crate::desktop::DesktopStyle;
use makepad_app_module::AppModule;
use makepad_widgets::*;

#[derive(Clone)]
pub struct WmBuild {
    /// The app crates linked into this binary as modules, hostable
    /// in-process (libs/app_module).
    pub modules: Vec<&'static dyn AppModule>,
    /// Every app is a module: no client hub, no warm pool, no cargo
    /// launches — the all-in-one. A desktop build hosts processes too.
    pub modules_only: bool,
    /// The style the desk comes up in.
    pub style: DesktopStyle,
    /// The assistant's widget families (`makepad_aichat::script_mod`), when
    /// the pane seats the assistant in-process.
    pub assistant: Option<fn(&mut ScriptVm)>,
    /// The window's title.
    pub title: String,
}

impl Default for WmBuild {
    /// The desktop window manager: processes, no linked modules, the
    /// Omarchy desk, the assistant as a child process.
    fn default() -> Self {
        WmBuild {
            modules: Vec::new(),
            modules_only: false,
            style: DesktopStyle::Omarchy,
            assistant: None,
            title: "makepad-wm".to_string(),
        }
    }
}

impl WmBuild {
    /// The build the binary set on `Cx` before startup, or the default
    /// desktop when it set none.
    pub fn from_cx(cx: &mut Cx) -> WmBuild {
        if cx.has_global::<WmBuild>() {
            cx.get_global::<WmBuild>().clone()
        } else {
            WmBuild::default()
        }
    }

    pub fn module(&self, id: &str) -> Option<&'static dyn AppModule> {
        self.modules.iter().copied().find(|m| m.id() == id)
    }

    pub fn linked_ids(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.id()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_build_is_the_desktop_and_cx_carries_a_binary_s_choice() {
        let d = WmBuild::default();
        assert!(d.modules.is_empty() && !d.modules_only && d.assistant.is_none());
        assert_eq!(d.style, DesktopStyle::Omarchy);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(WmBuild::from_cx(&mut cx).title, "makepad-wm");
        cx.set_global(WmBuild { modules_only: true, style: DesktopStyle::Ios, title: "all".into(), ..Default::default() });
        let b = WmBuild::from_cx(&mut cx);
        assert!(b.modules_only && b.style == DesktopStyle::Ios && b.title == "all");
        assert!(b.module("sheets").is_none() && b.linked_ids().is_empty());
    }
}
