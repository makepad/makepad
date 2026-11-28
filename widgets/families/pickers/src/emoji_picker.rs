//! A keyboard- and pointer-operated emoji grid for the content of a Popover.

use crate::{event::TAP_COUNT_DISTANCE, makepad_derive_widget::*, makepad_draw::*, widget::*};

const SECTIONS: &[(&str, &[&str])] = &[
    ("Smileys", &[
        "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🤣",
        "😊", "😇", "🙂", "🙃", "😉", "😌", "😍", "🥰",
        "😘", "😋", "😛", "😜", "🤪", "😎", "🤓", "🧐",
        "🤔", "🤭", "🤫", "🤗", "😏", "😐", "🙄", "😬",
        "😮", "😴", "😢", "😭", "😤", "😡", "🤯", "🥳",
    ]),
    ("People", &[
        "👋", "🤚", "🖐️", "✋", "🖖", "👌", "✌️", "🤞",
        "🤟", "🤘", "🤙", "👈", "👉", "👆", "👇", "☝️",
        "👍", "👎", "✊", "👊", "🤛", "🤜", "👏", "🙌",
        "👐", "🤲", "🤝", "🙏", "✍️", "💪", "👀", "🧠",
    ]),
    ("Animals and nature", &[
        "🐶", "🐱", "🐭", "🐹", "🐰", "🦊", "🐻", "🐼",
        "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐧",
        "🐦", "🦉", "🦆", "🦋", "🐝", "🐢", "🐙", "🐬",
        "🐳", "🦔", "🌵", "🌲", "🌳", "🌱", "🌿", "🍀",
        "🌻", "🌷", "🌹", "🍄", "☀️", "🌙", "⭐", "🌈",
    ]),
    ("Food and drink", &[
        "🍏", "🍎", "🍐", "🍊", "🍋", "🍌", "🍉", "🍇",
        "🍓", "🍒", "🍑", "🍍", "🥝", "🥑", "🥦", "🥕",
        "🌽", "🍅", "🥐", "🍞", "🧀", "🥚", "🥞", "🍔",
        "🍟", "🍕", "🌮", "🍜", "🍣", "🍙", "🍿", "🍪",
        "🎂", "🍩", "🍫", "🍦", "☕", "🍵", "🥛", "🍹",
    ]),
    ("Activities", &[
        "⚽", "🏀", "🏈", "⚾", "🎾", "🏐", "🏉", "🎱",
        "🏓", "🏸", "🥅", "⛳", "🏹", "🎣", "🥊", "🎿",
        "🛷", "🏆", "🥇", "🎯", "🎮", "🎲", "🎳", "🎨",
        "🎭", "🎤", "🎧", "🎸", "🎹", "🥁", "🎻", "🎬",
    ]),
    ("Travel and places", &[
        "🚗", "🚕", "🚙", "🚌", "🚎", "🏎️", "🚓", "🚑",
        "🚒", "🚚", "🚜", "🚲", "🛴", "🏍️", "🚂", "🚆",
        "🚇", "🚉", "🚦", "⛽", "⚓", "⛵", "🛳️", "✈️",
        "🚁", "🚀", "🛰️", "🏕️", "⛰️", "🏝️", "🌋", "🌍",
    ]),
    ("Objects", &[
        "⌚", "📱", "💻", "⌨️", "🖥️", "🖨️", "🖱️", "📷",
        "🎥", "📺", "📻", "⏰", "💡", "🔦", "🔋", "🔌",
        "🧭", "🧲", "🔑", "🔒", "🔨", "🔧", "⚙️", "✂️",
        "📌", "📎", "📏", "✏️", "📝", "📖", "📚", "📦",
        "🎁", "🎈", "🔔", "🔎", "💎", "🕯️", "🧰", "🧵",
    ]),
    ("Symbols", &[
        "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "💔",
        "💕", "💖", "💯", "💢", "💬", "💤", "✨", "🔥",
        "🎉", "🎊", "✅", "❌", "❓", "❗", "⚠️", "♻️",
        "➕", "➖", "➡️", "⬅️", "⬆️", "⬇️", "🔄", "🎵",
    ]),
];

