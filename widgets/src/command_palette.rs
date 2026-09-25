//! CommandPalette — everything the app can do, found by typing at it.
//!
//! A menu is a map: you learn where a thing lives and reach for the same
//! place every time. A palette is the other trade — you say the name and it
//! comes to you — and it is the only way back to a command whose name you
//! remember and whose home you have forgotten. An app wants both, and this
//! one is shaped so that the SAME table of lines feeds the palette and the
//! shortcut sheet: each line of `commands` is either `"# Heading"` or
//! `"What it does = ctrl+p"`, exactly the format
//! [`crate::kbd::ShortcutHelp`] reads.
//!
//! Typing narrows the list by a subsequence match with the matched letters
//! marked, and every row draws its chord as key caps through a
//! [`crate::kbd::KbdGroup`] — so somebody who arrived by typing leaves
//! knowing the keys, and stops needing the palette for that command.
//!
//! # Ranking, and why the headings survive it
//!
//! Ranking and grouping usually fight: rank the whole list and the groups
//! interleave into nonsense, keep the groups and the best match is buried
//! three headings down. They are settled here by ranking BOTH. Inside a
//! group the commands are ordered by score; the groups themselves are
//! ordered by their own best command. The first line of the first group is
//! therefore the best match in the whole list — the cursor starts on it and
//! Enter runs it — and the sheet still reads as the sheet somebody wrote.
//! With no query every score is zero, ties fall back to the written order,
//! and the palette shows the table exactly as it stands.
//!
//! # What it deliberately does not do
//!
//! It does not bind the chord that opens it. Nothing here listens for a
//! global key; a host calls [`CommandPalette::open`] from wherever it keeps
//! its bindings, because a widget that bound its own opener would quietly
//! become the place that binding is defined, which is the last place anyone
//! would look for it.
//!
//! It does not run anything either. Choosing a row raises an action carrying
//! the command's ordinal and its label, and the host does the work — the
//! palette has no idea what any of these lines mean.
//!
//! It has no scrollbar. The list windows to `max_rows` lines around the
//! cursor and the keyboard walks it. A palette is a keyboard instrument; a
//! bar to drag would be furniture for a gesture nobody makes here.
//!
//! It searches labels, not chords. The marks have to land on the letters the
//! person typed, and a query that matched "ctrl" would mark nothing at all.
//! Searching by chord is what the shortcut sheet is for.
use crate::{
    kbd::{parse_help_entries, HelpEntry, KbdGroup},
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInput, TextInputAction},
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The palette owns a TextInput and a KbdGroup, so this module has to
    // register after both of theirs: a block's `use` only sees what already
    // exists, and a slot filled from a name that is not there yet is an
    // empty slot.

    set_type_default() do #(DrawPaletteRow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawPaletteText::script_shader(vm)){
        ..mod.draw.DrawText
    }

    mod.widgets.CommandPaletteBase = #(CommandPalette::register_widget(vm))

    /** A search field over the commands an app can run, over an overlay that
     * covers the window. */
    mod.widgets.CommandPalette = set_type_default() do mod.widgets.CommandPaletteBase{
        // Zero by zero, and `on_after_apply` pins it there. The palette is an
        // overlay: it paints on a root turtle sized by the pass and never
        // walks its parent's turtle, so the walk it reports upward must claim
        // nothing. A `Fill` here would make it a deferred fill of its parent,
        // taking a share of that parent's spare length whether or not it was
        // ever opened.
        width: 0.
        height: 0.

        /** the lines: "# Heading" opens a group, "What it does = ctrl+p" is a command */
        commands: []
        /** shown in place of the list when nothing matches */
        empty_text: "No command matches"
        /** the quiet line under the list; "" leaves it out */
        hint_text: "\u{2191}\u{2193} to move \u{00b7} Enter to run \u{00b7} Esc to close"

        /** how wide the panel is, in pixels 240..900 step 10 */
        panel_width: 520.
        /** how far below the top of the window the panel sits 0..400 step 4 */
        top_margin: 96.
        /** the least room left between the panel and the window edge 0..80 step 2 */
        screen_margin: 24.
        /** the most lines shown at once; a heading counts as a line 3..30 step 1 */
        max_rows: 12
        /** the room inside the panel edge 0..40 step 1 */
        pad: 10.
        /** the room either side of a row's text 0..40 step 1 */
        row_pad: 10.
        /** the least room between a label and its key caps 0..40 step 1 */
        chord_gap: 12.
        /** the height of one command row 16..48 step 1 */
        row_height: 26.
        /** the height of a heading row 16..48 step 1 */
        heading_height: 26.
        /** the height of the search field 16..60 step 1 */
        search_height: 30.
        /** the room under the search field 0..40 step 1 */
        search_gap: 8.
        /** the height of the key caps in a row 12..40 step 1 */
        cap_height: 20.
        /** the height of the hint line 0..40 step 1 */
        hint_height: 20.

        search: mod.widgets.TextInput{
            empty_text: "Type a command"
        }
        chord: mod.widgets.KbdGroup{}

        draw_scrim +: {
            /** how far the window behind is dimmed 0..1 step 0.01 */
            dim: uniform(0.55)
            color: uniform(theme.color_scrim)
            pixel: fn() {
                // The scrim colour token is opaque — it is the colour the
                // dimming is a tint OF, not the tint — so the alpha is ours.
                return vec4(self.color.xyz, self.dim)
            }
        }

        draw_panel +: {
            /** corner rounding 0..24 step 0.5 */
            radius: uniform(theme.radius_l)
            /** outline width in pixels 0..3 step 0.5 */
            border_size: uniform(1.0)
            color: uniform(theme.color_surface_container_high)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }

        draw_row +: {
            /** the band under the keyboard cursor 0..1 step 0.01 */
            active: 0.0
            /** the band under the pointer 0..1 step 0.01 */
            hover: 0.0
            /** corner rounding 0..16 step 0.5 */
            radius: uniform(theme.radius_s)
            color_hover: uniform(theme.color_opaque_u_1)
            color_active: uniform(theme.color_primary_container)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                let band = mix(self.color_hover, self.color_active, self.active)
                sdf.fill(vec4(band.xyz, band.w * max(self.hover, self.active)))
                return sdf.result
            }
        }

        draw_label +: {
            /** a letter the search matched 0..1 step 0.01 */
            matched: 0.0
            /** the row under the keyboard cursor 0..1 step 0.01 */
            active: 0.0
            color: theme.color_text
            color_active: uniform(theme.color_on_primary_container)
            color_match: uniform(theme.color_primary)
            // Line spacing of one, so the line box IS the ink box: the runs
            // are placed against the row by arithmetic, not by the layouter,
            // and the two only agree when the line carries no leading.
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
            get_color: fn() {
                // The marks are the accent whatever row they are on, so the
                // letters you typed read the same as the cursor moves over
                // them.
                let ink = self.color
                    .mix(self.color_active, self.active)
                    .mix(self.color_match, self.matched)
                return ink
            }
        }

        draw_heading +: {
            color: theme.color_text_meta
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }

        draw_meta +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }
}

