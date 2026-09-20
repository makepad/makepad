use std::any::Any;

// ============================================================================
// ScopeDataRef / ScopeDataMut - Type-erased data containers for scope
// ============================================================================

#[derive(Default)]
pub struct ScopeDataRef<'a>(Option<&'a dyn Any>);

#[derive(Default)]
pub struct ScopeDataMut<'a>(Option<&'a mut dyn Any>);

impl<'a> ScopeDataRef<'a> {
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.as_ref().and_then(|r| r.downcast_ref())
    }
}

impl<'a> ScopeDataMut<'a> {
    pub fn get<T: Any>(&mut self) -> Option<&T> {
        self.0.as_ref().and_then(|r| r.downcast_ref())
    }

    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.0.as_mut().and_then(|r| r.downcast_mut())
    }
}

// ============================================================================
// Scope - Context passed during apply operations
// ============================================================================

#[derive(Default)]
pub struct Scope<'a, 'b> {
    pub data: ScopeDataMut<'a>,
    pub props: ScopeDataRef<'b>,
    pub index: usize,
}

impl<'a, 'b> Scope<'a, 'b> {
    pub fn with_data<T: Any>(v: &'a mut T) -> Self {
        Self {
            data: ScopeDataMut(Some(v)),
            props: ScopeDataRef(None),
            index: 0,
        }
    }

    pub fn with_data_props<T: Any + Sized, U: Any + Sized>(v: &'a mut T, w: &'b U) -> Self {
        Self {
            data: ScopeDataMut(Some(v)),
            props: ScopeDataRef(Some(w)),
            index: 0,
        }
    }

    pub fn with_props<T: Any>(w: &'b T) -> Self {
        Self {
            data: ScopeDataMut(None),
            props: ScopeDataRef(Some(w)),
            index: 0,
        }
    }

    pub fn with_data_index<T: Any>(v: &'a mut T, index: usize) -> Self {
        Self {
            data: ScopeDataMut(Some(v)),
            props: ScopeDataRef(None),
            index,
        }
    }

    pub fn with_data_props_index<T: Any>(v: &'a mut T, w: &'b T, index: usize) -> Self {
        Self {
            data: ScopeDataMut(Some(v)),
            props: ScopeDataRef(Some(w)),
            index,
        }
    }

    pub fn with_props_index<T: Any>(w: &'b T, index: usize) -> Self {
        Self {
            data: ScopeDataMut(None),
            props: ScopeDataRef(Some(w)),
            index,
        }
    }

    pub fn empty() -> Self {
        Self {
            data: ScopeDataMut(None),
            props: ScopeDataRef(None),
            index: 0,
        }
    }