const COLUMNS: usize = 8;
const CELL: f64 = 32.0;
const TITLE: f64 = 22.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.EmojiPicker = set_type_default() do #(EmojiPicker::register_widget(vm)){
        width: #(COLUMNS as f64 * CELL) height: 260
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 18.0}}
        title_color: theme.color_text_meta
        draw_highlight +: {color: theme.color_outset_hover}
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum EmojiPickerAction {
    Picked(String),
    #[default]
    None,
}

// Section zero is Recent. Geometry is relative to the unscrolled grid, so
// drawing, hits and accessibility all follow the popover's final placement.
#[derive(Clone, Copy)]
struct Section {
    id: usize,
    count: usize,
    first: usize,
    first_row: usize,
}

fn sections(recent_count: usize) -> impl Iterator<Item = Section> {
    std::iter::once(recent_count).chain(SECTIONS.iter().map(|(_, items)| items.len()))
        .enumerate().filter(|(_, n)| *n != 0)
        .scan((0, 0), |(first, row), (id, count)| {
            let section = Section { id, count, first: *first, first_row: *row };
            *first += count;
            *row += count.div_ceil(COLUMNS);
            Some(section)
        })
}

impl Section {
    fn title(self) -> &'static str {
        if self.id == 0 { "Recent" } else { SECTIONS[self.id - 1].0 }
    }

    fn emoji(self, recents: &[String], offset: usize) -> &str {
        if self.id == 0 { &recents[offset] } else { SECTIONS[self.id - 1].1[offset] }
    }

    fn top(self, recent_count: usize) -> f64 {
        let titles_before = self.id - usize::from(recent_count == 0);
        self.first_row as f64 * CELL + titles_before as f64 * TITLE
    }

    fn cell(self, offset: usize, top: f64) -> Rect {
        Rect {
            pos: dvec2((offset % COLUMNS) as f64 * CELL, top + TITLE + (offset / COLUMNS) as f64 * CELL),
            size: dvec2(CELL, CELL),
        }
    }

    fn visible_cells(self, top: f64, height: f64) -> std::ops::Range<usize> {
        let first = ((-top - TITLE) / CELL).floor().max(0.0) as usize;
        let end = ((height - top - TITLE) / CELL).ceil().max(0.0) as usize;
        (first * COLUMNS).min(self.count)..(end * COLUMNS).min(self.count)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct EmojiPicker {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[redraw] #[rust] area: Area,
    #[live] draw_text: DrawText,
    #[live] draw_highlight: DrawColor,
    #[live] title_color: Vec4f,
    #[rust] recents: Vec<String>,
    #[rust] selected: usize,
    #[rust] hovered: Option<usize>,
    #[rust] pressed: Option<usize>,
    #[rust] scroll: f64,
    #[rust] press_scroll: f64,
}

impl EmojiPicker {
    pub fn recents(&self) -> &[String] {
        &self.recents
    }

    pub fn set_recents(&mut self, cx: &mut Cx, recents: &[String]) {
        self.recents.clear();
        // An emoji no longer in the set (or a damaged entry) is skipped; the rest stay.
        for emoji in recents {
            if !SECTIONS.iter().any(|(_, items)| items.contains(&emoji.as_str())) { continue }
            if self.recents.len() < 2 * COLUMNS && !self.recents.contains(emoji) {
                self.recents.push(emoji.clone());
            }
        }
        self.selected = 0;
        self.scroll = 0.0;
        self.hovered = None;
        self.pressed = None;
        self.area.redraw(cx);
    }

    fn section_of(&self, index: usize) -> Option<Section> {
        sections(self.recents.len()).find(|s| index < s.first + s.count)
    }

    fn hit(&self, point: DVec2) -> Option<usize> {
        if point.x < 0.0 || point.x >= COLUMNS as f64 * CELL { return None }
        let col = (point.x / CELL) as usize;
        for s in sections(self.recents.len()) {
            let y = point.y + self.scroll - s.top(self.recents.len()) - TITLE;
            if y < 0.0 { return None }
            let offset = (y / CELL) as usize * COLUMNS + col;
            if offset < s.count { return Some(s.first + offset) }
        }
        None
    }

    fn scroll_to(&mut self, cx: &mut Cx, scroll: f64) {
        let n = self.recents.len();
        let last = sections(n).last().unwrap();
        let bottom = last.top(n) + TITLE + last.count.div_ceil(COLUMNS) as f64 * CELL;
        self.scroll = scroll.clamp(0.0, (bottom - self.area.rect(cx).size.y).max(0.0));
        self.hovered = None;
        self.area.redraw(cx);
    }

    fn navigate(&mut self, cx: &mut Cx, key: KeyCode) {
        let n = self.recents.len();
        let Some(s) = self.section_of(self.selected) else { return };
        let offset = self.selected - s.first;
        let row = s.first_row + offset / COLUMNS;
        let next = match key {
            KeyCode::ArrowLeft => self.selected.saturating_sub(1),
            KeyCode::ArrowRight => self.selected + 1,
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let row = if key == KeyCode::ArrowUp { row.saturating_sub(1) } else { row + 1 };
                let Some(target) = sections(n).find(|s| row < s.first_row + s.count.div_ceil(COLUMNS)) else { return };
                target.first + ((row - target.first_row) * COLUMNS + offset % COLUMNS).min(target.count - 1)
            }
            _ => return,
        };
        let Some(s) = self.section_of(next) else { return };
        self.selected = next;
        let y = s.cell(next - s.first, s.top(n)).pos.y;
        let height = self.area.rect(cx).size.y;
        self.scroll_to(cx, self.scroll.min(y).max(y + CELL - height));
    }

    fn pick(&mut self, cx: &mut Cx, index: usize) {
        let Some(s) = self.section_of(index) else { return };
        let offset = index - s.first;
        let emoji = s.emoji(&self.recents, offset).to_owned();
        self.recents.retain(|other| other != &emoji);
        self.recents.insert(0, emoji.clone());
        self.recents.truncate(2 * COLUMNS);
        // Keep the keyboard on the picked symbol when the Recent section grows.
        self.selected = if s.id == 0 { 0 } else {
            sections(self.recents.len()).find(|section| section.id == s.id).unwrap().first + offset
        };
        self.hovered = None;
        self.area.redraw(cx);
        cx.widget_action(self.uid, EmojiPickerAction::Picked(emoji));
    }
}

