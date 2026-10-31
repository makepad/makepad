//! The style manager and the AI chat component.
//!
//! - **`HwStyleManager`**: a drawer over the device ("Edit styles…" in the
//!   style menu) listing every style in a shared `ReorderList`: drag the
//!   grip to reorder, row keys to use, tweak, copy, hide and delete, a
//!   right-click menu with the same and more (rename, reset to original,
//!   move up/down), Undo for the last delete. Built-ins are never written:
//!   Tweak on one forks it into "<Title> (mine)" first. It draws with the
//!   style's own templates.
//! - **Tweak…**: the common knobs of a style (palette colours, finishes,
//!   spill and light, the colour map, knob and meter kinds) as editable
//!   Splash values; Return writes the value into the (forked) user style
//!   and applies it through the AI's validated live apply.
//! - **`HwChat`**: the `chat` component a layout can place: the AI chat
//!   panel (`makepad_app_aichat`) under a header with the Live preview switch.
//!
//! All file work is `style_store`; applying is `ai`.

use crate::ai;
use crate::hw;
use crate::layout::HwStageWidgetRefExt;
use crate::style_store::{self, StyleInfo};
use makepad_widgets::reorder_list::ReorderList;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ---- the chat component ----
    mod.widgets.HwChat = View{
        width: Fill
        height: Fill
        chat_well := mod.widgets.HwPanel{
            draw_bg +: {plate_id: 6.0}
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            padding: Inset{left: 8 right: 8 top: 12 bottom: 8}
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                padding: Inset{left: 6 right: 4}
                align: Align{y: 0.5}
                mod.widgets.HwPanelTitle{width: Fill text: "AI STYLIST"}
                chat_styles := mod.widgets.HwNavButton{text: "Styles…"}
            }
            // The chat on a dark display window, so it reads on any front.
            chat_display := mod.widgets.HwDisplay{
                width: Fill
                height: Fill
                flow: Down
                spacing: 4
                padding: Inset{left: 4 right: 4 top: 8 bottom: 4}
                live_preview := mod.widgets.HwBrowserCheck{text: "Live preview: apply each write as it lands" margin: Inset{left: 10}}
                chat_panel := AiChatPanel{
                    width: Fill
                    height: Fill
                    // The person's own cloud agent; the panel's menu switches.
                    provider: "claude-cli"
                    // The panel's controls in the style: the display's
                    // chooser, hints and field, on the display's glass.
                    header +: {
                        provider_pick := mod.widgets.HwDropDown{width: Fit labels: ["Model"]}
                    }
                    mcp_row +: {
                        mcp_connect := mod.widgets.HwConnectHint{text: "Connect the desktop assistant"}
                        mcp_status := mod.widgets.HwConnText{text: "" draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
                        mcp_hint := mod.widgets.HwConnMuted{text: "Type in the desktop assistant; its calls show here." draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
                    }
                    // The display's field is the composer: no second box round it.
                    composer +: {
                        draw_bg +: {color: #x00000000 border_color: #x00000000}
                        input := mod.widgets.HwTextInput{width: Fill height: Fit empty_text: "Ask for a change or a question"}
                    }
                }
            }
        }
    }

    // ---- the style manager ----
    mod.widgets.HwStyleManagerBase = #(StyleManager::register_widget(vm))
    mod.widgets.HwStyleManager = set_type_default() do mod.widgets.HwStyleManagerBase{
        width: Fill
        height: Fill
        flow: Overlay
        align: Align{x: 1.0 y: 0.0}
        visible: false
        sm_plate := mod.widgets.HwPlate{
            draw_bg +: {plate_id: 3.0}
            width: 680
            height: Fill
            depth: 7.0
            inset: 10
            margin: Inset{top: 10 bottom: 10 right: 10}
            flow: Down
            spacing: 8
            padding: Inset{left: 20 right: 20 top: 18 bottom: 18}
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{y: 0.5}
                mod.widgets.HwTitle{width: Fill text: "STYLES"}
                sm_undo_cap := mod.widgets.HwKey{width: 70 height: 34 sm_undo := mod.widgets.HwKeyText{text: "UNDO"}}
                sm_close_cap := mod.widgets.HwKey{width: 44 height: 34 draw_bg +: {role: 28.0} sm_close := mod.widgets.HwKeyButton{text: "\u{f00d}"}}
            }
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{y: 0.5}
                sm_title := mod.widgets.HwTextInput{width: Fill height: 30 empty_text: "Title (for Rename or New)"}
                sm_rename := mod.widgets.HwNavButton{width: 90 text: "Rename"}
                sm_new := mod.widgets.HwNavButton{width: 70 text: "New"}
            }
            sm_status := mod.widgets.HwPanelText{width: Fill text: "Drag ≡ to reorder · right-click a row for more"}
            sm_well := mod.widgets.HwDisplay{
                draw_bg +: {plate_id: 11.0}
                width: Fill
                height: Fill
                flow: Down
                padding: Inset{left: 6 right: 6 top: 6 bottom: 6}
                sm_rows := View{
                width: Fill
                height: Fill
                sm_list := ReorderList{
                    width: Fill
                    height: Fill
                    drag_handle: @grip
                    Row := View{
                        width: Fill
                        height: 34
                        flow: Right
                        spacing: 6
                        align: Align{y: 0.5}
                        padding: Inset{left: 2 right: 2}
                        cursor: MouseCursor.Default
                        show_bg: true
                        draw_bg +: {color: #x00000000}
                        grip := View{
                            width: 20
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            cursor: MouseCursor.Grab
                            mod.widgets.HwDisplayText{text: "≡"}
                        }
                        chip := SolidView{width: 16 height: 16 draw_bg +: {color: #x808080}}
                        title := mod.widgets.HwDisplayText{width: Fill text: ""}
                        tag := mod.widgets.HwDisplayText{width: Fit text: ""}
                        use_btn := ButtonFlatter{width: Fit text: "Use"}
                        tweak_btn := ButtonFlatter{width: Fit text: "Tweak…"}
                        copy_btn := ButtonFlatter{width: Fit text: "Copy"}
                        hide_btn := ButtonFlatter{width: Fit text: "Hide"}
                        del_btn := ButtonFlatter{width: Fit text: "Delete"}
                    }
                }
                }
                sm_tweak := View{
                    visible: false
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 6
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        sm_tweak_title := mod.widgets.HwDisplayText{width: Fill text: ""}
                        sm_reset := ButtonFlatter{text: "Reset to original"}
                        sm_back := ButtonFlatter{text: "Back"}
                    }
                    sm_fields := PortalList{
                        width: Fill
                        height: Fill
                        Field := View{
                            width: Fill
                            height: 32
                            flow: Right
                            spacing: 6
                            align: Align{y: 0.5}
                            fchip := SolidView{width: 14 height: 14 draw_bg +: {color: #x00000000}}
                            flabel := mod.widgets.HwDisplayText{width: 230 text: ""}
                            fvalue := mod.widgets.HwTextInput{width: Fill height: 28 empty_text: "(template default)"}
                        }
                    }
                }
            }
        }
        // The rows' right-click menu, over the drawer.
        sm_menu := mod.widgets.HwMenuLayer{}
    }
}

/// The knobs the Tweak editor offers: (assigned path, label).
const TWEAK_FIELDS: &[(&str, &str)] = &[
    ("mod.widgets.hw_palette.accent", "Accent (lights, selection)"),
    ("mod.widgets.hw_palette.screen_bg", "List glass"),
    ("mod.widgets.hw_palette.screen_text", "List text"),
    ("mod.widgets.hw_palette.screen_muted", "List text, secondary"),
    ("mod.widgets.hw_palette.panel_text", "Panel type"),
    ("mod.widgets.hw_palette.panel_muted", "Panel type, secondary"),
    ("mod.widgets.hw_palette.menu_bg", "Menus"),
    ("mod.widgets.HwChassis.draw_bg.ground", "Front colour"),
    ("mod.widgets.HwChassis.draw_bg.pattern", "Front finish: kind, scale, strength, aniso"),
    ("mod.widgets.HwChassis.draw_bg.sheen", "Front sheen"),
    ("mod.widgets.Window.pass.clear_color", "Window behind the front"),
    ("mod.widgets.HwPlate.draw_bg.ground", "Wings colour"),
    ("mod.widgets.HwPlate.draw_bg.pattern", "Wings finish"),
    ("mod.widgets.HwPanel.draw_bg.ground", "Panels colour"),
    ("mod.widgets.HwBezel.draw_bg.ground", "Screen surround"),
    ("mod.widgets.HwKey.draw_bg.ground", "Keys colour"),
    ("mod.widgets.HwKey.draw_bg.pattern", "Keys finish"),
    ("mod.widgets.HwKeyText.draw_text.color", "Key legends"),
    ("mod.widgets.hw_palette.spill_color", "Light colour, style-wide"),
    ("mod.widgets.hw_palette.spill_mono", "Light: 0 own colours … 1 mono"),
    ("mod.widgets.hw_palette.spill_knee", "Light ceiling (0 = none)"),
    ("mod.widgets.hw_palette.spill_dark", "Light on black surfaces"),
    ("mod.widgets.HwKey.draw_bg.spill", "Keys take light (x), shade (y)"),
    ("mod.widgets.HwPlate.draw_bg.spill", "Wings take light (x), shade (y)"),
    ("mod.widgets.HwMilk.light", "The visualizer lights the device"),
    ("mod.widgets.HwMilk.light_knee", "Visualizer light ceiling"),
    ("mod.widgets.HwSpectrum.light", "Spectrum lights the device"),
    ("mod.widgets.hw_palette.map0", "Colour map: dark"),
    ("mod.widgets.hw_palette.map1", "Colour map 1"),
    ("mod.widgets.hw_palette.map2", "Colour map 2"),
    ("mod.widgets.hw_palette.map3", "Colour map: bright"),
    ("mod.widgets.hw_palette.map_strength", "Colour map amount"),
    ("mod.widgets.hw_palette.map_default", "Colour map on (1) by default"),
    ("mod.widgets.hw_palette.screen_filter", "Screen filter kind"),
    ("mod.widgets.hw_palette.screen_filter_strength", "Screen filter strength"),
    ("mod.widgets.HwKnobFace.draw_bg.knob", "Knob: kind, cap, knurls, depth"),
    ("mod.widgets.HwKnobFace.draw_bg.ring", "Knob LED ring"),
    ("mod.widgets.HwMeterFace.draw_bg.meter", "Meter: kind, segments, hot, needle"),
];

/// A `#xRRGGBB` / `#RRGGBB` value as a colour, for the field's chip.
fn hex_color(value: &str) -> Option<Vec4f> {
    let hex = value.trim().strip_prefix("#x").or_else(|| value.trim().strip_prefix('#'))?;
    let hex: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 6 {
        return None;
    }
    Some(Vec4f::from_u32((u32::from_str_radix(&hex, 16).ok()? << 8) | 0xff))
}

#[derive(Script, ScriptHook, Widget)]
pub struct StyleManager {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    rows: Vec<StyleInfo>,
    /// The row the title field and the menu act on.
    #[rust]
    selected: Option<String>,
    /// The style the Tweak editor edits (a user style).
    #[rust]
    tweaking: Option<String>,
    /// The row the open menu belongs to.
    #[rust]
    menu_row: Option<String>,
    #[rust]
    status: Option<String>,
}

impl StyleManager {
    pub fn refresh(&mut self, cx: &mut Cx) {
        self.rows = style_store::list_all();
        self.view.redraw(cx);
    }

    fn say(&mut self, cx: &mut Cx, text: impl Into<String>) {
        self.status = Some(text.into());
        self.view.redraw(cx);
    }

    fn row(&self, name: &str) -> Option<&StyleInfo> {
        self.rows.iter().find(|r| r.name == name)
    }

    fn act(&mut self, cx: &mut Cx, op: &str, name: &str) {
        let result: Result<String, String> = match op {
            "use" => {
                hw::set_style(cx, name);
                Ok(format!("Showing {}", style_store::title_of(name)))
            }
            "copy" => style_store::duplicate(name, None).map(|made| {
                self.selected = Some(made.clone());
                format!("Copied to \"{}\"", style_store::title_of(&made))
            }),
            "hide" => {
                let hidden = self.row(name).is_some_and(|r| r.hidden);
                style_store::set_hidden(name, !hidden).map(|_| if hidden { "Shown in the picker again".into() } else { "Hidden from the picker".into() })
            }
            "delete" => {
                let on_screen = hw::current(cx) == name;
                style_store::delete(name).map(|_| {
                    if on_screen {
                        hw::set_style(cx, "brushed");
                    }
                    format!("Deleted \"{}\" · UNDO brings it back", name)
                })
            }
            "reset" => style_store::reset_to_original(name).map(|_| {
                if hw::current(cx) == name {
                    ai::apply_style_later(cx, name);
                }
                "Back to the original".into()
            }),
            "rename" => {
                let title = self.view.text_input(cx, ids!(sm_title)).text();
                if title.trim().is_empty() {
                    self.view.text_input(cx, ids!(sm_title)).set_text(cx, &style_store::title_of(name));
                    self.view.text_input(cx, ids!(sm_title)).set_key_focus(cx);
                    Ok("Type the new title, then Rename".into())
                } else {
                    let on_screen = hw::current(cx) == name;
                    style_store::rename(name, title.trim()).map(|new| {
                        if on_screen {
                            hw::set_style(cx, &new);
                        }
                        self.selected = Some(new);
                        format!("Renamed to \"{}\"", title.trim())
                    })
                }
            }
            "tweak" => style_store::writable(name).map(|target| {
                self.tweaking = Some(target.clone());
                self.selected = Some(target.clone());
                if hw::current(cx) != target {
                    hw::set_style(cx, &target);
                }
                if target != name {
                    format!("Built-ins stay as they are: tweaking a copy, \"{}\"", style_store::title_of(&target))
                } else {
                    format!("Tweaking \"{}\": Return applies a value", style_store::title_of(&target))
                }
            }),
            "up" | "down" => {
                let mut names: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
                if let Some(at) = names.iter().position(|n| n == name) {
                    let to = if op == "up" { at.saturating_sub(1) } else { (at + 1).min(names.len() - 1) };
                    let item = names.remove(at);
                    names.insert(to, item);
                }
                style_store::reorder(&names).map(|_| "Moved".into())
            }
            _ => Ok(String::new()),
        };
        match result {
            Ok(text) => self.say(cx, text),
            Err(e) => self.say(cx, e),
        }
        self.refresh(cx);
    }

    fn open_menu(&mut self, cx: &mut Cx, name: &str, at: DVec2) {
        let Some(row) = self.row(name).cloned() else { return };
        self.menu_row = Some(name.to_string());
        let mut rows = vec![
            MenuRow::new(live_id!(m_use), "Use"),
            MenuRow::new(live_id!(m_tweak), if row.builtin { "Tweak a copy…" } else { "Tweak…" }),
            MenuRow::new(live_id!(m_copy), "Duplicate"),
            MenuRow::new(live_id!(m_rename), "Rename…").enabled(!row.builtin),
            MenuRow::new(live_id!(m_hide), if row.hidden { "Show in picker" } else { "Hide from picker" }),
        ];
        if row.from.is_some() {
            rows.push(MenuRow::new(live_id!(m_reset), "Reset to original"));
        }
        rows.push(MenuRow::separator());
        rows.push(MenuRow::new(live_id!(m_up), "Move up"));
        rows.push(MenuRow::new(live_id!(m_down), "Move down"));
        rows.push(MenuRow::separator());
        rows.push(MenuRow::new(live_id!(m_delete), "Delete").enabled(!row.builtin).danger(true));
        self.view.menu_layer(cx, ids!(sm_menu)).open(cx, live_id!(style_row_menu), rows, Rect { pos: at, size: dvec2(0.0, 0.0) }, MenuPlace::At);
    }

    fn tweak_value(&mut self, cx: &mut Cx, field: usize, value: &str) {
        let Some(name) = self.tweaking.clone() else { return };
        let Some((target, _)) = TWEAK_FIELDS.get(field) else { return };
        let text = style_store::read(&name).unwrap_or_default();
        let next = if value.trim().is_empty() {
            // Empty: back to the template's default (drop the assignment).
            style_store::statements(&text).into_iter().filter(|(t, _)| t != target).map(|(_, s)| s).collect::<Vec<_>>().join("\n") + "\n"
        } else {
            style_store::upsert(&text, target, value)
        };
        match style_store::write(&name, &next) {
            Ok(()) => {
                ai::apply_style_later(cx, &name);
                self.say(cx, format!("{} = {}", target.trim_start_matches("mod.widgets."), value.trim()));
            }
            Err(e) => self.say(cx, e),
        }
    }
}

impl Widget for StyleManager {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape && self.view.visible() {
                if self.tweaking.take().is_some() {
                    self.view.redraw(cx);
                } else {
                    self.view.set_visible(cx, false);
                }
            }
        }
        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.rows.is_empty() {
            self.rows = style_store::list_all();
        }
        let tweaking = self.tweaking.clone();
        self.view.view(cx, ids!(sm_tweak)).set_visible(cx, tweaking.is_some());
        self.view.view(cx, ids!(sm_rows)).set_visible(cx, tweaking.is_none());
        if let Some(status) = self.status.take() {
            self.view.label(cx, ids!(sm_status)).set_text(cx, &status);
        }
        if let Some(name) = &tweaking {
            let title = format!("TWEAK · {}", style_store::title_of(name));
            self.view.label(cx, ids!(sm_tweak_title)).set_text(cx, &title);
            let from = style_store::read(name).and_then(|t| style_store::header(&t, "from"));
            self.view.button(cx, ids!(sm_reset)).set_visible(cx, from.is_some());
        }
        let current = hw::current(cx);
        let source = tweaking.as_ref().and_then(|n| style_store::read(n)).unwrap_or_default();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<ReorderList>() {
                list.set_item_range(cx, 0, self.rows.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(info) = self.rows.get(index).cloned() else { continue };
                    let row = list.item(cx, index, live_id!(Row));
                    let (fill, _dot) = hw::swatch(&info.name);
                    let mut chip = row.widget(cx, ids!(chip));
                    script_apply_eval!(cx, chip, {draw_bg +: {color: #(fill)}});
                    let mut title = info.title.clone();
                    if info.name == current {
                        title = format!("▸ {title}");
                    }
                    row.label(cx, ids!(title)).set_text(cx, &title);
                    let mut tags = vec![if info.builtin { "built-in" } else { "yours" }];
                    if info.hidden {
                        tags.push("hidden");
                    }
                    if info.generating {
                        tags.push("generating…");
                    }
                    row.label(cx, ids!(tag)).set_text(cx, &tags.join(" · "));
                    row.button(cx, ids!(hide_btn)).set_text(cx, if info.hidden { "Show" } else { "Hide" });
                    row.button(cx, ids!(del_btn)).set_visible(cx, !info.builtin);
                    let selected = self.selected.as_deref() == Some(info.name.as_str());
                    let mut bg = row.clone();
                    let tint = if selected { vec4(1.0, 1.0, 1.0, 0.08) } else { vec4(0.0, 0.0, 0.0, 0.0) };
                    script_apply_eval!(cx, bg, {draw_bg +: {color: #(tint)}});
                    row.draw_all(cx, &mut Scope::empty());
                }
            } else if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, TWEAK_FIELDS.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some((target, label)) = TWEAK_FIELDS.get(index) else { continue };
                    let row = list.item(cx, index, live_id!(Field));
                    row.label(cx, ids!(flabel)).set_text(cx, label);
                    let value = style_store::value_of(&source, target).unwrap_or_default();
                    let input = row.text_input(cx, ids!(fvalue));
                    if !input.key_focus(cx) {
                        input.set_text(cx, &value);
                    }
                    let color = hex_color(&value).unwrap_or(vec4(0.0, 0.0, 0.0, 0.0));
                    let mut chip = row.widget(cx, ids!(fchip));
                    script_apply_eval!(cx, chip, {draw_bg +: {color: #(color)}});
                    row.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
}

impl WidgetMatchEvent for StyleManager {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if self.view.button(cx, ids!(sm_close)).clicked(actions) {
            self.tweaking = None;
            self.view.set_visible(cx, false);
            return;
        }
        if self.view.button(cx, ids!(sm_undo)).clicked(actions) {
            match style_store::undo_delete() {
                Ok(name) => self.say(cx, format!("\"{}\" is back", style_store::title_of(&name))),
                Err(e) => self.say(cx, e),
            }
            self.refresh(cx);
        }
        if self.view.button(cx, ids!(sm_back)).clicked(actions) {
            self.tweaking = None;
            self.refresh(cx);
        }
        if self.view.button(cx, ids!(sm_reset)).clicked(actions) {
            if let Some(name) = self.tweaking.clone() {
                self.act(cx, "reset", &name);
            }
        }
        let title_field = self.view.text_input(cx, ids!(sm_title));
        let rename = self.view.button(cx, ids!(sm_rename)).clicked(actions) || title_field.returned(actions).is_some();
        if rename {
            if let Some(name) = self.selected.clone() {
                self.act(cx, "rename", &name);
            } else {
                self.say(cx, "Pick a row to rename");
            }
        }
        if self.view.button(cx, ids!(sm_new)).clicked(actions) {
            let title = title_field.text();
            let title = if title.trim().is_empty() { "My Style".to_string() } else { title.trim().to_string() };
            match style_store::create(&style_store::name_from_title(&title), Some(&title), None) {
                Ok(made) => {
                    self.selected = Some(made.clone());
                    self.say(cx, format!("Made \"{title}\" (empty: the base look). Tweak it or ask the AI."));
                }
                Err(e) => self.say(cx, e),
            }
            self.refresh(cx);
        }
        if let Some(id) = menu_picked(actions, live_id!(style_row_menu)) {
            if let Some(name) = self.menu_row.take() {
                let ops = [
                    (live_id!(m_use), "use"),
                    (live_id!(m_tweak), "tweak"),
                    (live_id!(m_copy), "copy"),
                    (live_id!(m_rename), "rename"),
                    (live_id!(m_hide), "hide"),
                    (live_id!(m_reset), "reset"),
                    (live_id!(m_up), "up"),
                    (live_id!(m_down), "down"),
                    (live_id!(m_delete), "delete"),
                ];
                if let Some((_, op)) = ops.iter().find(|(i, _)| *i == id) {
                    if *op == "rename" {
                        self.view.text_input(cx, ids!(sm_title)).set_text(cx, "");
                    }
                    self.act(cx, op, &name);
                }
            }
        }
        // The list: reorder, row keys, a click selects, a right-click opens the menu.
        let list = self.view.widget(cx, ids!(sm_list));
        let (reordered, items) = match list.borrow::<ReorderList>() {
            Some(inner) => (inner.reordered(actions), inner.items_with_actions(actions)),
            None => (None, Vec::new()),
        };
        if let Some((from, to)) = reordered {
            let mut names: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
            if from < names.len() {
                let item = names.remove(from);
                names.insert(to.min(names.len()), item);
                let _ = style_store::reorder(&names);
                self.refresh(cx);
            }
        }
        for (index, item) in items {
            let Some(name) = self.rows.get(index).map(|r| r.name.clone()) else { continue };
            for (btn, op) in [(ids!(use_btn), "use"), (ids!(tweak_btn), "tweak"), (ids!(copy_btn), "copy"), (ids!(hide_btn), "hide"), (ids!(del_btn), "delete")] {
                if item.button(cx, btn).clicked(actions) {
                    self.selected = Some(name.clone());
                    self.act(cx, op, &name);
                }
            }
            if let Some(down) = item.as_view().finger_down(actions) {
                self.selected = Some(name.clone());
                self.view.text_input(cx, ids!(sm_title)).set_text(cx, &style_store::title_of(&name));
                if down.mouse_button().is_some_and(|b| b.is_secondary()) {
                    self.open_menu(cx, &name, down.abs);
                }
                self.view.redraw(cx);
            }
        }
        // The Tweak fields: Return applies.
        let fields = self.view.portal_list(cx, ids!(sm_fields));
        for (index, item) in fields.items_with_actions(actions) {
            if let Some((value, _)) = item.text_input(cx, ids!(fvalue)).returned(actions) {
                self.tweak_value(cx, index, &value);
            }
        }
    }
}

/// The style menu: user styles marked in the key column ("yours", or
/// "generating…" while the AI writes one), and the manager's row.
pub fn extend_style_menu(rows: &mut Vec<MenuRow>) {
    for info in style_store::list_all() {
        let mark = if info.generating {
            "generating…"
        } else if info.user() {
            "yours"
        } else {
            continue;
        };
        let id = LiveId::from_str(&info.name);
        if let Some(row) = rows.iter_mut().find(|r| r.id == id) {
            row.shortcut = mark.to_string();
        }
    }
    rows.push(MenuRow::separator());
    rows.push(MenuRow::new(live_id!(edit_styles), "Edit styles…"));
}

// ---- the chat's place on the device ----

/// Open or fold the chat pane (the layout's `chat_split`).
/// Fold the chat pane open or shut (the ≡ menu's AI entry).
pub(crate) fn toggle_chat(cx: &mut Cx, ui: &WidgetRef) {
    let open = {
        let ai = cx.global::<ai::ThemeAi>();
        ai.chat_open = !ai.chat_open;
        ai.chat_open
    };
    show_chat(cx, ui, open);
}

fn show_chat(cx: &mut Cx, ui: &WidgetRef, open: bool) {
    // A layout without a place for it shows it as a drawer (layout.rs).
    ui.hw_stage(cx, ids!(stage)).set_chat_open(cx, open);
    let split = ui.splitter(cx, ids!(chat_split));
    let collapse = if open { SplitterCollapse::None } else { SplitterCollapse::B };
    if split.collapse() != collapse {
        split.set_collapse(cx, collapse);
    }
    if open {
        ui.widget(cx, ids!(chat_panel)).redraw(cx);
    }
}

fn open_manager(cx: &mut Cx, ui: &WidgetRef) {
    let manager = ui.widget(cx, ids!(style_manager));
    if let Some(mut inner) = manager.borrow_mut::<StyleManager>() {
        inner.view.set_visible(cx, true);
        inner.selected = Some(hw::current(cx));
        inner.tweaking = None;
        inner.refresh(cx);
    };
}

/// Key presses and restyles: Cmd+I folds the chat, a restyle puts the
/// pane and the Live preview switch back the way they were.
pub fn handle_event(cx: &mut Cx, event: &Event, ui: &WidgetRef) {
    if let Event::KeyDown(key) = event {
        if key.key_code == KeyCode::KeyI && key.modifiers.is_primary() {
            let open = {
                let ai = cx.global::<ai::ThemeAi>();
                ai.chat_open = !ai.chat_open;
                ai.chat_open
            };
            show_chat(cx, ui, open);
        }
    }
    if let Event::LiveEdit | Event::Startup = event {
        let open = cx.has_global::<ai::ThemeAi>() && cx.global::<ai::ThemeAi>().chat_open;
        show_chat(cx, ui, open);
        let live = ai::live(cx);
        ui.check_box(cx, ids!(live_preview)).set_active(cx, live, Animate::No);
        let manager = ui.widget(cx, ids!(style_manager));
        if let Some(mut inner) = manager.borrow_mut::<StyleManager>() {
            inner.refresh(cx);
        };
    }
}

pub fn handle_actions(cx: &mut Cx, actions: &Actions, ui: &WidgetRef) {
    if ui.button(cx, ids!(ai_key)).clicked(actions) {
        let open = {
            let ai = cx.global::<ai::ThemeAi>();
            ai.chat_open = !ai.chat_open;
            ai.chat_open
        };
        show_chat(cx, ui, open);
    }
    if let Some(on) = ui.check_box(cx, ids!(live_preview)).changed(actions) {
        ai::set_live(cx, on);
    }
    if ui.button(cx, ids!(chat_styles)).clicked(actions) {
        open_manager(cx, ui);
    }
    if menu_picked(actions, live_id!(style_menu)) == Some(live_id!(edit_styles)) {
        open_manager(cx, ui);
    }
}