    pub fn override_props<T: Any, F, R>(&mut self, props: &'b T, f: F) -> R
    where
        F: FnOnce(&mut Scope) -> R,
    {
        let mut props = ScopeDataRef(Some(props));
        std::mem::swap(&mut self.props, &mut props);
        let r = f(self);
        std::mem::swap(&mut self.props, &mut props);
        r
    }

    pub fn override_props_index<T: Any, F, R>(&mut self, props: &'b T, index: usize, f: F) -> R
    where
        F: FnOnce(&mut Scope) -> R,
    {
        let mut props = ScopeDataRef(Some(props));
        let old_index = self.index;
        self.index = index;
        std::mem::swap(&mut self.props, &mut props);
        let r = f(self);
        std::mem::swap(&mut self.props, &mut props);
        self.index = old_index;
        r
    }
}

// ============================================================================
// Apply - Source of apply operation
// ============================================================================

#[derive(Debug, Clone, Default)]
pub enum Apply {
    #[default]
    New,
    /// LiveEdit-driven hot-reload. The DSL itself changed; the template
    /// is the new source of truth and template values should override
    /// any prior runtime state.
    Reload,
    /// `script_mod` was re-run and the fresh template re-walked, but the DSL
    /// source did NOT change — the re-run exists only to re-bake heap
    /// primitives that are evaluated at `script_mod` time and are therefore
    /// unreachable to `ScriptReapply` (canonical case:
    /// `mod.widgets.SAFE_INSET_PAD_*` after a safe-area inset change, which
    /// `cx.request_live_edit()` triggers on every Android system-bar hide and
    /// every rotation).
    ///
    /// Structurally this is a reload — children lists are rebuilt and the
    /// `#[source]` object is re-bound, because the template really is a new
    /// object. Semantically it is a `ScriptReapply` — nothing the developer
    /// wrote changed, so imperative runtime state must survive. Routing it
    /// through `Reload` is what used to wipe every `set_text`, every
    /// `set_visible`, and every animator state on each inset change.
    Rebake,
    /// Heap-mutation broadcast triggered by `cx.request_script_reapply()`
    /// (e.g. preference change, safe-area inset change). The template has
    /// NOT changed — the same cached `app_value` is being re-walked so
    /// shared-heap-object references (`(mod.widgets.X.field)` lookups)
    /// pick up their new values. Field types whose canonical mutation
    /// path is an imperative setter (e.g. `Label::set_text`) should
    /// early-return on this variant so runtime state survives.
    ScriptReapply,
    Animate,
    Eval,
    Default(usize),
}

impl Apply {
    pub fn is_from_script(&self) -> bool {
        match self {
            Self::New => true,
            Self::Reload => true,
            Self::Rebake => true,
            Self::ScriptReapply => true,
            Self::Eval => true,
            _ => false,
        }
    }

    /// Returns true if this is a template apply (New or Reload) where
    /// the #[source] field should be updated. Excludes Eval since eval
    /// creates temporary objects that would become dangling after GC.
    /// Excludes ScriptReapply because the template hasn't changed —
    /// the same source object is being re-walked, so re-binding it is
    /// unnecessary work. Includes `Rebake`: `script_mod` re-ran, so the
    /// source object really is new even though the DSL text is unchanged.
    pub fn is_template_apply(&self) -> bool {
        match self {
            Self::New => true,
            Self::Reload => true,
            Self::Rebake => true,
            _ => false,
        }
    }

    pub fn is_new(&self) -> bool {
        match self {
            Self::New => true,
            _ => false,
        }
    }

    /// True for any re-application after the initial `New` — i.e. either a
    /// LiveEdit hot-reload (`Apply::Reload`) or a `request_script_reapply`-driven
    /// walk (`Apply::ScriptReapply`).
    ///
    /// Most widgets that branch on this method want to handle "the template
    /// is being re-applied for whatever reason" uniformly (re-attach
    /// listeners, re-resolve heap-ref-driven dimensions, refresh derived
    /// state). They should NOT need to differentiate between the two
    /// triggers, so we keep `is_reload` as the broad predicate.
    ///
    /// Use `is_live_edit_reload()` if you specifically want only LiveEdit,
    /// or `is_script_reapply()` for only the heap-mutation-broadcast case.
    pub fn is_reload(&self) -> bool {
        match self {
            Self::Reload => true,
            Self::Rebake => true,
            Self::ScriptReapply => true,
            _ => false,
        }
    }

    /// True only for `Apply::Reload` — a LiveEdit-driven hot-reload where
    /// the DSL itself changed. Excludes `Apply::ScriptReapply` and
    /// `Apply::Rebake` (a `script_mod` re-run with unchanged source). Use this
    /// (rather than `is_reload`) when behavior should fire only when the
    /// template source has actually changed (e.g. re-running script_mod
    /// scaffolding, invalidating template-derived caches).
    pub fn is_live_edit_reload(&self) -> bool {
        match self {
            Self::Reload => true,
            _ => false,
        }
    }

    /// True only for `Apply::ScriptReapply` — a `request_script_reapply`-driven
    /// re-walk where the template has NOT changed.
    ///
    /// Prefer `preserves_runtime_state()` when the question is "may I clobber
    /// the runtime value here?"; this predicate exists for the narrower cases
    /// that care about the specific trigger.
    pub fn is_script_reapply(&self) -> bool {
        match self {
            Self::ScriptReapply => true,
            _ => false,
        }
    }

    /// True for the walks that follow a `script_mod` re-run (`Reload`,
    /// `Rebake`). The re-run builds a fresh object heap, so any script object
    /// reference a widget cached during an earlier walk is now dangling —
    /// `Animator`'s cached state objects are the canonical example. Code
    /// holding such references must re-resolve them from the incoming value
    /// rather than reuse what it stored.
    ///
    /// `ScriptReapply` is excluded: it re-walks the same cached `app_value`,
    /// so cached references stay alive.
    pub fn follows_script_rerun(&self) -> bool {
        match self {
            Self::Reload => true,
            Self::Rebake => true,
            _ => false,
        }
    }

    /// True for the re-apply walks where nothing the developer wrote changed,
    /// so template values must NOT overwrite state that was set through an
    /// imperative setter (`Label::set_text`, `set_visible`, animator state).
    /// Field impls whose canonical mutation path is such a setter should
    /// early-return when this is true (canonical example:
    /// `ArcStringMut::script_apply`).
    ///
    /// Excludes `Apply::Reload`, where the DSL genuinely changed and the new
    /// template is meant to win.
    pub fn preserves_runtime_state(&self) -> bool {
        match self {
            Self::ScriptReapply => true,
            Self::Rebake => true,
            _ => false,
        }
    }

    pub fn is_animate(&self) -> bool {
        match self {
            Self::Animate => true,
            _ => false,
        }
    }

    pub fn is_eval(&self) -> bool {
        match self {
            Self::Eval => true,
            _ => false,
        }
    }

    pub fn as_default(&self) -> Option<usize> {
        match self {
            Self::Default(u) => Some(*u),
            _ => None,
        }
    }

    pub fn is_default(&self) -> bool {
        match self {
            Self::Default(_) => true,
            _ => false,
        }
    }
}