impl Widget for EmojiPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let view = cx.turtle().rect();
        let highlighted = self.hovered.unwrap_or(self.selected);
        let size = self.draw_text.text_style.font_size;
        let ink = self.draw_text.color;
        let n = self.recents.len();
        for s in sections(n) {
            let top = s.top(n) - self.scroll;
            if top >= view.size.y { break }
            if top + TITLE > 0.0 {
                self.draw_text.text_style.font_size = 10.0;
                self.draw_text.color = self.title_color;
                self.draw_text.draw_abs(cx, view.pos + dvec2(4.0, top + 3.0), s.title());
                self.draw_text.text_style.font_size = size;
                self.draw_text.color = ink;
            }
            for offset in s.visible_cells(top, view.size.y) {
                let rect = s.cell(offset, top).translate(view.pos);
                if s.first + offset == highlighted {
                    self.draw_highlight.draw_abs(cx, rect);
                }
                self.draw_text.draw_walk(cx, Walk::abs_rect(rect), Align { x: 0.5, y: 0.5 }, s.emoji(&self.recents, offset));
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        // One stop for the whole grid, where the arrows move: a focus-trapping popover holding
        // it gives it key focus when it opens, and gives the focus back after.
        cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hover = self.hit(e.abs - e.rect.pos);
                if hover.is_some() { cx.set_cursor(MouseCursor::Hand); }
                if hover != self.hovered {
                    self.hovered = hover;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hovered.take().is_some() { self.area.redraw(cx); }
            }
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                self.pressed = self.hit(e.abs - e.rect.pos);
                self.press_scroll = self.scroll;
            }
            Hit::FingerMove(e) if e.device.is_touch() && e.move_distance() > TAP_COUNT_DISTANCE => {
                self.pressed = None;
                self.scroll_to(cx, self.press_scroll + e.abs_start.y - e.abs.y);
            }
            Hit::FingerUp(e) => {
                if let Some(index) = self.pressed.take() {
                    if !e.cancelled && e.is_over && self.hit(e.abs - e.rect.pos) == Some(index) {
                        self.pick(cx, index);
                    }
                }
            }
            Hit::FingerScroll(e) => self.scroll_to(cx, self.scroll + e.scroll.y),
            Hit::KeyDown(e) => match e.key_code {
                KeyCode::ReturnKey | KeyCode::NumpadEnter => self.pick(cx, self.selected),
                key => self.navigate(cx, key),
            },
            _ => {}
        }
    }

    fn semantics_parts(&self, cx: &Cx, out: &mut Vec<(u64, Option<u64>, a11y_tree::Node)>) {
        use a11y_tree::{Actions, Node, Role};
        if !self.area.is_valid(cx) { return }
        let view = self.area.rect(cx);
        let clip = self.area.clipped_rect(cx);
        let bounds = |rect: Rect| makepad_platform::a11y::semantic_rect(rect.translate(view.pos).clip((clip.pos, clip.pos + clip.size)));
        let n = self.recents.len();
        for s in sections(n) {
            let top = s.top(n) - self.scroll;
            let height = TITLE + s.count.div_ceil(COLUMNS) as f64 * CELL;
            if top >= view.size.y || top + height <= 0.0 { continue }
            let group = s.id as u64;
            out.push((group, None, Node {
                role: Role::Group, name: s.title().into(),
                bounds: bounds(Rect { pos: dvec2(0.0, top), size: dvec2(view.size.x, height) }),
                ..Default::default()
            }));
            for offset in s.visible_cells(top, view.size.y) {
                let emoji = s.emoji(&self.recents, offset);
                // Distinguish Recent from the catalogue and keep each key across reordering.
                let key = ((group + 1) << 32) | emoji.chars().next().unwrap() as u64;
                out.push((key, Some(group), Node {
                    role: Role::Button, name: emoji.into(), bounds: bounds(s.cell(offset, top)),
                    actions: Actions::PRESS, ..Default::default()
                }));
            }
        }
    }

    fn handle_action(&mut self, cx: &mut Cx, request: &a11y_tree::ActionRequest, _scope: &mut Scope) -> a11y_tree::ActionResult {
        use a11y_tree::{ActionResult, Actions};
        if request.action != Actions::PRESS { return ActionResult::Unhandled }
        let Some(key) = cx.a11y.action_part_key(request.node) else { return ActionResult::Unhandled };
        for s in sections(self.recents.len()) {
            if s.id as u64 + 1 != key >> 32 { continue }
            if let Some(offset) = (0..s.count).find(|&i| s.emoji(&self.recents, i).chars().next().unwrap() as u32 == key as u32) {
                self.pick(cx, s.first + offset);
                return ActionResult::Handled
            }
        }
        ActionResult::Unhandled
    }
}

impl EmojiPickerRef {
    pub fn set_recents(&self, cx: &mut Cx, recents: &[String]) {
        if let Some(mut picker) = self.borrow_mut() { picker.set_recents(cx, recents); }
    }

    pub fn recents(&self) -> Vec<String> {
        self.borrow().map(|picker| picker.recents().to_vec()).unwrap_or_default()
    }

    pub fn picked(&self, actions: &Actions) -> Option<String> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            EmojiPickerAction::Picked(emoji) => Some(emoji),
            EmojiPickerAction::None => None,
        }
    }
}
