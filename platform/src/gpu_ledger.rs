//! Cx-side GPU attribution: owner scopes, labels and creation sites.
//! The HAL receives these on resource commands and owns accounting and inspection.
//! Web retains its browser-owned inventory in `gpu_inventory`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::Location;
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ origins

thread_local! {
    static SCOPES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// The label of the innermost open [`gpu_owner`] scope of this thread.
pub(crate) fn innermost_owner() -> Option<&'static str> {
    SCOPES.with_borrow(|s| s.last().copied())
}

/// Pops its owner label when dropped.
pub struct GpuOwnerScope(std::marker::PhantomData<*const ()>);

/// Names the owner of every GPU resource created while the guard lives (this thread). Nest freely.
#[must_use = "the owner label ends when the guard is dropped"]
pub fn gpu_owner(label: &'static str) -> GpuOwnerScope {
    SCOPES.with_borrow_mut(|s| s.push(label));
    GpuOwnerScope(std::marker::PhantomData)
}

impl Drop for GpuOwnerScope {
    fn drop(&mut self) {
        SCOPES.with_borrow_mut(|s| {
            let popped = s.pop();
            debug_assert!(popped.is_some(), "gpu_owner scopes dropped out of order");
        });
    }
}

/// Who made a GPU resource. The HAL records allocation and usage times.
#[derive(Clone, Debug, Default)]
pub struct GpuOrigin {
    /// the `Texture::new_with_format` (or pool alloc) call
    /// `file:line` of the creating call, copied out of the program's code at creation (a Relax
    /// hot patch may unmap the code that held the `Location`)
    pub site: Option<Arc<str>>,
    /// the owner scopes open at creation, outermost first, joined with `/`
    pub scope: String,
    /// an explicit `Texture::label`
    pub label: Option<Arc<str>>,
    owner_cache: std::cell::OnceCell<String>,
}
static LABELS: Mutex<Option<HashMap<(usize, usize), Arc<str>>>> = Mutex::new(None);

/// An owned copy of a `&'static str` label, once per label.
fn intern_label(label: &'static str) -> Arc<str> {
    let mut m = LABELS.lock().unwrap_or_else(|e| e.into_inner());
    m.get_or_insert_with(HashMap::new).entry((label.as_ptr() as usize, label.len())).or_insert_with(|| label.into()).clone()
}

impl GpuOrigin {
    #[track_caller]
    pub(crate) fn here() -> GpuOrigin {
        let scope = SCOPES.with_borrow(|s| s.join("/"));
        let site = Location::caller();
        let site_text: Arc<str> = format!("{}:{}", site.file(), site.line()).into();
        GpuOrigin {
            site: Some(site_text),
            scope,
            label: None,
            owner_cache: Default::default(),
        }
    }
    /// The owner name reports group by: the label, else the scope, else the creating file.
    pub fn owner(&self) -> String {
        match (self.label.as_deref(), self.scope.as_str()) {
            (Some(l), "") => l.to_string(),
            (Some(l), s) => format!("{s}/{l}"),
            (None, "") => self.site.as_deref().map_or_else(|| "unknown".to_string(), |l| l.rsplit_once(':').map_or(l, |x| x.0).to_string()),
            (None, s) => s.to_string(),
        }
    }
    /// `owner()` computed once (the label can still change until the first allocation).
    pub fn owner_cached(&self) -> &str {
        self.owner_cache.get_or_init(|| self.owner())
    }
    /// Sets the explicit label (drops the cached owner string).
    pub(crate) fn set_label(&mut self, label: &'static str) {
        self.label = Some(intern_label(label));
        self.owner_cache = Default::default();
    }
    pub fn site_string(&self) -> String {
        self.site.as_deref().map_or_else(String::new, str::to_string)
    }
}
