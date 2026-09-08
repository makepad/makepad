//! The shared workspace surface: the Structured Dock and the Architecture
//! Dock, one drawn at a time, with the async pump that reaches every
//! resident of both. The surface changes which Dock shows; it never owns
//! documents, processes or Dock items.
use crate::atlas::view::AtlasView;
use crate::presentation::{Camera, Navigator, Presenter};
use crate::surface_pump::{self, ResidentPump};
use crate::workspace::{self, Mode, Workspace};
use makepad_terminal::widget::MpTerm;
use makepad_widgets::dock::BodyDispatch;
use makepad_widgets::*;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioSurface = #(StudioSurface::register_widget(vm)){
        width: Fill height: Fill
        dock: Dock{}
        atlas_dock: Dock{}
        disk_dock: Dock{}
    }
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioSurface {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    dock: WidgetRef,
    /// The Architecture presentation's own Dock: the map and the Inspector.
    #[live]
    atlas_dock: WidgetRef,
    /// The Disk presentation's own Dock: the volume map and a side report.
    #[live]
    disk_dock: WidgetRef,
    #[rust]
    area: Area,
    #[rust]
    pub workspace: Workspace,
    /// The last walked view rectangle, for the Architecture camera.
    #[rust]
    view: Rect,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    externally_hosted_terminals: HashSet<u64>,
    #[rust]
    atlas_draw_list: Option<DrawList2d>,
    #[rust]
    disk_draw_list: Option<DrawList2d>,
    /// The async pump over both Docks, rebuilt only when the resident set,
    /// the external set or the visible presentation changes.
    #[rust]
    pump: Option<ResidentPump>,
    #[rust]
    pump_key: Option<(Mode, Vec<u64>, Vec<u64>, Vec<u64>, Vec<u64>)>,
}
impl WidgetNode for StudioSurface {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.dock.redraw(cx);
        self.atlas_dock.redraw(cx);
        self.disk_dock.redraw(cx);
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        visit(id!(dock), self.dock.clone());
        visit(id!(atlas_dock), self.atlas_dock.clone());
        visit(id!(disk_dock), self.disk_dock.clone());
    }
    fn find_widgets_from_point(&self, cx: &Cx, p: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        match self.workspace.mode {
            Mode::Structured => self.dock.find_widgets_from_point(cx, p, found),
            Mode::Architecture => self.atlas_dock.find_widgets_from_point(cx, p, found),
            Mode::Disk => self.disk_dock.find_widgets_from_point(cx, p, found),
            // The tasks view covers the surface; its widgets are its own.
            Mode::Tasks => {}
        }
    }
}
impl StudioSurface {
    /// While the tasks view is visible, its view owns these exact Dock tab
    /// bodies. Keeping the references in Dock preserves layout and PTY
    /// lifetime; excluding their events prevents duplicate text, keys and
    /// async ticks.
    pub fn set_externally_hosted_terminals(&mut self, cx: &mut Cx, ids: HashSet<u64>) {
        if self.externally_hosted_terminals == ids { return; }
        for id in ids.difference(&self.externally_hosted_terminals) {
            self.set_anchor(cx, &self.body(*id), None);
        }
        self.externally_hosted_terminals = ids;
        self.area.redraw(cx);
    }
    pub fn set_workspace(&mut self, cx: &mut Cx, workspace: Workspace) {
        self.workspace = workspace;
        self.pump_key = None;
        self.redraw(cx);
    }
    pub fn mode(&self) -> Mode {
        self.workspace.mode
    }
    pub fn set_mode(&mut self, cx: &mut Cx, mode: Mode) {
        if self.workspace.mode != mode {
            match self.workspace.mode {
                Mode::Structured | Mode::Tasks => StructuredPresenter(self).deactivate(cx),
                Mode::Architecture => ArchitecturePresenter(self).deactivate(cx),
                Mode::Disk => DiskPresenter(self).deactivate(cx),
            }
            // Residents of the presentation being hidden end their own
            // gestures: a terminal mid-selection would otherwise keep its
            // edge auto-scroll armed with no release ever arriving.
            self.cancel_resident_gestures(cx);
            self.workspace.mode = mode;
            self.pump_key = None;
            cx.set_key_focus(self.area);
            cx.hide_text_ime();
            self.redraw(cx);
        }
    }
    fn body(&self, id: u64) -> WidgetRef {
        self.dock.as_dock().item(LiveId(id))
    }
    /// End every resident terminal's in-progress selection drag, in both
    /// Docks; editors and other bodies hold no gesture that survives a hide.
    fn cancel_resident_gestures(&self, cx: &mut Cx) {
        for dock in [&self.dock, &self.atlas_dock, &self.disk_dock] {
            let bodies = dock
                .borrow_mut::<Dock>()
                .map(|mut d| d.items().iter().map(|(_, (_, w))| w.clone()).collect::<Vec<_>>())
                .unwrap_or_default();
            for body in bodies {
                if let Some(mut term) = body.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    term.cancel_gestures(cx);
                }
            }
        }
    }
    fn set_anchor(
        &self,
        cx: &mut Cx,
        body: &WidgetRef,
        anchor: Option<(Area, PopupAnchorTransform)>,
    ) {
        if let Some(mut term) = body.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.canvas_ime_anchor = anchor;
        }
        if let Some(mut editor) = body.borrow_mut::<crate::document::StudioCodeEditor>() {
            editor.set_canvas_anchor(anchor);
        }
    }
    fn external_uids(&self) -> HashSet<WidgetUid> {
        self.externally_hosted_terminals
            .iter()
            .map(|id| self.body(*id).widget_uid())
            .collect()
    }
    /// The identities of a Dock's resident bodies, sorted: a same-count
    /// replacement of a tab changes this where a count would not.
    fn dock_identity(dock: &WidgetRef) -> Vec<u64> {
        let mut ids: Vec<u64> = dock
            .borrow_mut::<Dock>()
            .map(|mut d| d.items().iter().map(|(_, (_, w))| w.widget_uid().0).collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }
    /// Deliver a non-input event once to every resident of both Docks. The
    /// pump is cached and rebuilt only when resident identities, external
    /// ownership or the visible presentation change; hidden residents never
    /// get NextFrame.
    pub(crate) fn pump_async(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let mut external: Vec<u64> = self.externally_hosted_terminals.iter().copied().collect();
        external.sort_unstable();
        let key = (self.workspace.mode, Self::dock_identity(&self.dock), Self::dock_identity(&self.atlas_dock), Self::dock_identity(&self.disk_dock), external);
        if self.pump_key.as_ref() != Some(&key) || self.pump.is_none() {
            let mut pump = ResidentPump::new(self.external_uids());
            match self.workspace.mode {
                Mode::Architecture => {
                    pump.add_dock(&self.atlas_dock);
                    pump.add_hidden_dock(&self.dock);
                    pump.add_hidden_dock(&self.disk_dock);
                }
                Mode::Disk => {
                    pump.add_dock(&self.disk_dock);
                    pump.add_hidden_dock(&self.dock);
                    pump.add_hidden_dock(&self.atlas_dock);
                }
                Mode::Structured | Mode::Tasks => {
                    pump.add_dock(&self.dock);
                    pump.add_hidden_dock(&self.atlas_dock);
                    pump.add_hidden_dock(&self.disk_dock);
                }
            }
            self.pump = Some(pump);
            self.pump_key = Some(key);
        }
        if let Some(mut pump) = self.pump.take() {
            pump.dispatch(cx, event, scope);
            self.pump = Some(pump);
        }
    }
    /// The Architecture map widget, materialized on demand.
    pub fn atlas_view(&self, cx: &mut Cx) -> WidgetRef {
        self.atlas_dock
            .as_dock()
            .item_or_create(cx, id!(atlas_tab), id!(ArchitectureTab))
            .unwrap_or_default()
    }
    /// The Inspector widget, materialized on demand.
    pub fn atlas_inspector(&self, cx: &mut Cx) -> WidgetRef {
        self.atlas_dock
            .as_dock()
            .item_or_create(cx, id!(inspector_tab), id!(InspectorTab))
            .unwrap_or_default()
    }
    /// The History widget, materialized on demand.
    pub fn atlas_history(&self, cx: &mut Cx) -> WidgetRef {
        self.atlas_dock
            .as_dock()
            .item_or_create(cx, id!(history_tab), id!(HistoryTab))
            .unwrap_or_default()
    }
    /// The map widget if the Dock has materialized it.
    fn atlas_view_ref(&self) -> Option<WidgetRef> {
        let view = self.atlas_dock.as_dock().item(id!(atlas_tab));
        (!view.is_empty()).then_some(view)
    }
    /// Materialize the Disk map and its side report so toolbar/status binds.
    pub fn ensure_disk_tabs(&self, cx: &mut Cx) {
        let _ = self
            .disk_dock
            .as_dock()
            .item_or_create(cx, id!(disk_map_tab), id!(DiskTab));
        let _ = self
            .disk_dock
            .as_dock()
            .item_or_create(cx, id!(disk_inspector_tab), id!(DiskInspectorTab));
    }
}