/// Where a glyph's ink starts below the y handed to `draw_abs`, as a share of
/// the font size. `draw_abs` takes the top of the LINE box, not the ink, and
/// a run of capitals has no descender to fill the bottom of the line, so a
/// centred line box leaves the text riding high.
const INK_TOP: f64 = 0.30;

/// The narrowest the panel may be squeezed to before it stops giving ground
/// to the window edge; under this a command list is unreadable anyway.
const MIN_PANEL_WIDTH: f64 = 200.0;

/// Where one line of text goes so that it sits in the middle of `rect`.
fn ink_y(rect: Rect, font_size: f64) -> f64 {
    rect.pos.y + (rect.size.y - font_size) * 0.5 - font_size * INK_TOP
}

/// How far a drawn run advances the pen.
///
/// The slack [`crate::badge::measure`] adds is there so a box drawn AROUND a run
/// clears the last glyph's side bearing; the runs of one label are drawn end to
/// end, and adding it between them would open a two-pixel hole in the middle of
/// every marked word.
fn run_width(draw_text: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    crate::badge::advance(draw_text, cx, text)
}

/// Where the search letters landed in a label, and what that alignment
/// scored.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteMatch {
    pub score: i32,
    /// Char indices into the label, ascending.
    pub marks: Vec<usize>,
}

/// What one matched letter is worth wherever it falls.
const MATCH_BASE: i32 = 4;
/// A letter that starts a word is the letter somebody meant to type.
const WORD_START_BONUS: i32 = 6;
/// A letter taken immediately after the one before it.
const RUN_BONUS: i32 = 5;
/// How far into a label the earliness penalty keeps counting. Past this
/// everything is simply "late", so a long label does not lose to a short one
/// on its tail alone.
const LATE_CAP: i32 = 12;

/// Whether `at` begins a word. The ORIGINAL characters are read, not the
/// folded ones, so a run of camel case counts as the several words it is.
fn word_start(chars: &[char], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let prev = chars[at - 1];
    !prev.is_alphanumeric() || (prev.is_lowercase() && chars[at].is_uppercase())
}

fn place_score(chars: &[char], at: usize) -> i32 {
    MATCH_BASE
        + if word_start(chars, at) { WORD_START_BONUS } else { 0 }
        - (at as i32).min(LATE_CAP)
}

/// Lowercase one character at a time rather than the whole string: folding a
/// string can change how many characters it has, and the marks index the
/// original.
fn folded(chars: &[char]) -> Vec<char> {
    chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect()
}

