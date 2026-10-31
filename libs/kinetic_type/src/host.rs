//! One kit as a host draws it (Motion's KineticText, a VJ deck's kinetic
//! document, its text picture): the view is made on the kit's first frame
//! and a kit that does not build is reported once and draws nothing. The
//! host sets what it has (words, font, axes, colours, overlay) on the view
//! and renders it.

use crate::view::KineticView;
use makepad_draw::*;

pub struct KineticHost {
    name: String,
    source: String,
    view: Option<KineticView>,
    error: Option<String>,
}

impl KineticHost {
    /// The kit `source`, named `name` in its diagnostics (`kinetic/<name>`).
    pub fn new(name: &str, source: &str) -> Self {
        Self { name: name.to_string(), source: source.to_string(), view: None, error: None }
    }

    /// A host that only reports `error` (a kit that does not exist).
    pub fn failed(name: &str, error: String) -> Self {
        Self { name: name.to_string(), source: String::new(), view: None, error: Some(error) }
    }

    /// The kit's view, made on first use; `None` when the kit failed
    /// ([`Self::error`] says why).
    pub fn view(&mut self, cx: &mut Cx) -> Option<&mut KineticView> {
        if self.error.is_some() {
            return None;
        }
        if self.view.is_none() {
            cx.with_vm(crate::view::script_mod);
            match KineticView::new(cx, &self.source, &format!("kinetic/{}", self.name)) {
                Ok(view) => self.view = Some(view),
                Err(error) => {
                    log!("kinetic {}: {error}", self.name);
                    self.error = Some(error);
                    return None;
                }
            }
        }
        self.view.as_mut()
    }

    /// Why the kit does not draw: its build error, else the view's first
    /// error.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| self.view.as_ref().and_then(|v| v.errors.first().map(|e| e.as_str())))
    }

    /// The view's first pass (its picture's consumer, the scene), once it
    /// has drawn.
    pub fn scene_pass(&self) -> Option<DrawPassId> {
        self.view.as_ref().map(|v| v.scene_pass())
    }
}