/// The Structured presentation: the Dock as is. The tasks view uses the same
/// presenter underneath: hidden behind the tasks view, its bodies get async
/// delivery but never positional, key or text input.
pub(crate) struct StructuredPresenter<'a>(pub &'a mut StudioSurface);
/// The Architecture presentation: the map and the Inspector Dock.
pub(crate) struct ArchitecturePresenter<'a>(pub &'a mut StudioSurface);
/// The Disk presentation: the CWD volume map filling the workspace body.
pub(crate) struct DiskPresenter<'a>(pub &'a mut StudioSurface);

macro_rules! presenter_deref {
    ($t:ident) => {
        impl Deref for $t<'_> {
            type Target = StudioSurface;
            fn deref(&self) -> &StudioSurface {
                self.0
            }
        }
        impl DerefMut for $t<'_> {
            fn deref_mut(&mut self) -> &mut StudioSurface {
                self.0
            }
        }
    };
}
presenter_deref!(StructuredPresenter);
presenter_deref!(ArchitecturePresenter);
presenter_deref!(DiskPresenter);

impl Presenter for StructuredPresenter<'_> {
    fn camera(&self) -> Option<Camera> {
        None
    }
    fn navigator(&self) -> Option<Navigator> {
        None
    }
    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope, view: Rect) {
        let mut list = self.0.draw_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(view.pos),
                width: Size::Fixed(view.size.x),
                height: Size::Fixed(view.size.y),
                ..Default::default()
            },
            Layout::flow_overlay(),
        );
        self.0.dock.draw_walk_all(cx, scope, Walk::fill());
        cx.end_turtle();
        list.end(cx);
        list.set_view_transform_self_only(cx, &Mat4f::identity());
        self.0.draw_list = Some(list);
    }
    fn input(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !surface_pump::is_input(event) {
            // Signals, timers and document updates reach every resident of
            // both Docks once; the Dock handles only its own chrome.
            self.0.pump_async(cx, event, scope);
            if let Some(mut dock) = self.0.dock.borrow_mut::<Dock>() {
                dock.handle_event_with_bodies(cx, event, scope, BodyDispatch::Skip);
            }
        } else if self.0.workspace.mode == Mode::Structured
            && self.0.externally_hosted_terminals.is_empty()
        {
            self.0.dock.handle_event(cx, event, scope);
        }
        // Otherwise the Structured presentation is hidden behind the tasks
        // view: positional, key and text input never reaches its bodies.
    }
    fn deactivate(&mut self, cx: &mut Cx) {
        cx.hide_text_ime();
    }
}