/// The best way to read `query` as a subsequence of `label`, or `None` when
/// it is not one at all. An empty query matches everything at zero.
///
/// Best, not first. A greedy left-to-right scan takes each letter at its
/// earliest chance, which for "fa" against "Frame All" marks the F and the a
/// of *Frame* — the letters somebody typed are the initials, and marking the
/// wrong ones makes the palette look as though it matched by accident. So
/// every alignment is scored and the best one wins: the search runs over the
/// choices rather than down the first path it finds.
pub fn fuzzy_match(label: &str, query: &str) -> Option<PaletteMatch> {
    let needle: Vec<char> = query.trim().chars().collect();
    if needle.is_empty() {
        return Some(PaletteMatch { score: 0, marks: Vec::new() });
    }
    let hay: Vec<char> = label.chars().collect();
    if needle.len() > hay.len() {
        return None;
    }
    let hay_low = folded(&hay);
    let needle_low = folded(&needle);
    let (m, n) = (needle_low.len(), hay.len());

    const NONE: i32 = i32::MIN;
    // best[j * n + p]: the best total for needle[j..] when needle[j] is taken
    // at p, NOT counting the run bonus needle[j] earns from needle[j - 1] —
    // that belongs to whoever chose p, since only they know where the letter
    // before it went. Quadratic in the label length, which costs nothing for
    // the short labels a command list holds.
    let mut best = vec![NONE; m * n];
    let mut pick = vec![usize::MAX; m * n];
    for j in (0..m).rev() {
        for p in 0..n {
            if hay_low[p] != needle_low[j] {
                continue;
            }
            if j + 1 == m {
                best[j * n + p] = place_score(&hay, p);
                continue;
            }
            let mut tail = NONE;
            let mut tail_at = usize::MAX;
            for q in p + 1..n {
                let next = best[(j + 1) * n + q];
                if next == NONE {
                    continue;
                }
                let with_run = next + if q == p + 1 { RUN_BONUS } else { 0 };
                if with_run > tail {
                    tail = with_run;
                    tail_at = q;
                }
            }
            if tail != NONE {
                best[j * n + p] = place_score(&hay, p) + tail;
                pick[j * n + p] = tail_at;
            }
        }
    }

    let mut at = usize::MAX;
    let mut score = NONE;
    for p in 0..n {
        if best[p] > score {
            score = best[p];
            at = p;
        }
    }
    if score == NONE {
        return None;
    }
    let mut marks = Vec::with_capacity(m);
    let mut j = 0;
    while at != usize::MAX {
        marks.push(at);
        at = pick[j * n + at];
        j += 1;
    }
    Some(PaletteMatch { score, marks })
}

/// One line of the list as it will be drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteLine {
    Heading(String),
    Command {
        /// The command's ordinal among the command rows AS WRITTEN, which is
        /// what a host's own array of commands is indexed by. It does not
        /// move when the ranking does.
        index: usize,
        label: String,
        shortcut: String,
        /// Char indices into `label` that the query matched.
        marks: Vec<usize>,
    },
}

/// The lines a query leaves, in the order they are drawn.
///
/// Commands are ranked inside their group and the groups are ranked by their
/// own best command, so the first command of the first group is the best
/// match in the whole list. A group that lost every command loses its heading
/// with them.
pub fn rank_commands(entries: &[HelpEntry], query: &str) -> Vec<PaletteLine> {
    // Commands written before any heading belong to a group with no title.
    let mut groups: Vec<(Option<String>, Vec<(i32, usize, PaletteLine)>)> = vec![(None, Vec::new())];
    let mut ordinal = 0usize;
    for entry in entries {
        match entry {
            HelpEntry::Heading(text) => groups.push((Some(text.clone()), Vec::new())),
            HelpEntry::Row { label, shortcut } => {
                let index = ordinal;
                ordinal += 1;
                let Some(hit) = fuzzy_match(label, query) else {
                    continue;
                };
                let line = PaletteLine::Command {
                    index,
                    label: label.clone(),
                    shortcut: shortcut.clone(),
                    marks: hit.marks,
                };
                // There is always a group open: the untitled one, at worst.
                if let Some(group) = groups.last_mut() {
                    group.1.push((hit.score, index, line));
                }
            }
        }
    }

    let mut kept: Vec<(i32, usize, Option<String>, Vec<PaletteLine>)> = Vec::new();
    for (at, (heading, mut rows)) in groups.into_iter().enumerate() {
        if rows.is_empty() {
            continue;
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let best = rows[0].0;
        kept.push((best, at, heading, rows.into_iter().map(|(_, _, line)| line).collect()));
    }
    // The written position is the tie-break at both levels, so an empty
    // query — every score zero — reproduces the table exactly as written.
    kept.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut lines = Vec::new();
    for (_, _, heading, rows) in kept {
        if let Some(heading) = heading {
            lines.push(PaletteLine::Heading(heading));
        }
        lines.extend(rows);
    }
    lines
}

/// The label split into runs, each flagged as matched or not, so a run is one
/// call to the text layer at one colour.
pub fn mark_runs(label: &str, marks: &[usize]) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    for (at, ch) in label.chars().enumerate() {
        // The marks are ascending, which is what makes this a search rather
        // than a scan of the whole list per character.
        let hit = marks.binary_search(&at).is_ok();
        match out.last_mut() {
            Some((flag, text)) if *flag == hit => text.push(ch),
            _ => out.push((hit, ch.to_string())),
        }
    }
    out
}

/// Where the window over the list starts, so that `cursor` is inside it and
/// the last line is never scrolled past.
pub fn palette_window_top(len: usize, cursor: Option<usize>, top: usize, max_rows: usize) -> usize {
    let max = max_rows.max(1);
    let Some(at) = cursor else {
        return 0;
    };
    let mut top = top;
    if at < top {
        top = at;
    } else if at >= top + max {
        top = at + 1 - max;
    }
    top.min(len.saturating_sub(max))
}

