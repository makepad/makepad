//! The AI music page's song list: every song of the conversation, newest
//! first, in a virtualised list (a `PortalList` named `list` with an
//! `AiRow` template, laid out in main.rs): only the rows in view are drawn,
//! it scrolls by wheel, scroll bar and keys (Home, End, Page Up/Down,
//! arrows). A song made while the person is scrolled down the list goes in
//! at the top without moving what they look at.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiSongsBase = #(AiSongs::register_widget(vm))
    mod.widgets.AiSongs = set_type_default() do mod.widgets.AiSongsBase{
        width: Fill
        height: Fill
    }
}

/// One row as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct AiRowView {
    /// Stable across updates: the song's file, or "making" for the one
    /// being made.
    pub key: String,
    pub num: String,
    pub title: String,
    pub style: String,
    pub length: String,
    pub status: String,
    pub running: bool,
    pub playable: bool,
    /// The row's background (picked, just made, being made).
    pub tint: Vec4f,
    /// The progress bar of the song being made: progress, sheen phase,
    /// fill and track colours.
    pub bar: Option<(f64, f64, Vec4f, Vec4f)>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiSongs {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<AiRowView>,
}

impl Widget for AiSongs {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else { continue };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(index) else { continue };
                let item = list.item(cx, index, live_id!(AiRow));
                for (id, text) in [(ids!(num), &row.num), (ids!(title), &row.title), (ids!(style), &row.style), (ids!(length), &row.length), (ids!(status), &row.status)] {
                    let label = item.label(cx, id);
                    if label.text() != *text {
                        label.set_text(cx, text);
                    }
                }
                item.widget(cx, ids!(play_cap)).set_visible(cx, row.playable);
                item.widget(cx, ids!(cancel_cap)).set_visible(cx, row.running);
                item.widget(cx, ids!(restart_cap)).set_visible(cx, row.running);
                let mut bar = item.widget(cx, ids!(bar));
                bar.set_visible(cx, row.bar.is_some());
                if let Some((progress, phase, fill, track)) = row.bar {
                    script_apply_eval!(cx, bar, {draw_bg +: {progress: #(progress) phase: #(phase) fill: #(fill) track: #(track)}});
                }
                let tint = row.tint;
                let mut bg = item.clone();
                script_apply_eval!(cx, bg, {draw_bg +: {color: #(tint)}});
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

impl AiSongsRef {
    /// The rows, newest first. When the list is scrolled away from the top,
    /// the row at its top stays there (a new song goes in above it).
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<AiRowView>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.rows == rows {
            return;
        }
        let list = inner.view.portal_list(cx, ids!(list));
        let first = list.first_id();
        if first > 0 {
            if let Some(key) = inner.rows.get(first).map(|r| r.key.clone()) {
                if let Some(now) = rows.iter().position(|r| r.key == key) {
                    if now != first {
                        let scroll = list.borrow().map_or(0.0, |l| l.first_scroll());
                        list.set_first_id_and_scroll_in_place(now, scroll);
                    }
                }
            }
        }
        inner.rows = rows;
        inner.view.redraw(cx);
    }

}