impl Presenter for ArchitecturePresenter<'_> {
    fn camera(&self) -> Option<Camera> {
        let view = self.0.atlas_view_ref()?;
        let atlas = view.borrow::<AtlasView>()?;
        let (pan, zoom) = atlas.camera();
        Some(Camera::new(self.0.view, workspace::Camera { pan_x: pan.x, pan_y: pan.y, zoom }))
    }
    fn navigator(&self) -> Option<Navigator> {
        let view = self.0.atlas_view_ref()?;
        let atlas = view.borrow::<AtlasView>()?;
        let (bounds, viewport) = atlas.navigator()?;
        Some(Navigator { bounds, viewport: Some(viewport) })
    }
    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope, view: Rect) {
        let mut list = self.0.atlas_draw_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(view.pos),
                width: Size::Fixed(view.size.x),
                height: Size::Fixed(view.size.y),
                ..Default::default()
            },
            Layout::flow_overlay(),
        );
        self.0.atlas_dock.draw_walk_all(cx, scope, Walk::fill());
        cx.end_turtle();
        list.end(cx);
        list.set_view_transform_self_only(cx, &Mat4f::identity());
        self.0.atlas_draw_list = Some(list);
    }
    fn input(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !surface_pump::is_input(event) {
            self.0.pump_async(cx, event, scope);
            if let Some(mut dock) = self.0.atlas_dock.borrow_mut::<Dock>() {
                dock.handle_event_with_bodies(cx, event, scope, BodyDispatch::Skip);
            }
        } else {
            self.0.atlas_dock.handle_event(cx, event, scope);
        }
    }
    fn deactivate(&mut self, cx: &mut Cx) {
        if let Some(view) = self.0.atlas_view_ref() {
            if let Some(mut atlas) = view.borrow_mut::<AtlasView>() {
                atlas.set_hidden(cx);
            }
        }
    }
}
impl Presenter for DiskPresenter<'_> {
    fn camera(&self) -> Option<Camera> {
        None
    }
    fn navigator(&self) -> Option<Navigator> {
        None
    }
    fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope, view: Rect) {
        let mut list = self.0.disk_draw_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(view.pos),
                width: Size::Fixed(view.size.x),
                height: Size::Fixed(view.size.y),
                ..Default::default()
            },
            Layout::flow_overlay(),
        );
        self.0.disk_dock.draw_walk_all(cx, scope, Walk::fill());
        cx.end_turtle();
        list.end(cx);
        list.set_view_transform_self_only(cx, &Mat4f::identity());
        self.0.disk_draw_list = Some(list);
    }
    fn input(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !surface_pump::is_input(event) {
            self.0.pump_async(cx, event, scope);
            if let Some(mut dock) = self.0.disk_dock.borrow_mut::<Dock>() {
                dock.handle_event_with_bodies(cx, event, scope, BodyDispatch::Skip);
            }
        } else {
            self.0.disk_dock.handle_event(cx, event, scope);
        }
    }
    fn deactivate(&mut self, cx: &mut Cx) {
        cx.hide_text_ime();
    }
}
impl Widget for StudioSurface {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        match self.workspace.mode {
            Mode::Structured | Mode::Tasks => StructuredPresenter(self).draw(cx, scope, view),
            Mode::Architecture => ArchitecturePresenter(self).draw(cx, scope, view),
            Mode::Disk => DiskPresenter(self).draw(cx, scope, view),
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match self.workspace.mode {
            Mode::Structured | Mode::Tasks => StructuredPresenter(self).input(cx, event, scope),
            Mode::Architecture => ArchitecturePresenter(self).input(cx, event, scope),
            Mode::Disk => DiskPresenter(self).input(cx, event, scope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_roundtrip_at_extreme_pan_and_zoom() {
        for zoom in [workspace::MIN_ZOOM, 0.01, 0.2, 1.0, 8.0] {
            for pan in [-9_000_000.0, 0.0, 9_000_000.0] {
                let c = Camera::new(
                    Rect {
                        pos: dvec2(140.0, 80.0),
                        size: dvec2(1000.0, 700.0),
                    },
                    workspace::Camera {
                        pan_x: pan,
                        pan_y: -pan,
                        zoom,
                    },
                );
                let p = dvec2(511.0, 344.0);
                assert!((c.local_to_screen(c.screen_to_local(p)) - p).length() < 0.00001);
            }
        }
    }
}