/// The band under a row: one for the keyboard cursor, one for the pointer.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPaletteRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    active: f32,
    #[live]
    hover: f32,
}

/// A run of a label, told whether the search matched it and whether the row
/// it is on holds the cursor.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPaletteText {
    #[deref]
    draw_super: DrawText,
    #[live]
    matched: f32,
    #[live]
    active: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum CommandPaletteAction {
    /// A command was chosen. Carries its ordinal among the command rows as
    /// written and the label it was chosen by.
    Ran { index: usize, label: String, shortcut: String },
    /// Closed by the person rather than by the host, with nothing run.
    Dismissed,
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct CommandPalette {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The dimming layer, which is also what catches every press meant for
    /// whatever is underneath.
    #[redraw]
    #[live]
    draw_scrim: DrawQuad,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_row: DrawPaletteRow,
    #[live]
    draw_label: DrawPaletteText,
    #[live]
    draw_heading: DrawText,
    /// The empty message and the hint line: the two quiet ones.
    #[live]
    draw_meta: DrawText,
    /// One chord, drawn once per row.
    #[live]
    chord: KbdGroup,
    #[live]
    search: TextInput,

    /// The lines: "# Heading" opens a group, "What it does = ctrl+p" is a
    /// command.
    #[live]
    pub commands: Vec<String>,
    #[live("No command matches".to_string())]
    pub empty_text: String,
    /// The quiet line under the list; empty leaves it out.
    #[live]
    pub hint_text: String,

    #[live(520.0)]
    pub panel_width: f64,
    #[live(96.0)]
    pub top_margin: f64,
    #[live(24.0)]
    pub screen_margin: f64,
    #[live(12)]
    pub max_rows: usize,
    #[live(10.0)]
    pub pad: f64,
    #[live(10.0)]
    pub row_pad: f64,
    #[live(12.0)]
    pub chord_gap: f64,
    #[live(26.0)]
    pub row_height: f64,
    #[live(26.0)]
    pub heading_height: f64,
    #[live(30.0)]
    pub search_height: f64,
    #[live(8.0)]
    pub search_gap: f64,
    #[live(20.0)]
    pub cap_height: f64,
    #[live(20.0)]
    pub hint_height: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    /// The palette paints on a list of its own so it can be over everything
    /// that was drawn before it.
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    shown: bool,
    #[rust]
    query: String,
    #[rust]
    lines: Vec<PaletteLine>,
    /// Index into `lines`, always at a `PaletteLine::Command`.
    #[rust]
    cursor: Option<usize>,
    #[rust]
    top: usize,
    #[rust]
    hover: Option<usize>,
    /// Set by `open`, spent by the first draw that gives the field an area:
    /// focus cannot be taken before there is something to take it with.
    #[rust]
    want_focus: bool,
    /// Where each command row landed this draw, and which line it is.
    #[rust]
    rows: Vec<(usize, Rect)>,
    #[rust]
    panel_rect: Rect,
}

impl ScriptHook for CommandPalette {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
        self.rebuild();
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Forced here rather than only left out of the DSL, so an instance
        // writing `CommandPalette{height: Fill}` cannot make the palette a
        // deferred fill of the column it happens to be declared in.
        self.walk = Walk::empty();
        self.rebuild();
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl CommandPalette {
    /// Re-read the table and re-rank it against the query. Cheap enough to do
    /// on every keystroke: the table is tens of lines, not thousands.
    fn rebuild(&mut self) {
        let entries = parse_help_entries(&self.commands);
        self.lines = rank_commands(&entries, &self.query);
        self.cursor = self.first_command();
        self.top = 0;
        self.hover = None;
    }

    fn first_command(&self) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| matches!(line, PaletteLine::Command { .. }))
    }

    /// The lines that are commands, in drawn order.
    fn command_lines(&self) -> Vec<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, PaletteLine::Command { .. }))
            .map(|(at, _)| at)
            .collect()
    }

    fn redraw_overlay(&mut self, cx: &mut Cx) {
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
        self.draw_scrim.redraw(cx);
    }

    /// Move the cursor by `delta` commands. The arrows wrap, which is what
    /// makes the last command one keypress from the first; a page does not,
    /// because a page that came round would lose your place entirely.
    fn step_cursor(&mut self, cx: &mut Cx, delta: isize, wrap: bool) {
        let commands = self.command_lines();
        if commands.is_empty() {
            self.cursor = None;
            return;
        }
        let now = self
            .cursor
            .and_then(|at| commands.iter().position(|line| *line == at))
            .unwrap_or(0) as isize;
        let len = commands.len() as isize;
        let next = if wrap {
            (now + delta).rem_euclid(len)
        } else {
            (now + delta).clamp(0, len - 1)
        };
        self.cursor = Some(commands[next as usize]);
        // The pointer is somewhere else entirely; leaving its band lit while
        // the keyboard walks away from it says two rows are chosen.
        self.hover = None;
        self.redraw_overlay(cx);
    }

    /// Keys the palette owns, before the search field can eat them. Returns
    /// true when the key was spent here.
    fn handle_nav_key(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        match ke.key_code {
            KeyCode::ArrowDown => {
                self.step_cursor(cx, 1, true);
                true
            }
            KeyCode::ArrowUp => {
                self.step_cursor(cx, -1, true);
                true
            }
            KeyCode::PageDown => {
                self.step_cursor(cx, self.max_rows.max(1) as isize, false);
                true
            }
            KeyCode::PageUp => {
                self.step_cursor(cx, -(self.max_rows.max(1) as isize), false);
                true
            }
            KeyCode::ReturnKey => {
                // Nothing matched: Enter does nothing at all and the query
                // stays editable, rather than running whatever is nearest.
                if let Some(at) = self.cursor {
                    self.run(cx, at);
                }
                true
            }
            KeyCode::Escape => {
                self.dismiss(cx);
                true
            }
            // Home and End belong to the caret. The query is a few characters
            // long and the arrows wrap, so the list loses nothing by leaving
            // the text field the keys a text field is expected to have.
            _ => false,
        }
    }

    fn run(&mut self, cx: &mut Cx, at: usize) {
        let line = self.lines.get(at).cloned();
        let Some(PaletteLine::Command { index, label, shortcut, .. }) = line else {
            return;
        };
        self.close(cx);
        cx.widget_action(self.uid, CommandPaletteAction::Ran { index, label, shortcut });
    }

    /// Closed by the person: the host hears about it, because a host that
    /// only listened for a command would go on believing the palette was up.
    fn dismiss(&mut self, cx: &mut Cx) {
        if !self.shown {
            return;
        }
        self.close(cx);
        cx.widget_action(self.uid, CommandPaletteAction::Dismissed);
    }

    pub fn is_open(&self) -> bool {
        self.shown
    }

    /// Show the palette with an empty query and the cursor on the first
    /// command. It always opens fresh: a palette that reopened holding the
    /// last search would answer the next keystroke with the wrong list.
    pub fn open(&mut self, cx: &mut Cx) {
        if self.shown {
            return;
        }
        self.shown = true;
        self.query.clear();
        self.search.set_text(cx, "");
        self.rebuild();
        self.want_focus = true;
        self.redraw_overlay(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if !self.shown {
            return;
        }
        self.shown = false;
        self.hover = None;
        self.rows.clear();
        cx.unblock_scrolling_within_area(self.draw_panel.area());
        cx.revert_key_focus();
        self.redraw_overlay(cx);
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.shown {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    pub fn set_commands(&mut self, cx: &mut Cx, commands: Vec<String>) {
        if self.commands == commands {
            return;
        }
        self.commands = commands;
        self.rebuild();
        self.redraw_overlay(cx);
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Filter the list. The field is set along with the model, so a query
    /// pushed by the host and one typed into the box leave the same state.
    pub fn set_query(&mut self, cx: &mut Cx, query: &str) {
        if self.query == query {
            return;
        }
        self.query = query.to_string();
        self.rebuild();
        if self.search.text() != query {
            self.search.set_text(cx, query);
        }
        self.redraw_overlay(cx);
    }

    /// How many commands the query leaves; a test waits on this rather than
    /// on pixels.
    pub fn matches(&self) -> usize {
        self.command_lines().len()
    }

    /// The label the cursor is on, or nothing.
    pub fn highlighted(&self) -> Option<String> {
        match self.lines.get(self.cursor?) {
            Some(PaletteLine::Command { label, .. }) => Some(label.clone()),
            _ => None,
        }
    }

    fn draw_body(&mut self, cx: &mut Cx2d, scope: &mut Scope, pass: Vec2d) {
        let margin = self.screen_margin.max(0.0);
        let panel_w = self
            .panel_width
            .min((pass.x - margin * 2.0).max(MIN_PANEL_WIDTH))
            .max(MIN_PANEL_WIDTH);

        self.top = palette_window_top(self.lines.len(), self.cursor, self.top, self.max_rows);
        let count = self.max_rows.max(1).min(self.lines.len().saturating_sub(self.top));
        let visible: Vec<PaletteLine> = self.lines[self.top..self.top + count].to_vec();

        let list_h: f64 = if visible.is_empty() {
            self.row_height
        } else {
            visible
                .iter()
                .map(|line| match line {
                    PaletteLine::Heading(_) => self.heading_height,
                    PaletteLine::Command { .. } => self.row_height,
                })
                .sum()
        };
        let hint_h = if self.hint_text.is_empty() { 0.0 } else { self.hint_height };
        let panel_h = self.pad * 2.0 + self.search_height + self.search_gap + list_h + hint_h;

        let x = ((pass.x - panel_w) * 0.5).max(margin).floor();
        // Near the top, not centred: the list grows downward from the field,
        // and a panel that centred itself would move under the reading eye
        // every time a keystroke changed how many rows there are.
        let y = self.top_margin.min((pass.y - panel_h - margin).max(margin)).floor();

        self.draw_panel
            .begin(cx, Walk::fixed(panel_w, panel_h).with_abs_pos(dvec2(x, y)), self.layout);
        let panel = cx.turtle().rect();
        let left = panel.pos.x + self.pad;
        let width = (panel.size.x - self.pad * 2.0).max(0.0);
        let mut cy = panel.pos.y + self.pad;

        let field = Walk {
            abs_pos: Some(dvec2(left, cy)),
            width: Size::Fixed(width),
            height: Size::Fixed(self.search_height),
            ..Walk::default()
        };
        let _ = self.search.draw_walk(cx, scope, field);
        cy += self.search_height + self.search_gap;

        self.rows.clear();
        if visible.is_empty() {
            let rect = Rect { pos: dvec2(left, cy), size: dvec2(width, self.row_height) };
            let size = self.draw_meta.text_style.font_size as f64;
            let text = self.empty_text.clone();
            self.draw_meta
                .draw_abs(cx, dvec2(left + self.row_pad, ink_y(rect, size)), &text);
            cy += self.row_height;
        }

        for (offset, line) in visible.iter().enumerate() {
            let at = self.top + offset;
            match line {
                PaletteLine::Heading(text) => {
                    let rect = Rect { pos: dvec2(left, cy), size: dvec2(width, self.heading_height) };
                    let size = self.draw_heading.text_style.font_size as f64;
                    self.draw_heading
                        .draw_abs(cx, dvec2(left + self.row_pad, ink_y(rect, size)), text);
                    cy += self.heading_height;
                }
                PaletteLine::Command { label, shortcut, marks, .. } => {
                    let rect = Rect { pos: dvec2(left, cy), size: dvec2(width, self.row_height) };
                    let active = self.cursor == Some(at);
                    let hover = self.hover == Some(at) && !active;
                    if active || hover {
                        self.draw_row.active = if active { 1.0 } else { 0.0 };
                        self.draw_row.hover = if hover { 1.0 } else { 0.0 };
                        self.draw_row.draw_abs(cx, rect);
                    }
                    // The chord is measured before it is placed: a row reads
                    // right to left from the keys, so they end at the edge.
                    self.chord.shortcut = shortcut.clone();
                    let chord = self.chord.chord_size(cx, self.cap_height);
                    if chord.x > 0.0 {
                        let chord_rect = Rect {
                            pos: dvec2(
                                rect.pos.x + rect.size.x - self.row_pad - chord.x,
                                rect.pos.y + (rect.size.y - chord.y) * 0.5,
                            ),
                            size: chord,
                        };
                        self.chord.draw_chord(cx, chord_rect);
                    }
                    // A turtle of its own, so a label longer than the room it
                    // has is cut at the keys rather than running under them.
                    let reserved = if chord.x > 0.0 { chord.x + self.chord_gap } else { 0.0 };
                    let text_w = (rect.size.x - self.row_pad * 2.0 - reserved).max(1.0);
                    cx.begin_turtle(
                        Walk::fixed(text_w, rect.size.y)
                            .with_abs_pos(dvec2(rect.pos.x + self.row_pad, rect.pos.y)),
                        Layout::flow_right(),
                    );
                    let size = self.draw_label.text_style.font_size as f64;
                    let baseline = ink_y(rect, size);
                    let mut pen = rect.pos.x + self.row_pad;
                    self.draw_label.active = if active { 1.0 } else { 0.0 };
                    for (marked, run) in mark_runs(label, marks) {
                        self.draw_label.matched = if marked { 1.0 } else { 0.0 };
                        self.draw_label.draw_abs(cx, dvec2(pen, baseline), &run);
                        pen += run_width(&self.draw_label, cx, &run);
                    }
                    self.draw_label.matched = 0.0;
                    self.draw_label.active = 0.0;
                    cx.end_turtle();

                    self.rows.push((at, rect));
                    cy += self.row_height;
                }
            }
        }

        if !self.hint_text.is_empty() {
            let rect = Rect { pos: dvec2(left, cy), size: dvec2(width, self.hint_height) };
            let size = self.draw_meta.text_style.font_size as f64;
            let text = self.hint_text.clone();
            self.draw_meta
                .draw_abs(cx, dvec2(left + self.row_pad, ink_y(rect, size)), &text);
        }

        self.draw_panel.end(cx);
        self.panel_rect = Rect { pos: dvec2(x, y), size: dvec2(panel_w, panel_h) };
    }
}

impl Widget for CommandPalette {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(open) {
            vm.with_cx_mut(|cx| self.open(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(close) {
            vm.with_cx_mut(|cx| self.close(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(toggle) {
            vm.with_cx_mut(|cx| self.toggle(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.shown {
            return;
        }

        // The keys the list owns, taken before the field can eat them. Gated
        // on the field holding focus, so a palette that is up but not typed
        // into does not swallow the arrows of whatever does.
        if let Event::KeyDown(ke) = event {
            if self.search.key_focus(cx) && self.handle_nav_key(cx, ke) {
                return;
            }
        }

        for action in cx.capture_actions(|cx| self.search.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::Changed(text) => self.set_query(cx, &text),
                TextInputAction::Escaped => {
                    self.dismiss(cx);
                    return;
                }
                _ => {}
            }
        }

        // Everything over the scrim belongs to the palette. Asking for the
        // hit is what consumes it, and consuming it is what keeps the press
        // away from whatever is drawn underneath.
        match event.hits(cx, self.draw_scrim.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self
                    .rows
                    .iter()
                    .find(|(_, rect)| rect.contains(fe.abs))
                    .map(|(at, _)| *at);
                if at != self.hover {
                    self.hover = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.redraw_overlay(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw_overlay(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && !fe.cancelled => {
                let row = self
                    .rows
                    .iter()
                    .find(|(_, rect)| rect.contains(fe.abs))
                    .map(|(at, _)| *at);
                match row {
                    // A press runs the row it landed on, not the row the
                    // keyboard is holding: the finger is the instruction.
                    Some(at) => self.run(cx, at),
                    None if !self.panel_rect.contains(fe.abs) => self.dismiss(cx),
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_list.is_none() {
            self.draw_list = Some(DrawList2d::new(cx));
        }
        self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_overlay());
        if self.shown && self.visible {
            // The scrim is begun around the panel so its area is the whole
            // pass, which is what the hit test above tests against.
            self.draw_scrim.begin(cx, Walk::fill(), Layout::flow_overlay());
            self.draw_body(cx, scope, pass);
            self.draw_scrim.end(cx);
        } else {
            self.rows.clear();
            self.panel_rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(0.0, 0.0) };
        }
        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);

        if self.shown && self.visible {
            cx.block_scrolling_except_within(self.draw_panel.area());
            if self.want_focus {
                self.want_focus = false;
                self.search.take_key_focus(cx.cx);
            }
        }
        DrawStep::done()
    }

    /// What is in the search box.
    fn text(&self) -> String {
        self.query.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_query(cx, v);
    }

    /// The command the cursor is on, which is what the palette is being asked
    /// for.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.highlighted().unwrap_or_default())
    }
}

impl CommandPaletteRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle(cx);
        }
    }

    /// Ask rather than track: the palette closes for reasons a host never
    /// hears about.
    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    pub fn set_commands(&self, cx: &mut Cx, commands: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_commands(cx, commands);
        }
    }

    pub fn set_query(&self, cx: &mut Cx, query: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_query(cx, query);
        }
    }

    pub fn matches(&self) -> usize {
        self.borrow().map(|inner| inner.matches()).unwrap_or(0)
    }

    pub fn highlighted(&self) -> Option<String> {
        self.borrow().and_then(|inner| inner.highlighted())
    }

    /// The command chosen this pass: its ordinal among the command rows as
    /// written, and its label.
    pub fn ran(&self, actions: &Actions) -> Option<(usize, String)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<CommandPaletteAction>() {
            CommandPaletteAction::Ran { index, label, .. } => Some((index, label)),
            _ => None,
        }
    }

    /// The chord of the command chosen this pass, as it was written.
    pub fn ran_shortcut(&self, actions: &Actions) -> Option<String> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<CommandPaletteAction>() {
            CommandPaletteAction::Ran { shortcut, .. } => Some(shortcut),
            _ => None,
        }
    }

    /// Closed by the person, with nothing run.
    pub fn dismissed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|action| matches!(action.cast(), CommandPaletteAction::Dismissed))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|row| row.to_string()).collect()
    }

    fn table(rows: &[&str]) -> Vec<HelpEntry> {
        parse_help_entries(&lines(rows))
    }

    #[test]
    fn the_letters_may_be_anywhere_as_long_as_they_are_in_order() {
        let hit = fuzzy_match("Toggle Full Screen", "tfs").expect("a subsequence");
        assert_eq!(hit.marks, [0, 7, 12]);
        assert!(fuzzy_match("Toggle Full Screen", "toggle").is_some());
        // Case is not part of it, on either side.
        assert!(fuzzy_match("Toggle Full Screen", "TFS").is_some());
    }

    #[test]
    fn out_of_order_is_not_a_match_and_neither_is_a_letter_that_is_not_there() {
        assert!(fuzzy_match("Frame All", "zzz").is_none());
        assert!(fuzzy_match("Frame All", "af").is_none(), "the letters were in the wrong order");
        assert!(fuzzy_match("Copy", "copies").is_none(), "longer than the label");
    }

    #[test]
    fn an_empty_query_matches_everything_at_nothing() {
        let hit = fuzzy_match("Frame All", "").expect("an empty query matches");
        assert_eq!(hit.score, 0);
        assert!(hit.marks.is_empty());
        assert_eq!(fuzzy_match("Frame All", "   ").expect("blank is empty").score, 0);
    }

    /// The letters somebody types are usually the initials, so the scoring
    /// has to prefer word starts over the first characters that happen to
    /// fit. "fa" is the F of Frame and the A of All, not the a of Frame.
    #[test]
    fn the_marks_land_on_the_word_starts_rather_than_the_earliest_fit() {
        let hit = fuzzy_match("Frame All", "fa").expect("a match");
        assert_eq!(hit.marks, [0, 6]);
        assert_eq!(
            mark_runs("Frame All", &hit.marks),
            vec![
                (true, "F".to_string()),
                (false, "rame ".to_string()),
                (true, "A".to_string()),
                (false, "ll".to_string()),
            ]
        );
    }

    #[test]
    fn a_match_near_the_front_beats_the_same_match_further_in() {
        let early = fuzzy_match("Open Project", "op").expect("a match").score;
        let late = fuzzy_match("Stop Playing", "op").expect("a match").score;
        assert!(early > late, "{early} should beat {late}");
    }

    #[test]
    fn a_run_of_letters_beats_the_same_letters_scattered() {
        let together = fuzzy_match("Reload", "rel").expect("a match").score;
        let apart = fuzzy_match("Rotate Element", "rel").expect("a match").score;
        assert!(together > apart, "{together} should beat {apart}");
    }

    #[test]
    fn camel_case_counts_as_the_words_it_is() {
        // No spaces to go on, so the case change is the only word boundary
        // there is; without it "fa" here would mark the a of Frame.
        let hit = fuzzy_match("FrameAll", "fa").expect("a match");
        assert_eq!(hit.marks, [0, 5]);
    }

    #[test]
    fn with_no_query_the_table_reads_exactly_as_it_was_written() {
        let entries = table(&[
            "# Files",
            "Reload config = ctrl+r",
            "# Editing",
            "Copy = ctrl+c",
            "Cut = ctrl+x",
        ]);
        let out = rank_commands(&entries, "");
        assert_eq!(out[0], PaletteLine::Heading("Files".to_string()));
        assert_eq!(out[2], PaletteLine::Heading("Editing".to_string()));
        assert_eq!(out.len(), 5);
    }

    /// The group holding the best command comes first, so the cursor's home
    /// — the first command of the first group — is the best match in the
    /// whole list, and the headings still survive the ranking.
    #[test]
    fn the_group_with_the_best_command_comes_first() {
        let entries = table(&[
            "# Files",
            "Reload config = ctrl+r",
            "# Editing",
            "Copy = ctrl+c",
            "Cut = ctrl+x",
        ]);
        let out = rank_commands(&entries, "co");
        assert_eq!(out[0], PaletteLine::Heading("Editing".to_string()));
        match &out[1] {
            PaletteLine::Command { label, .. } => assert_eq!(label, "Copy"),
            other => panic!("expected the best command, got {other:?}"),
        }
        assert_eq!(out[2], PaletteLine::Heading("Files".to_string()));
    }

    #[test]
    fn inside_a_group_the_better_match_rises() {
        let entries = table(&["# Editing", "Select all = ctrl+a", "Copy = ctrl+c"]);
        let out = rank_commands(&entries, "c");
        match &out[1] {
            PaletteLine::Command { label, .. } => {
                assert_eq!(label, "Copy", "written second, ranked first")
            }
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn a_group_that_lost_every_command_loses_its_heading_too() {
        let entries = table(&["# Files", "Reload = ctrl+r", "# Editing", "Copy = ctrl+c"]);
        let out = rank_commands(&entries, "copy");
        assert_eq!(
            out,
            vec![
                PaletteLine::Heading("Editing".to_string()),
                PaletteLine::Command {
                    index: 1,
                    label: "Copy".to_string(),
                    shortcut: "ctrl+c".to_string(),
                    marks: vec![0, 1, 2, 3],
                },
            ]
        );
    }

    /// The ordinal a row reports is its position among the command rows as
    /// WRITTEN — a host indexes its own array by it, and it must not move
    /// when the ranking does.
    #[test]
    fn the_ordinal_is_the_written_position_not_the_ranked_one() {
        let entries = table(&["# Editing", "Select all = ctrl+a", "Copy = ctrl+c"]);
        let out = rank_commands(&entries, "c");
        let ordinals: Vec<usize> = out
            .iter()
            .filter_map(|line| match line {
                PaletteLine::Command { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(ordinals, [1, 0]);
    }

    #[test]
    fn a_command_written_before_any_heading_still_appears() {
        let entries = table(&["Quit = ctrl+q", "# Editing", "Copy = ctrl+c"]);
        let out = rank_commands(&entries, "quit");
        assert_eq!(out.len(), 1, "and brings no heading with it");
        match &out[0] {
            PaletteLine::Command { label, .. } => assert_eq!(label, "Quit"),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn runs_are_one_call_to_the_text_layer_each() {
        assert_eq!(mark_runs("Copy", &[]), vec![(false, "Copy".to_string())]);
        assert_eq!(
            mark_runs("Copy", &[0, 1, 2, 3]),
            vec![(true, "Copy".to_string())],
            "neighbouring marks are one run, not four"
        );
        assert!(mark_runs("", &[]).is_empty());
    }

    #[test]
    fn the_window_moves_only_far_enough_to_hold_the_cursor() {
        assert_eq!(palette_window_top(20, Some(15), 0, 12), 4);
        assert_eq!(palette_window_top(20, Some(2), 4, 12), 2);
        assert_eq!(palette_window_top(20, Some(6), 4, 12), 4, "already inside, left alone");
    }

    #[test]
    fn the_window_never_scrolls_past_the_last_line() {
        assert_eq!(palette_window_top(20, Some(19), 0, 12), 8);
        assert_eq!(palette_window_top(3, Some(0), 9, 12), 0, "a short list starts at the top");
        assert_eq!(palette_window_top(0, None, 5, 12), 0);
    }
}
