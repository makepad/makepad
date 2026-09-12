//! Root ownership of canonical document, edit session, navigation and storage.
use crate::storage::{
    encode, load_from_storage, record_write, reduce_save, LoadOutcome, SaveEffect, SaveEvent,
    SaveMachine,
};
use crate::{
    controls::{NotesMeta, NotesRule, NotesSelection, NotesTap, TapAction},
    engine,
    model::*,
    projection::ParagraphStyle,
    seed,
    text_surface::{NotesTextSurface, SurfaceAction, SurfaceState},
};
use makepad_widgets::makepad_platform::storage::{
    StorageHandle, StorageRequestId, StorageResponse, StorageResult,
};
use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum OverlayKind {
    #[default]
    None,
    More,
    Move,
    Format,
    Folders,
}
#[derive(Script, ScriptHook, Widget)]
pub struct NotesView {
    #[deref]
    view: View,
    #[live]
    reduced_motion: bool,
    #[rust]
    applied_reduced_motion: bool,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    load_req: Option<StorageRequestId>,
    #[rust]
    doc: NotesDocument,
    #[rust]
    ui: NotesUi,
    #[rust]
    load: LoadState,
    #[rust]
    load_error: Option<String>,
    #[rust]
    edit_error: Option<String>,
    #[rust]
    save: SaveMachine,
    #[rust]
    layout: LayoutDecision,
    #[rust]
    size: Vec2d,
    #[rust]
    debounce: Option<Timer>,
    #[rust]
    overlay: OverlayKind,
    #[rust]
    invoker: WidgetRef,
    #[rust]
    menu_opacity: f64,
    #[rust]
    menu_animation: Option<(f64, f64, f64, f64)>,
    #[rust]
    menu_frame: NextFrame,
    #[rust]
    menu_closing: bool,
    #[rust]
    closing: bool,
    #[rust]
    quit_now: bool,
    #[rust]
    started: bool,
    #[rust]
    list_rows: Vec<ListRow>,
    #[rust]
    rendered_rows: Vec<(WidgetUid, WidgetUid, NoteId)>,
    #[rust]
    pending_nav: bool,
    #[rust]
    navigation_frame: NextFrame,
    #[rust]
    transferred_surface: Option<(NoteId, SurfaceState)>,
    #[rust]
    focus_editor: bool,
}
fn now_ms() -> i64 {
    (Cx::time_now() * 1000.0) as i64
}
impl NotesView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    pub fn document(&self) -> &NotesDocument {
        &self.doc
    }

    pub fn query(&self) -> &str {
        &self.ui.query
    }

    pub fn set_query_text(&mut self, query: String) {
        self.ui.query = query;
    }

    pub fn tools_ready(&self) -> bool {
        self.load == LoadState::Ready
    }

    pub fn ai_summary(&self) -> String {
        if self.load != LoadState::Ready {
            return "Notes are loading".to_string();
        }
        format!(
            "{} notes · {}",
            engine::active_count(&self.doc),
            self.ui.collection.label()
        )
    }

    pub fn ai_answer(
        &self,
        call: &makepad_ai_services::wire::ServiceCall,
    ) -> makepad_ai_services::wire::ToolResult {
        crate::ai::answer(self.load, &self.doc, call)
    }

    pub fn request_close(&mut self, cx: &mut Cx) -> bool {
        self.flush_editor(cx);
        if self.load != LoadState::Ready || self.save.is_saved() {
            return true;
        }
        self.closing = true;
        self.save_now(cx, false);
        false
    }

    pub fn take_quit(&mut self) -> bool {
        let q = self.quit_now;
        self.quit_now = false;
        q
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(storage) = self.storage.clone() {
            self.load = LoadState::Loading;
            self.load_req = Some(storage.get(cx, STORAGE_KEY));
        }
        self.redraw(cx);
    }

    fn save_now(&mut self, cx: &mut Cx, debounce: bool) {
        if self.load != LoadState::Ready {
            return;
        }
        let effect = reduce_save(
            &mut self.save,
            SaveEvent::Changed {
                revision: self.doc.revision,
                now_ms: now_ms(),
                debounce,
            },
        );
        self.apply_save_effect(cx, effect);
        if debounce {
            if let Some(t) = self.debounce.take() {
                cx.stop_timer(t);
            }
            self.debounce = Some(cx.start_timeout(0.3));
        }
        self.sync_status(cx);
    }

    fn apply_save_effect(&mut self, cx: &mut Cx, effect: SaveEffect) {
        if let SaveEffect::Write { revision } = effect {
            if let Some(storage) = self.storage.clone() {
                match encode(&self.doc) {
                    Ok(bytes) => {
                        let id = storage.set(cx, STORAGE_KEY, bytes);
                        record_write(&mut self.save, id.0, revision);
                    }
                    Err(message) => {
                        self.save.error = Some(message.into());
                    }
                }
            }
        }
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.load_req == Some(response.request_id) {
                self.load_req = None;
                match &response.result {
                    Ok(StorageResult::Value(bytes)) => match load_from_storage(bytes.as_deref()) {
                        LoadOutcome::Missing => {
                            self.doc = seed::generate(now_ms());
                            self.ui = engine::initial_ui(&self.doc);
                            self.load = LoadState::Ready;
                            self.save.current = self.doc.revision;
                            self.save.saved = 0;
                            self.save_now(cx, false);
                            self.refresh_projection();
                        }
                        LoadOutcome::Loaded(doc) => {
                            self.doc = doc;
                            self.ui = engine::initial_ui(&self.doc);
                            self.load = LoadState::Ready;
                            self.save.current = self.doc.revision;
                            self.save.saved = self.doc.revision;
                            self.refresh_projection();
                        }
                        LoadOutcome::Error { message } => {
                            self.load = LoadState::Error;
                            self.load_error = Some(message.into());
                        }
                    },
                    Err(e) => {
                        self.load = LoadState::Error;
                        self.load_error = Some(e.to_string());
                    }
                    _ => {}
                }
                self.redraw(cx);
            }
            if self.save.in_flight.map(|(id, _)| StorageRequestId(id)) == Some(response.request_id)
            {
                let ok = response.result.is_ok();
                let message = response
                    .result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                let effect = reduce_save(
                    &mut self.save,
                    SaveEvent::Ack {
                        request_id: response.request_id.0,
                        ok,
                        message,
                    },
                );
                self.apply_save_effect(cx, effect);
                if ok && self.closing && self.save.is_saved() {
                    self.quit_now = true;
                }
                self.sync_status(cx);
            }
        }
    }

    fn mutations_allowed(&self) -> bool {
        self.load == LoadState::Ready
    }

    fn selected_is_deleted(&self) -> bool {
        self.ui
            .selected
            .and_then(|id| self.doc.note(id))
            .map(|n| n.is_deleted())
            .unwrap_or(false)
    }

    fn surface(&self, cx: &mut Cx) -> WidgetRef {
        self.view.widget(
            cx,
            if self.layout.is_compact() {
                ids!(compact.editor_view.screen.document)
            } else {
                ids!(wide.editor.document)
            },
        )
    }
    fn list(&self, cx: &mut Cx) -> WidgetRef {
        self.view.widget(
            cx,
            if self.layout.is_compact() {
                ids!(compact.list_view.screen.rows)
            } else {
                ids!(wide.notes_list.rows)
            },
        )
    }
    fn active_editor(&self) -> bool {
        !self.layout.is_compact() || engine::current_screen(&self.ui) == Screen::Editor
    }
    fn refresh_projection(&mut self) {
        self.list_rows = engine::list_rows(&self.doc, self.ui.collection, &self.ui.query, now_ms());
    }
    fn flush_editor(&mut self, cx: &mut Cx) {
        if self.active_editor() {
            if let Some(surface) = self.surface(cx).borrow::<NotesTextSurface>() {
                if let Some(session) = self.ui.editor.as_mut() {
                    session.selection = surface.source_selection();
                }
            }
        }
        self.save_now(cx, false);
    }
    fn accept_source(
        &mut self,
        cx: &mut Cx,
        id: NoteId,
        revision: u64,
        source: String,
        selection: ByteSelection,
        kind: EditKind,
    ) {
        if !self.mutations_allowed()
            || self.selected_is_deleted()
            || self.ui.selected != Some(id)
            || self.doc.revision != revision
        {
            return;
        }
        if let Some(session) = self.ui.editor.as_mut() {
            match engine::set_source(&mut self.doc, session, source, selection, now_ms(), kind) {
                Ok(()) => {
                    self.edit_error = None;
                    self.refresh_projection();
                    self.save_now(cx, kind == EditKind::Typing);
                }
                Err(e) => {
                    self.edit_error = Some(e.into());
                }
            }
            self.sync_surface(cx);
            self.redraw(cx);
        }
    }
    fn sync_surface(&mut self, cx: &mut Cx) {
        if !self.active_editor() {
            return;
        }
        let Some(session) = self.ui.editor.as_ref() else {
            return;
        };
        let Some(note) = self.doc.note(session.note_id).cloned() else {
            return;
        };
        let selection = session.selection;
        let surface = self.surface(cx);
        if let Some(mut surface) = surface.borrow_mut::<NotesTextSurface>() {
            surface.reduced_motion = self.reduced_motion;
            surface.hydrate(
                cx,
                &note,
                self.doc.revision,
                selection,
                engine::format_note_date(note.edited_ms, now_ms()),
            );
            if let Some((id, state)) = self.transferred_surface.take() {
                if id == note.id {
                    surface.restore_state(cx, state);
                }
            }
            if self.focus_editor {
                surface.focus(cx);
                self.focus_editor = false;
            }
        };
    }
    fn open_collection(&mut self, cx: &mut Cx, collection: Collection) {
        if !self.mutations_allowed() {
            return;
        }
        self.flush_editor(cx);
        engine::select_collection(&self.doc, &mut self.ui, collection);
        self.ui.route = vec![Screen::Folders, Screen::List];
        self.pending_nav = self.layout.is_compact();
        self.dismiss_overlay(cx);
        self.refresh_projection();
        self.list(cx)
            .as_portal_list()
            .set_first_id_and_scroll(0, 0.0);
        self.redraw(cx);
    }
    fn open_note(&mut self, cx: &mut Cx, id: NoteId) {
        self.flush_editor(cx);
        engine::select_note(&self.doc, &mut self.ui, id);
        self.pending_nav = self.layout.is_compact();
        self.redraw(cx);
    }
    fn go_back(&mut self, cx: &mut Cx) {
        self.flush_editor(cx);
        if let Some(mut surface) = self.surface(cx).borrow_mut::<NotesTextSurface>() {
            surface.finish(cx);
        }
        if self.layout.is_compact() {
            engine::go_back(&mut self.ui);
            self.pending_nav = true;
        }
        self.dismiss_overlay(cx);
        self.redraw(cx);
    }
    fn new_note(&mut self, cx: &mut Cx) {
        if !self.mutations_allowed() || !self.ui.collection.allows_create() {
            return;
        }
        self.flush_editor(cx);
        match engine::create_note(&mut self.doc, &mut self.ui, now_ms()) {
            Ok(_) => {
                self.focus_editor = true;
                self.pending_nav = self.layout.is_compact();
                self.refresh_projection();
                self.save_now(cx, false);
            }
            Err(e) => self.edit_error = Some(e.into()),
        }
        self.redraw(cx);
    }
    fn apply_block(&mut self, cx: &mut Cx, style: BlockStyle) {
        if self.selected_is_deleted() {
            return;
        }
        self.flush_editor(cx);
        if let Some(session) = self.ui.editor.as_ref() {
            if let Some(note) = self.doc.note(session.note_id) {
                let (source, selection) =
                    engine::apply_block_style(&note.source, session.selection, style);
                self.accept_source(
                    cx,
                    session.note_id,
                    self.doc.revision,
                    source,
                    selection,
                    EditKind::Format,
                );
                self.focus_editor = true;
            }
        }
    }
    fn apply_inline(&mut self, cx: &mut Cx, style: InlineStyle) {
        let surface = self.surface(cx);
        let actions = cx.capture_actions(|cx| {
            if let Some(mut surface) = surface.borrow_mut::<NotesTextSurface>() {
                surface.toggle_inline(cx, style);
            }
        });
        self.handle_surface_actions(cx, &actions);
    }
    fn undo_edit(&mut self, cx: &mut Cx, redo: bool) {
        if self.selected_is_deleted() || !self.mutations_allowed() {
            return;
        }
        self.flush_editor(cx);
        if let Some(session) = self.ui.editor.as_mut() {
            let changed = if redo {
                engine::redo(&mut self.doc, session, now_ms())
            } else {
                engine::undo(&mut self.doc, session, now_ms())
            };
            if changed {
                self.refresh_projection();
                self.save_now(cx, false);
                self.sync_surface(cx);
                self.redraw(cx);
            }
        }
    }
    fn selected_mutation(&mut self, cx: &mut Cx, command: LiveId) {
        if !self.mutations_allowed() {
            return;
        }
        self.flush_editor(cx);
        let Some(id) = self.ui.selected else {
            return;
        };
        let result = match command {
            live_id!(pin) => engine::pin_toggle(&mut self.doc, id).map(|_| ()),
            live_id!(delete) => engine::delete_note(&mut self.doc, &mut self.ui, id, now_ms()),
            live_id!(restore) => engine::restore_note(&mut self.doc, &mut self.ui, id),
            live_id!(notes) => engine::move_note(&mut self.doc, id, FolderId::Notes),
            live_id!(work) => engine::move_note(&mut self.doc, id, FolderId::Work),
            live_id!(personal) => engine::move_note(&mut self.doc, id, FolderId::Personal),
            _ => return,
        };
        match result {
            Ok(()) => {
                engine::normalize_after_filter(&self.doc, &mut self.ui);
                self.pending_nav = self.layout.is_compact();
                self.refresh_projection();
                self.save_now(cx, false);
            }
            Err(e) => self.edit_error = Some(e.into()),
        }
        self.dismiss_overlay(cx);
        self.redraw(cx);
    }
    fn open_overlay(&mut self, cx: &mut Cx, kind: OverlayKind, invoker: WidgetRef) {
        self.flush_editor(cx);
        self.overlay = kind;
        self.invoker = invoker;
        self.menu_closing = false;
        self.menu_animation = Some((
            Cx::time_now(),
            self.menu_opacity,
            1.0,
            if self.reduced_motion { 0.0 } else { 0.16 },
        ));
        self.menu_frame = cx.new_next_frame();
        self.sync_overlay(cx);
        self.redraw(cx);
    }
    fn dismiss_overlay(&mut self, cx: &mut Cx) {
        if self.overlay == OverlayKind::None {
            return;
        }
        self.menu_closing = true;
        self.menu_animation = Some((
            Cx::time_now(),
            self.menu_opacity,
            0.0,
            if self.reduced_motion { 0.0 } else { 0.12 },
        ));
        self.menu_frame = cx.new_next_frame();
        self.redraw(cx);
    }
    fn animate_menu(&mut self, cx: &mut Cx) {
        let Some((start, from, to, duration)) = self.menu_animation else {
            return;
        };
        let t = if duration == 0.0 {
            1.0
        } else {
            ((Cx::time_now() - start) / duration).clamp(0.0, 1.0)
        };
        self.menu_opacity = from + (to - from) * (1.0 - (1.0 - t).powi(3));
        if t == 1.0 {
            self.menu_animation = None;
            if self.menu_closing {
                self.overlay = OverlayKind::None;
                self.menu_closing = false;
                if !self.invoker.is_empty() {
                    cx.set_key_focus(self.invoker.area());
                }
                self.visible(cx, ids!(overlay), false);
            }
        } else {
            self.menu_frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }
    fn visible(&self, cx: &mut Cx, path: &[LiveId], visible: bool) {
        self.view.widget(cx, path).set_visible(cx, visible);
    }
    fn text(&self, cx: &mut Cx, path: &[LiveId], text: &str) {
        self.view.widget(cx, path).set_text(cx, text);
    }
    fn activated(&self, cx: &mut Cx, actions: &Actions, path: &[LiveId]) -> bool {
        let uid = self.view.widget(cx, path).widget_uid();
        matches!(actions.find_widget_action(uid).cast(), TapAction::Activated)
    }
    fn sync_status(&mut self, cx: &mut Cx) {
        let message = self
            .load_error
            .as_ref()
            .or(self.edit_error.as_ref())
            .or(self.save.error.as_ref());
        self.visible(cx, ids!(status), message.is_some());
        self.text(
            cx,
            ids!(status_text),
            message.map(String::as_str).unwrap_or(""),
        );
        self.visible(cx, ids!(loading), self.load == LoadState::Loading);
    }
    fn configure_folders(&mut self, cx: &mut Cx, parent: WidgetRef, compact: bool, overlay: bool) {
        for (index, (name, count)) in [
            live_id!(all),
            live_id!(notes),
            live_id!(work),
            live_id!(personal),
            live_id!(trash),
        ]
        .into_iter()
        .zip(engine::folder_counts(&self.doc))
        .enumerate()
        {
            let mut row = parent.widget(cx, &[name]);
            row.widget(cx, ids!(tap.name))
                .set_text(cx, count.collection.label());
            row.widget(cx, ids!(tap.count))
                .set_text(cx, &count.count.to_string());
            if let Some(mut selection) = row
                .widget(cx, ids!(selection))
                .borrow_mut::<NotesSelection>()
            {
                selection.set_selected(
                    cx,
                    !compact && self.ui.collection == count.collection,
                    self.reduced_motion,
                );
            }
            row.widget(cx, ids!(rule))
                .set_visible(cx, compact && index < 4);
            row.widget(cx, ids!(tap.chevron)).set_visible(cx, compact);
            let tap = row.widget(cx, ids!(tap));
            if let Some(mut tap) = tap.borrow_mut::<NotesTap>() {
                tap.nav_name = count.collection.label().into();
                tap.reduced_motion = self.reduced_motion;
            }
            let h = if compact { 52.0 } else { 44.0 };
            let font = if compact { 12.75 } else { 10.5 };
            script_apply_eval!(cx,row,{use mod.prelude.widgets.* height:#(h) tap +: {name +: {draw_text.text_style.font_size:#(font)}}});
            let mut count_widget = row.widget(cx, ids!(tap.count));
            if compact {
                script_apply_eval!(cx,count_widget,{use mod.prelude.widgets.* paper:theme.color_inset});
            } else if self.ui.collection == count.collection {
                script_apply_eval!(cx,count_widget,{use mod.prelude.widgets.* paper:theme.color_bg_highlight});
            } else if overlay {
                script_apply_eval!(cx,count_widget,{use mod.prelude.widgets.* paper:theme.color_bg_app});
            } else {
                script_apply_eval!(cx,count_widget,{use mod.prelude.widgets.* paper:theme.color_bg_container});
            }
        }
    }
    fn sync_chrome(&mut self, cx: &mut Cx) {
        self.view
            .children(&mut |_, child| set_control_motion(&child, self.reduced_motion));
        self.sync_status(cx);
        self.visible(
            cx,
            ids!(wide),
            !self.layout.is_compact() && self.load == LoadState::Ready,
        );
        self.visible(
            cx,
            ids!(compact_host),
            self.layout.is_compact() && self.load == LoadState::Ready,
        );
        if self.load != LoadState::Ready {
            return;
        }
        let compact = self.layout.is_compact();
        let screen = engine::current_screen(&self.ui);
        if !compact && self.layout.is_three() {
            let rows = self.view.widget(cx, ids!(wide.folders.rows));
            self.configure_folders(cx, rows, false, false);
        }
        if compact && screen == Screen::Folders {
            let rows = self
                .view
                .widget(cx, ids!(compact.root_view.scroll.group.rows));
            self.configure_folders(cx, rows, true, false);
        }
        let collection = self.ui.collection.label();
        let count = engine::list_hits(&self.doc, self.ui.collection, &self.ui.query).len();
        let count_text = format!("{count} {}", if count == 1 { "Note" } else { "Notes" });
        if !compact {
            self.text(cx, ids!(wide.notes_list.header), collection);
            self.text(
                cx,
                ids!(wide.notes_list.short_header.collection),
                collection,
            );
            self.text(cx, ids!(wide.notes_list.count), &count_text);
        } else if screen == Screen::List {
            self.text(cx, ids!(compact.list_view.screen.title), collection);
            self.text(cx, ids!(compact.list_view.screen.count), &count_text);
        }
        let search = self.view.widget(
            cx,
            if compact {
                ids!(compact.list_view.screen.dock.search)
            } else {
                ids!(wide.notes_list.search_slot.search)
            },
        );
        if !compact || screen == Screen::List {
            let field = search.text_input(cx, ids!(field));
            field.set_empty_text(cx, self.ui.collection.search_placeholder());
            if field.text() != self.ui.query {
                field.set_text(cx, &self.ui.query);
            }
            search
                .widget(cx, ids!(clear))
                .set_visible(cx, !self.ui.query.is_empty());
        }
        let has_note = self.ui.selected.is_some();
        let deleted = self.ui.collection == Collection::RecentlyDeleted;
        let allows_create = self.ui.collection.allows_create();
        if compact {
            self.visible(cx, ids!(compact.root_view.dock.compose), true);
            self.visible(
                cx,
                ids!(compact.list_view.screen.dock.compose),
                allows_create,
            );
        } else {
            self.visible(
                cx,
                ids!(wide.notes_list.short_header.compose),
                allows_create,
            );
        }
        if self.active_editor() {
            let editor = self.view.widget(
                cx,
                if compact {
                    ids!(compact.editor_view.screen)
                } else {
                    ids!(wide.editor)
                },
            );
            editor.widget(cx, ids!(document)).set_visible(cx, has_note);
            editor.widget(cx, ids!(empty)).set_visible(cx, !has_note);
            editor
                .widget(cx, ids!(empty.compose))
                .set_visible(cx, allows_create);
            let toolbar = editor.widget(cx, if compact { ids!(nav) } else { ids!(toolbar) });
            toolbar
                .widget(cx, ids!(back))
                .set_visible(cx, compact || deleted);
            toolbar
                .widget(cx, ids!(back.caption))
                .set_text(cx, collection);
            toolbar
                .widget(cx, ids!(restore))
                .set_visible(cx, deleted && has_note);
            toolbar
                .widget(cx, ids!(more))
                .set_visible(cx, !deleted && has_note);
            self.sync_surface(cx);
            let surface = self.surface(cx);
            let (selection, focused, marks) = surface
                .borrow::<NotesTextSurface>()
                .map(|s| (s.has_selection(), s.is_focused(cx), s.active_marks()))
                .unwrap_or_default();
            if compact {
                toolbar
                    .widget(cx, ids!(done))
                    .set_visible(cx, focused && !deleted);
                editor
                    .widget(cx, ids!(dock))
                    .set_visible(cx, !deleted && has_note);
                let tools = editor.widget(cx, ids!(dock.tools));
                for id in [live_id!(bold), live_id!(italic)] {
                    tools.widget(cx, &[id]).set_visible(cx, selection);
                }
                tools.widget(cx, ids!(compose)).set_visible(cx, !selection);
                for (id, on) in [
                    (live_id!(bold), marks.bold),
                    (live_id!(italic), marks.italic),
                ] {
                    if let Some(mut t) = tools.widget(cx, &[id]).borrow_mut::<NotesTap>() {
                        t.set_selected(cx, on);
                    }
                }
            } else {
                for id in [
                    live_id!(format),
                    live_id!(checklist),
                    live_id!(undo),
                    live_id!(redo),
                ] {
                    toolbar
                        .widget(cx, &[id])
                        .set_visible(cx, !deleted && has_note);
                }
                toolbar
                    .widget(cx, ids!(compose))
                    .set_visible(cx, !deleted && !self.layout.short_chrome);
            }
        }
        self.sync_overlay(cx);
    }
    fn sync_overlay(&mut self, cx: &mut Cx) {
        self.visible(cx, ids!(overlay), self.overlay != OverlayKind::None);
        if self.overlay == OverlayKind::None {
            return;
        }
        let moving = self.overlay == OverlayKind::Move;
        let more = self.overlay == OverlayKind::More;
        let format = self.overlay == OverlayKind::Format;
        let panel = self.view.widget(cx, ids!(overlay.panel));
        panel.widget(cx, ids!(rows)).set_visible(cx, more || moving);
        panel.widget(cx, ids!(formatting)).set_visible(cx, format);
        panel
            .widget(cx, ids!(folders))
            .set_visible(cx, self.overlay == OverlayKind::Folders);
        panel.widget(cx, ids!(heading)).set_visible(cx, moving);
        panel.widget(cx, ids!(heading)).set_text(cx, "Move Note");
        for id in [
            live_id!(pin),
            live_id!(move),
            live_id!(delete),
            live_id!(separator),
            live_id!(undo),
            live_id!(redo),
        ] {
            panel
                .widget(cx, &[live_id!(rows), id])
                .set_visible(cx, more);
        }
        for id in [
            live_id!(notes),
            live_id!(work),
            live_id!(personal),
            live_id!(cancel),
        ] {
            panel
                .widget(cx, &[live_id!(rows), id])
                .set_visible(cx, moving);
        }
        if let Some(note) = self.ui.selected.and_then(|id| self.doc.note(id)) {
            panel
                .widget(cx, ids!(rows.pin.caption))
                .set_text(cx, if note.pinned { "Unpin" } else { "Pin" });
            for (id, folder) in [
                (live_id!(notes), FolderId::Notes),
                (live_id!(work), FolderId::Work),
                (live_id!(personal), FolderId::Personal),
            ] {
                panel
                    .widget(cx, &[live_id!(rows), id, live_id!(current)])
                    .set_visible(cx, note.folder == folder);
            }
        }
        if self.overlay == OverlayKind::Folders {
            self.configure_folders(cx, panel.widget(cx, ids!(folders.rows)), false, true);
        }
        if format {
            let (marks, style) = self
                .surface(cx)
                .borrow::<NotesTextSurface>()
                .map(|s| (s.active_marks(), s.active_block()))
                .unwrap_or_default();
            for (id, selected) in [
                (live_id!(style_title), style == ParagraphStyle::Title),
                (live_id!(style_heading), style == ParagraphStyle::Heading),
                (live_id!(style_body), style == ParagraphStyle::Body),
                (live_id!(bold), marks.bold),
                (live_id!(italic), marks.italic),
                (live_id!(bullets), style == ParagraphStyle::Bullet),
                (
                    live_id!(numbers),
                    matches!(style, ParagraphStyle::Number(_)),
                ),
            ] {
                if let Some(mut tap) = panel.widget(cx, &[id]).borrow_mut::<NotesTap>() {
                    tap.set_selected(cx, selected);
                }
            }
        }
        let content = self.view.widget(cx, ids!(content)).area().rect(cx);
        let anchor = self.invoker.area().rect(cx);
        let requested = if format {
            370.0
        } else if self.overlay == OverlayKind::Folders {
            264.0
        } else {
            240.0
        };
        let width = requested.min((content.size.x - 16.0).max(44.0));
        let natural = if format {
            112.0
        } else if moving {
            224.0
        } else if more {
            237.0
        } else {
            236.0
        };
        let height = natural.min((content.size.y - 16.0).max(44.0));
        let mut position = menu_position(content, anchor, dvec2(width, height));
        position.y = (position.y + (1.0 - self.menu_opacity) * 6.0)
            .min((content.pos.y + content.size.y - height - 8.0).max(content.pos.y + 8.0));
        let mut panel = panel;
        let pad = if format { 12.0 } else { 8.0 };
        script_apply_eval!(cx,panel,{use mod.prelude.widgets.* width:#(width) height:#(height) abs_pos:#(position) padding:#(pad) draw_bg +: {layer_opacity:#(self.menu_opacity)} rows +: {height:Fill} formatting +: {height:Fill} folders +: {height:Fill}});
        menu_contents_opacity(cx, &panel, self.menu_opacity);
    }
    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let decision = engine::decide_layout(size.x, size.y, Some(self.layout));
        if size == self.size
            && decision == self.layout
            && self.applied_reduced_motion == self.reduced_motion
        {
            return;
        }
        let old = self.layout;
        if old.family != decision.family && self.active_editor() {
            self.transferred_surface = self.ui.selected.and_then(|id| {
                self.surface(cx)
                    .borrow::<NotesTextSurface>()
                    .map(|s| (id, s.state(cx)))
            });
            if self
                .transferred_surface
                .as_ref()
                .is_some_and(|(_, s)| s.focused)
            {
                cx.set_key_focus(Area::Empty);
            }
        }
        let old_list = self.list(cx).as_portal_list();
        let scroll = (
            old_list.first_id(),
            old_list.borrow().map(|l| l.first_scroll()).unwrap_or(0.0),
        );
        self.layout = decision;
        self.size = size;
        self.applied_reduced_motion = self.reduced_motion;
        self.visible(cx, ids!(wide.folders), decision.is_three());
        self.visible(cx, ids!(wide.divider_a), decision.is_three());
        self.visible(cx, ids!(wide.notes_list.header), decision.is_three());
        self.visible(cx, ids!(wide.notes_list.short_header), !decision.is_three());
        self.visible(cx, ids!(wide.notes_list.count), !decision.short_chrome);
        let width = engine::list_column_width(decision, size.x);
        let toolbar_height = if decision.short_chrome { 44.0 } else { 56.0 };
        let mut list = self.view.widget(cx, ids!(wide.notes_list));
        let search_height = if decision.short_chrome { 44.0 } else { 48.0 };
        let pad = if decision.short_chrome { 8.0 } else { 12.0 };
        script_apply_eval!(cx,list,{use mod.prelude.widgets.* width:#(width) search_slot +: {height:#(search_height) padding:Inset{left:#(pad) right:#(pad)}}});
        let mut toolbar = self.view.widget(cx, ids!(wide.editor.toolbar));
        script_apply_eval!(cx,toolbar,{use mod.prelude.widgets.* height:#(toolbar_height)});
        let mut surface = self.view.widget(cx, ids!(wide.editor.document));
        script_apply_eval!(cx,surface,{use mod.prelude.widgets.* short:#(decision.short_chrome)});
        if let Some(mut surface) = surface.borrow_mut::<NotesTextSurface>() {
            surface.invalidate_layout();
        }
        let duration = if self.reduced_motion { 0.0 } else { 0.26 };
        let offset = size.x + 1.0;
        for id in [live_id!(list_view), live_id!(editor_view)] {
            let mut page = self.view.widget(cx, &[live_id!(compact), id]);
            script_apply_eval!(cx,page,{use mod.prelude.widgets.* animator +: {slide: {hide: {from:{all:Play.Forward{duration:#(duration)}} apply:{offset:#(offset)}} show:{from:{all:Play.Forward{duration:#(duration)}}}}}});
        }
        if old.family != decision.family {
            self.list(cx)
                .as_portal_list()
                .set_first_id_and_scroll(scroll.0, scroll.1);
        }
        if decision.is_compact() {
            self.pending_nav = true;
        }
    }
    fn apply_pending_nav(&mut self, cx: &mut Cx) {
        if !self.pending_nav || !self.layout.is_compact() {
            return;
        }
        let nav = self.view.stack_navigation(cx, ids!(compact));
        if nav.stack_view_ids().iter().any(|id| {
            nav.view_by_id(cx, *id)
                .as_stack_navigation_view()
                .is_animating()
        }) {
            self.navigation_frame = cx.new_next_frame();
            return;
        }
        let desired = match engine::current_screen(&self.ui) {
            Screen::Folders => None,
            Screen::List => Some(live_id!(list_view)),
            Screen::Editor => Some(live_id!(editor_view)),
        };
        if nav.current_view() == desired {
            self.pending_nav = false;
            return;
        }
        match desired {
            Some(id) if nav.current_view() == Some(live_id!(editor_view)) => {
                nav.pop_to_view(cx, id)
            }
            Some(id) => nav.push(cx, id),
            None => nav.pop_to_root(cx),
        }
        self.pending_nav = false;
    }
    fn fill_list(&mut self, cx: &mut Cx2d, list: &mut PortalList, uid: WidgetUid) {
        self.rendered_rows
            .retain(|(presenter, _, _)| *presenter != uid);
        list.set_item_range(cx, 0, self.list_rows.len().max(1));
        while let Some(index) = list.next_visible_item(cx) {
            if self.list_rows.is_empty() {
                let item = list.item(cx, index, live_id!(Empty));
                item.widget(cx, ids!(title)).set_text(
                    cx,
                    if self.ui.query.is_empty() {
                        "No Notes"
                    } else {
                        "No Results"
                    },
                );
                item.widget(cx, ids!(explanation)).set_text(
                    cx,
                    if self.ui.query.is_empty() {
                        "Notes you add will appear here."
                    } else {
                        &self.ui.query
                    },
                );
                item.widget(cx, ids!(compose)).set_visible(
                    cx,
                    self.ui.collection.allows_create() && self.ui.query.is_empty(),
                );
                item.draw_all(cx, &mut Scope::empty());
                continue;
            }
            let Some(row) = self.list_rows.get(index) else {
                continue;
            };
            match row {
                ListRow::Section(section) => {
                    let mut item = list.item(cx, *section as usize, live_id!(Section));
                    item.set_text(cx, section.label());
                    let h = if self.layout.is_compact() {
                        32.0
                    } else if self.layout.short_chrome {
                        24.0
                    } else {
                        28.0
                    };
                    let font = if self.layout.is_compact() { 11.25 } else { 9.0 };
                    script_apply_eval!(cx,item,{use mod.prelude.widgets.* height:#(h) draw_text.text_style.font_size:#(font)});
                    item.draw_all(cx, &mut Scope::empty());
                }
                ListRow::Note(hit) => {
                    let stable_id =
                        16 + self.doc.notes.iter().position(|n| n.id == hit.id).unwrap();
                    let mut item = list.item(cx, stable_id, live_id!(Item));
                    let h = if self.layout.is_compact() {
                        88.0
                    } else if self.layout.short_chrome {
                        64.0
                    } else {
                        80.0
                    };
                    let font = if self.layout.is_compact() {
                        12.75
                    } else {
                        12.0
                    };
                    let meta = if self.layout.is_compact() {
                        11.25
                    } else {
                        9.75
                    };
                    let pad = if self.layout.is_compact() { 16.0 } else { 20.0 };
                    let top = if self.layout.is_compact() { 12.0 } else { 10.0 };
                    let small = if self.layout.is_compact() { 9.75 } else { 9.0 };
                    script_apply_eval!(cx,item,{use mod.prelude.widgets.* height:#(h) tap +: {padding:Inset{left:#(pad) right:#(pad) top:#(top) bottom:#(top)} title +: {draw_text.text_style.font_size:#(font)} preview +: {draw_text.text_style.font_size:#(meta)} metadata +: {date +: {draw_text.text_style.font_size:#(small)} folder +: {draw_text.text_style.font_size:#(small)}}}});
                    item.widget(cx, ids!(tap.title)).set_text(cx, &hit.title);
                    let date = engine::format_note_date(hit.edited_ms, now_ms());
                    item.widget(cx, ids!(tap.preview)).set_text(
                        cx,
                        &if self.layout.short_chrome {
                            format!("{date}   {}", hit.preview)
                        } else {
                            hit.preview.clone()
                        },
                    );
                    item.widget(cx, ids!(tap.metadata))
                        .set_visible(cx, !self.layout.short_chrome);
                    item.widget(cx, ids!(tap.metadata.date)).set_text(cx, &date);
                    item.widget(cx, ids!(tap.metadata.folder))
                        .set_text(cx, hit.folder.label());
                    let selected = self.ui.selected == Some(hit.id);
                    for path in [
                        ids!(tap.preview).as_slice(),
                        ids!(tap.metadata.date).as_slice(),
                        ids!(tap.metadata.folder).as_slice(),
                    ] {
                        let mut meta = item.widget(cx, path);
                        if selected {
                            script_apply_eval!(cx,meta,{use mod.prelude.widgets.* paper:theme.color_bg_highlight});
                        } else {
                            script_apply_eval!(cx,meta,{use mod.prelude.widgets.* paper:theme.color_bg_app});
                        }
                    }
                    if let Some(mut selection) = item
                        .widget(cx, ids!(selection))
                        .borrow_mut::<NotesSelection>()
                    {
                        selection.set_selected(cx, selected, self.reduced_motion);
                    }
                    item.widget(cx, ids!(rule)).set_visible(cx, !selected);
                    let tap = item.widget(cx, ids!(tap));
                    if let Some(mut tap) = tap.borrow_mut::<NotesTap>() {
                        tap.nav_name = format!("{} · {}", hit.title, hit.id);
                        tap.reduced_motion = self.reduced_motion;
                    }
                    self.rendered_rows.push((uid, tap.widget_uid(), hit.id));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
    }
    fn handle_surface_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if !self.active_editor() {
            return;
        }
        let uid = self.surface(cx).widget_uid();
        for action in actions {
            let Some(a) = action.as_widget_action() else {
                continue;
            };
            if a.widget_uid != uid {
                continue;
            }
            match a.cast() {
                SurfaceAction::Edit {
                    id,
                    revision,
                    source,
                    selection,
                    kind,
                } => self.accept_source(cx, id, revision, source, selection, kind),
                SurfaceAction::Selection { id, selection } => {
                    if let Some(session) = self.ui.editor.as_mut() {
                        if session.note_id == id {
                            session.selection = selection;
                        }
                    }
                }
                SurfaceAction::Check {
                    id,
                    revision,
                    line_start,
                } => {
                    if let Some(session) = self.ui.editor.as_ref() {
                        if let Some(note) = self.doc.note(session.note_id) {
                            if let Some(source) = engine::toggle_checkbox(
                                &note.source,
                                line_start,
                                note.id,
                                id,
                                self.doc.revision,
                                revision,
                            ) {
                                self.accept_source(
                                    cx,
                                    id,
                                    revision,
                                    source,
                                    session.selection,
                                    EditKind::Check,
                                );
                            }
                        }
                    }
                }
                SurfaceAction::Focus => {
                    self.ui.route = vec![Screen::Folders, Screen::List, Screen::Editor];
                    self.redraw(cx);
                }
                SurfaceAction::Undo => self.undo_edit(cx, false),
                SurfaceAction::Redo => self.undo_edit(cx, true),
                SurfaceAction::None => {}
            }
        }
    }
    fn folder_actions(&mut self, cx: &mut Cx, actions: &Actions, parent: &[LiveId]) -> bool {
        let parent = self.view.widget(cx, parent);
        for (name, collection) in [
            live_id!(all),
            live_id!(notes),
            live_id!(work),
            live_id!(personal),
            live_id!(trash),
        ]
        .into_iter()
        .zip(Collection::ALL)
        {
            let uid = parent.widget(cx, &[name, live_id!(tap)]).widget_uid();
            if matches!(actions.find_widget_action(uid).cast(), TapAction::Activated) {
                self.open_collection(cx, collection);
                return true;
            }
        }
        false
    }
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.activated(cx, actions, ids!(status.retry)) {
            if self.load == LoadState::Error {
                if let Some(storage) = self.storage.clone() {
                    self.load_error = None;
                    self.load = LoadState::Loading;
                    self.load_req = Some(storage.get(cx, STORAGE_KEY));
                }
            } else if self.edit_error.take().is_some() {
                self.sync_surface(cx);
            } else {
                let effect = reduce_save(&mut self.save, SaveEvent::Retry { now_ms: now_ms() });
                self.apply_save_effect(cx, effect);
            }
            self.sync_status(cx);
            self.redraw(cx);
            return;
        }
        self.handle_surface_actions(cx, actions);
        if self.menu_closing {
            return;
        }
        if self.overlay != OverlayKind::None {
            if self.activated(cx, actions, ids!(overlay.dismiss)) {
                self.dismiss_overlay(cx);
                return;
            }
            match self.overlay {
                OverlayKind::Folders => {
                    self.folder_actions(cx, actions, ids!(overlay.panel.folders.rows));
                }
                OverlayKind::More | OverlayKind::Move => {
                    for command in [
                        live_id!(pin),
                        live_id!(delete),
                        live_id!(notes),
                        live_id!(work),
                        live_id!(personal),
                    ] {
                        if self.activated(
                            cx,
                            actions,
                            &[live_id!(overlay), live_id!(panel), live_id!(rows), command],
                        ) {
                            self.selected_mutation(cx, command);
                            return;
                        }
                    }
                    if self.activated(cx, actions, ids!(overlay.panel.rows.move)) {
                        self.overlay = OverlayKind::Move;
                        self.sync_overlay(cx);
                        self.redraw(cx);
                    }
                    if self.activated(cx, actions, ids!(overlay.panel.rows.cancel)) {
                        self.dismiss_overlay(cx);
                    }
                    for (id, redo) in [(live_id!(undo), false), (live_id!(redo), true)] {
                        if self.activated(
                            cx,
                            actions,
                            &[live_id!(overlay), live_id!(panel), live_id!(rows), id],
                        ) {
                            self.undo_edit(cx, redo);
                            self.dismiss_overlay(cx);
                        }
                    }
                }
                OverlayKind::Format => {
                    for (id, style) in [
                        (live_id!(style_title), BlockStyle::Title),
                        (live_id!(style_heading), BlockStyle::Heading),
                        (live_id!(style_body), BlockStyle::Body),
                        (live_id!(bullets), BlockStyle::Bullets),
                        (live_id!(numbers), BlockStyle::Numbers),
                    ] {
                        if self.activated(
                            cx,
                            actions,
                            &[live_id!(overlay), live_id!(panel), live_id!(formatting), id],
                        ) {
                            self.apply_block(cx, style);
                        }
                    }
                    for (id, style) in [
                        (live_id!(bold), InlineStyle::Bold),
                        (live_id!(italic), InlineStyle::Italic),
                    ] {
                        if self.activated(
                            cx,
                            actions,
                            &[live_id!(overlay), live_id!(panel), live_id!(formatting), id],
                        ) {
                            self.apply_inline(cx, style);
                        }
                    }
                    self.sync_overlay(cx);
                }
                _ => {}
            }
            return;
        }
        if self.load != LoadState::Ready {
            return;
        }
        let compact = self.layout.is_compact();
        let screen = engine::current_screen(&self.ui);
        if compact && screen == Screen::Folders {
            if self.folder_actions(cx, actions, ids!(compact.root_view.scroll.group.rows)) {
                return;
            }
            if self.activated(cx, actions, ids!(compact.root_view.dock.compose)) {
                if !self.ui.collection.allows_create() {
                    self.ui.collection = Collection::All;
                }
                self.new_note(cx);
            }
        }
        if !compact && self.layout.is_three() {
            if self.folder_actions(cx, actions, ids!(wide.folders.rows)) {
                return;
            }
        }
        if !compact || screen == Screen::List {
            let list_uid = self.list(cx).widget_uid();
            let clicked = self
                .rendered_rows
                .iter()
                .find(|(presenter, uid, _)| {
                    *presenter == list_uid
                        && matches!(
                            actions.find_widget_action(*uid).cast(),
                            TapAction::Activated
                        )
                })
                .map(|(_, _, id)| *id);
            if let Some(id) = clicked {
                self.open_note(cx, id);
                return;
            }
            // Empty-state controls belong to this explicit presenter too.
            if self.list_rows.is_empty() {
                let empty = self.list(cx).as_portal_list().item(cx, 0, live_id!(Empty));
                if matches!(
                    actions
                        .find_widget_action(empty.widget(cx, ids!(compose)).widget_uid())
                        .cast(),
                    TapAction::Activated
                ) {
                    self.new_note(cx);
                    return;
                }
            }
            let search_path: &[LiveId] = if compact {
                ids!(compact.list_view.screen.dock.search)
            } else {
                ids!(wide.notes_list.search_slot.search)
            };
            let search = self.view.widget(cx, search_path);
            let mut query = search.text_input(cx, ids!(field)).changed(actions);
            if matches!(
                actions
                    .find_widget_action(search.widget(cx, ids!(clear)).widget_uid())
                    .cast(),
                TapAction::Activated
            ) {
                query = Some(String::new());
            }
            if let Some(query) = query {
                self.flush_editor(cx);
                engine::set_query(&self.doc, &mut self.ui, query);
                engine::open_selected_read(&self.doc, &mut self.ui);
                self.refresh_projection();
                self.list(cx)
                    .as_portal_list()
                    .set_first_id_and_scroll(0, 0.0);
                self.redraw(cx);
            }
            let compose: &[LiveId] = if compact {
                ids!(compact.list_view.screen.dock.compose)
            } else {
                ids!(wide.notes_list.short_header.compose)
            };
            if self.activated(cx, actions, compose) {
                self.new_note(cx);
                return;
            }
            if compact && self.activated(cx, actions, ids!(compact.list_view.screen.nav.back)) {
                self.go_back(cx);
                return;
            }
            if !compact && self.activated(cx, actions, ids!(wide.notes_list.short_header.folders)) {
                let invoker = self
                    .view
                    .widget(cx, ids!(wide.notes_list.short_header.folders));
                self.open_overlay(cx, OverlayKind::Folders, invoker);
                return;
            }
        }
        if self.active_editor() {
            let editor = self.view.widget(
                cx,
                if compact {
                    ids!(compact.editor_view.screen)
                } else {
                    ids!(wide.editor)
                },
            );
            let toolbar = editor.widget(cx, if compact { ids!(nav) } else { ids!(toolbar) });
            let activated = |cx: &mut Cx, root: &WidgetRef, id: LiveId| {
                matches!(
                    actions
                        .find_widget_action(root.widget(cx, &[id]).widget_uid())
                        .cast(),
                    TapAction::Activated
                )
            };
            if activated(cx, &toolbar, live_id!(back)) {
                if !compact && self.ui.collection == Collection::RecentlyDeleted {
                    self.open_collection(cx, Collection::All);
                } else {
                    self.go_back(cx);
                }
                return;
            }
            if activated(cx, &toolbar, live_id!(restore)) {
                self.selected_mutation(cx, live_id!(restore));
                return;
            }
            if activated(cx, &toolbar, live_id!(done)) {
                self.flush_editor(cx);
                if let Some(mut surface) = self.surface(cx).borrow_mut::<NotesTextSurface>() {
                    surface.finish(cx);
                }
                self.redraw(cx);
                return;
            }
            if activated(cx, &toolbar, live_id!(more)) {
                let invoker = toolbar.widget(cx, ids!(more));
                self.open_overlay(cx, OverlayKind::More, invoker);
                return;
            }
            if self.ui.collection == Collection::RecentlyDeleted {
                return;
            }
            let tools = if compact {
                editor.widget(cx, ids!(dock.tools))
            } else {
                toolbar
            };
            if activated(cx, &tools, live_id!(format)) {
                let invoker = tools.widget(cx, ids!(format));
                self.open_overlay(cx, OverlayKind::Format, invoker);
                return;
            }
            if activated(cx, &tools, live_id!(compose))
                || activated(cx, &editor.widget(cx, ids!(empty)), live_id!(compose))
            {
                self.new_note(cx);
                return;
            }
            if activated(cx, &tools, live_id!(checklist)) {
                self.apply_block(cx, BlockStyle::Checklist);
            }
            for (id, redo) in [(live_id!(undo), false), (live_id!(redo), true)] {
                if activated(cx, &tools, id) {
                    self.undo_edit(cx, redo);
                }
            }
            for (id, style) in [
                (live_id!(bold), InlineStyle::Bold),
                (live_id!(italic), InlineStyle::Italic),
            ] {
                if activated(cx, &tools, id) {
                    self.apply_inline(cx, style);
                }
            }
        }
    }
}

fn set_control_motion(widget: &WidgetRef, reduced_motion: bool) {
    if let Some(mut tap) = widget.borrow_mut::<NotesTap>() {
        tap.reduced_motion = reduced_motion;
    }
    widget.children(&mut |_, child| set_control_motion(&child, reduced_motion));
}
fn collect_menu_taps(widget: &WidgetRef, taps: &mut Vec<WidgetRef>) {
    if !widget.visible() {
        return;
    }
    if widget.borrow::<NotesTap>().is_some_and(|t| t.enabled) {
        taps.push(widget.clone());
        return;
    }
    widget.children(&mut |_, child| collect_menu_taps(&child, taps));
}
fn menu_contents_opacity(cx: &mut Cx, widget: &WidgetRef, opacity: f64) {
    let mut children = Vec::new();
    widget.children(&mut |_, child| children.push(child));
    for mut child in children {
        if let Some(mut label) = child.borrow_mut::<Label>() {
            label.draw_text.color.w = opacity as f32;
        }
        if let Some(mut meta) = child.borrow_mut::<NotesMeta>() {
            meta.opacity = opacity;
        }
        if let Some(mut tap) = child.borrow_mut::<NotesTap>() {
            tap.opacity = opacity;
        }
        if let Some(mut rule) = child.borrow_mut::<NotesRule>() {
            rule.opacity = opacity;
        }
        if child.borrow::<Icon>().is_some() {
            script_apply_eval!(cx,child,{use mod.prelude.widgets.* draw_icon +: {opacity:#(opacity)}});
        }
        menu_contents_opacity(cx, &child, opacity);
    }
}
/// Absolute panel coordinates in the host's current content rectangle.
fn menu_position(viewport: Rect, anchor: Rect, size: Vec2d) -> Vec2d {
    let low = viewport.pos + dvec2(8.0, 8.0);
    let high = viewport.pos + viewport.size - size - dvec2(8.0, 8.0);
    let below = anchor.pos.y + anchor.size.y + 4.0;
    let y = if below + size.y <= viewport.pos.y + viewport.size.y - 8.0 {
        below
    } else {
        anchor.pos.y - size.y - 4.0
    };
    dvec2(
        (anchor.pos.x + anchor.size.x - size.x).clamp(low.x, high.x.max(low.x)),
        y.clamp(low.y, high.y.max(low.y)),
    )
}
impl Widget for NotesView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        self.animate_menu(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if self
            .debounce
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            let effect = reduce_save(&mut self.save, SaveEvent::Timer { now_ms: now_ms() });
            self.apply_save_effect(cx, effect);
            self.sync_status(cx);
        }
        if let Event::KeyDown(key) = event {
            if self.overlay != OverlayKind::None && key.key_code == KeyCode::Tab {
                let mut taps = Vec::new();
                collect_menu_taps(&self.view.widget(cx, ids!(overlay.panel)), &mut taps);
                if !taps.is_empty() {
                    let current = taps.iter().position(|w| cx.has_key_focus(w.area()));
                    let index = if key.modifiers.shift {
                        current.map_or(taps.len() - 1, |i| (i + taps.len() - 1) % taps.len())
                    } else {
                        current.map_or(0, |i| (i + 1) % taps.len())
                    };
                    cx.set_key_focus(taps[index].area());
                }
                return;
            }
        }
        if matches!(event,Event::KeyDown(e) if e.key_code==KeyCode::Escape)
            && self.overlay != OverlayKind::None
        {
            self.dismiss_overlay(cx);
            return;
        }
        if matches!(event, Event::BackPressed { .. }) && event.back_pressed() {
            if self.overlay != OverlayKind::None {
                self.dismiss_overlay(cx);
                return;
            }
            if self.layout.is_compact() && engine::current_screen(&self.ui) != Screen::Folders {
                self.go_back(cx);
                self.apply_pending_nav(cx);
                return;
            }
        }
        self.sync_chrome(cx);
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
        let actions = cx.capture_actions(|cx| {
            if self.overlay != OverlayKind::None {
                self.view
                    .widget(cx, ids!(overlay))
                    .handle_event(cx, event, scope);
            } else if self.load == LoadState::Ready {
                if self.layout.is_compact() {
                    self.view
                        .widget(cx, ids!(compact))
                        .handle_event(cx, event, scope);
                } else {
                    self.view
                        .widget(cx, ids!(wide))
                        .handle_event(cx, event, scope);
                }
            }
            self.view
                .widget(cx, ids!(status))
                .handle_event(cx, event, scope);
        });
        self.handle_actions(cx, &actions);
        // Stack completion actions go back to the stock stack. Pop is consumed
        // once by this adapter so the stock handler cannot also pop to root.
        let mut pop = false;
        let transition_actions = cx.capture_actions(|cx| {
            for action in &actions {
                if let Some(a) = action.as_widget_action() {
                    match a.cast::<StackNavigationAction>() {
                        StackNavigationAction::Pop => pop = true,
                        _ => {}
                    }
                    match a.cast::<StackNavigationTransitionAction>() {
                        StackNavigationTransitionAction::ShowDone => cx
                            .widget_action(a.widget_uid, StackNavigationTransitionAction::ShowDone),
                        StackNavigationTransitionAction::HideEnd(uid) => cx.widget_action(
                            a.widget_uid,
                            StackNavigationTransitionAction::HideEnd(uid),
                        ),
                        _ => {}
                    }
                }
            }
        });
        if !transition_actions.is_empty() {
            self.view.widget(cx, ids!(compact)).handle_event(
                cx,
                &Event::Actions(transition_actions),
                scope,
            );
        }
        if pop {
            self.go_back(cx);
        }
        self.apply_pending_nav(cx);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        self.apply_layout(cx, size);
        self.sync_chrome(cx);
        self.apply_pending_nav(cx);
        let wide_list = self
            .view
            .widget(cx, ids!(wide.notes_list.rows))
            .widget_uid();
        let compact_list = self
            .view
            .widget(cx, ids!(compact.list_view.screen.rows))
            .widget_uid();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let uid = step.widget_uid();
            if (uid == wide_list && !self.layout.is_compact())
                || (uid == compact_list && self.layout.is_compact())
            {
                if let Some(mut list) = step.as_portal_list().borrow_mut() {
                    self.fill_list(cx, &mut list, uid);
                }
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panels_clamp_to_nonzero_host_viewports() {
        let viewport = Rect {
            pos: dvec2(130.0, 80.0),
            size: dvec2(402.0, 300.0),
        };
        let size = dvec2(370.0, 212.0);
        for point in [
            viewport.pos,
            viewport.pos + viewport.size - dvec2(44.0, 44.0),
        ] {
            let pos = menu_position(
                viewport,
                Rect {
                    pos: point,
                    size: dvec2(44.0, 44.0),
                },
                size,
            );
            assert!(pos.x >= 138.0 && pos.y >= 88.0);
            assert!(pos.x + size.x <= 524.0 && pos.y + size.y <= 372.0);
        }
    }
    #[test]
    fn layout_records_occupied_columns_and_full_compact_pages_without_a_gpu() {
        fn draw(cx: &mut Cx, root: &WidgetRef, size: Vec2d) {
            let pass = DrawPass::new(cx);
            pass.set_size(cx, size);
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            let mut list = DrawList2d::new(cx);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            list.begin_always(&mut cx2d);
            overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_overlay());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            overlay.end(&mut cx2d);
            list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            let value = script_eval!(vm,{use mod.widgets.* NotesView{reduced_motion:true}});
            WidgetRef::script_from_value(vm, value)
        });
        {
            let mut view = root.borrow_mut::<NotesView>().unwrap();
            view.started = true;
            view.doc = seed::generate(1_788_955_200_000);
            view.load = LoadState::Ready;
            view.ui = engine::initial_ui(&view.doc);
            view.refresh_projection();
        }
        for size in [dvec2(1240.0, 800.0), dvec2(874.0, 300.0)] {
            draw(&mut cx, &root, size);
            let view = root.borrow::<NotesView>().unwrap();
            assert!(
                !view.rendered_rows.is_empty(),
                "the visible list must be populated at {size:?}"
            );
            let list = view
                .view
                .widget(&mut cx, ids!(wide.notes_list))
                .area()
                .rect(&cx);
            let editor = view
                .view
                .widget(&mut cx, ids!(wide.editor))
                .area()
                .rect(&cx);
            assert_eq!(list.size.x, if size.x == 1240.0 { 320.0 } else { 280.0 });
            assert_eq!(editor.pos.x, if size.x == 1240.0 { 546.0 } else { 281.0 });
            assert_eq!(editor.size.y, size.y);
            // StackNavigation itself does not expose visibility. The app's
            // wrapper must prevent the phone folder tree from drawing at all.
            assert!(!root.widget(&mut cx, ids!(compact_host)).visible());
            assert!(!root.widget(&mut cx, ids!(compact.root_view.scroll.title)).area().is_valid(&cx));
            assert!(!root.widget(&mut cx, ids!(compact.root_view.scroll.group.rows.all.tap.icon)).area().is_valid(&cx));
            let presenter = view.list(&mut cx).as_portal_list();
            for (_, _, id) in &view.rendered_rows {
                if Some(*id) == view.ui.selected { continue; }
                let stable = 16 + view.doc.notes.iter().position(|n| n.id == *id).unwrap();
                let item = presenter.item(&mut cx, stable, live_id!(Item));
                let row = item.area().rect(&cx);
                let rule = item.widget(&mut cx, ids!(rule)).area().rect(&cx);
                assert!(rule.pos.x >= row.pos.x + 20.0, "{row:?} {rule:?}");
                assert!((rule.pos.y + rule.size.y - row.pos.y - row.size.y).abs() < 0.01,
                    "separator must follow its own row: {row:?} {rule:?}");
            }
            drop(view);
            for (kind, path) in [
                (OverlayKind::More, ids!(wide.editor.toolbar.more).as_slice()),
                (OverlayKind::Move, ids!(wide.editor.toolbar.more).as_slice()),
                (
                    OverlayKind::Format,
                    ids!(wide.editor.toolbar.format).as_slice(),
                ),
            ] {
                let invoker = root.widget(&mut cx, path);
                let anchor = invoker.area().rect(&cx);
                assert!(
                    anchor.size.x >= 44.0 && anchor.size.y >= 44.0,
                    "menu invokers have finite touch targets: {anchor:?}"
                );
                {
                    let mut view = root.borrow_mut::<NotesView>().unwrap();
                    view.open_overlay(&mut cx, kind, invoker);
                    view.animate_menu(&mut cx);
                }
                draw(&mut cx, &root, size);
                let panel = root.widget(&mut cx, ids!(overlay.panel)).area().rect(&cx);
                assert!(
                    panel.pos.x >= 8.0
                        && panel.pos.y >= 8.0
                        && panel.pos.x + panel.size.x <= size.x - 8.0
                        && panel.pos.y + panel.size.y <= size.y - 8.0,
                    "panel is clamped: {panel:?}"
                );
                {
                    let mut view = root.borrow_mut::<NotesView>().unwrap();
                    view.dismiss_overlay(&mut cx);
                    view.animate_menu(&mut cx);
                }
            }
        }
        let size = dvec2(402.0, 780.0);
        draw(&mut cx, &root, size);
        let folders = root
            .widget(&mut cx, ids!(compact.root_view))
            .area()
            .rect(&cx);
        assert_eq!(folders.size, size);
        assert!(root.widget(&mut cx, ids!(compact_host)).visible());
        for id in [live_id!(all), live_id!(notes), live_id!(work), live_id!(personal)] {
            let item = root.widget(&mut cx, &[live_id!(compact), live_id!(root_view),
                live_id!(scroll), live_id!(group), live_id!(rows), id]);
            let row = item.area().rect(&cx);
            let rule = item.widget(&mut cx, ids!(rule)).area().rect(&cx);
            assert_eq!(row.size.y, 52.0);
            assert!(rule.pos.x >= row.pos.x + 52.0);
            assert!((rule.pos.y + rule.size.y - row.pos.y - row.size.y).abs() < 0.01,
                "folder rule must follow its own row: {row:?} {rule:?}");
        }
        for (screen, page) in [
            (Screen::List, live_id!(list_view)),
            (Screen::Editor, live_id!(editor_view)),
        ] {
            {
                let mut view = root.borrow_mut::<NotesView>().unwrap();
                view.ui.route = if screen == Screen::List {
                    vec![Screen::Folders, Screen::List]
                } else {
                    vec![Screen::Folders, Screen::List, Screen::Editor]
                };
                view.pending_nav = true;
                view.apply_pending_nav(&mut cx);
            }
            let nav = root.stack_navigation(&mut cx, ids!(compact));
            let page_ref = nav.view_by_id(&mut cx, page);
            page_ref
                .as_stack_navigation_view()
                .show_at_rest(&mut cx, size.x);
            let actions = cx.capture_actions(|cx| {
                cx.widget_action(
                    page_ref.widget_uid(),
                    StackNavigationTransitionAction::ShowDone,
                )
            });
            root.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
            draw(&mut cx, &root, size);
            let rect = page_ref.widget(&mut cx, ids!(screen)).area().rect(&cx);
            assert_eq!(rect.pos, Vec2d::default());
            assert_eq!(rect.size, size);
            if screen == Screen::List {
                let search = page_ref
                    .widget(&mut cx, ids!(screen.dock.search))
                    .area()
                    .rect(&cx);
                assert!(
                    search.pos.y >= size.y - 72.0,
                    "search stays in the bottom dock: {search:?}"
                );
            }
        }
    }
}
