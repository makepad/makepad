//! One `CalculatorView` owns the document, layout, navigation, and storage.

use crate::engine::{display_expression, format_number, tokenize, MAX_SOURCE_BYTES};
use crate::model::{
    apply, rebuild_derived, display_expression_line, display_result, font_size_for_pt, status_line, AngleMode,
    BinaryOp, CalculatorDoc, Command, Function, LayoutMetrics, Phase, Route, UiState, WidthClass,
};
use crate::seed;
use makepad_widgets::makepad_platform::storage::{
    StorageHandle, StorageRequestId, StorageResponse, StorageResult,
};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let CalcKey = Button{
        width: 85 height: 85
        margin: 0 padding: 0
        grab_key_focus: false
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            border_radius: 42.5
            border_size: 0.0
            color: theme.color_inset
            color_hover: theme.color_inset_hover
            color_down: theme.color_inset_down
        }
        draw_text +: {
            color: theme.color_text
            color_hover: theme.color_text
            color_down: theme.color_text
            color_focus: theme.color_text
            text_style: theme.font_regular{font_size: 24}
        }
        animator +: {
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.08}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.08}
                        down: Forward {duration: 0.0}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: snap(1.0)}
                        draw_text: {down: 0.0, hover: snap(1.0)}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
        }
    }

    let UtilKey = CalcKey{
        draw_bg +: {color: theme.color_inset_1}
        draw_text +: {text_style: theme.font_regular{font_size: 18}}
    }
    let OpKey = CalcKey{
        draw_bg +: {
            color: theme.color_makepad
            color_hover: theme.color_makepad
            color_down: theme.color_outset_active
        }
        draw_text +: {
            color: theme.color_w
            color_hover: theme.color_w
            color_down: theme.color_w
            text_style: theme.font_regular{font_size: 25.5}
        }
    }
    let SciKey = CalcKey{
        draw_text +: {text_style: theme.font_regular{font_size: 13.5}}
    }

    let BasicPad = View{
        width: Fill height: Fit
        flow: Down spacing: 10
        brow1 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_clear := UtilKey{text: "AC"}
            k_sign := UtilKey{text: "⁺∕₋"}
            k_percent := UtilKey{text: "%"}
            k_div := OpKey{text: "÷"}
        }
        brow2 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_7 := CalcKey{text: "7"}
            k_8 := CalcKey{text: "8"}
            k_9 := CalcKey{text: "9"}
            k_mul := OpKey{text: "×"}
        }
        brow3 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_4 := CalcKey{text: "4"}
            k_5 := CalcKey{text: "5"}
            k_6 := CalcKey{text: "6"}
            k_sub := OpKey{text: "−"}
        }
        brow4 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_1 := CalcKey{text: "1"}
            k_2 := CalcKey{text: "2"}
            k_3 := CalcKey{text: "3"}
            k_add := OpKey{text: "+"}
        }
        brow5 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_back := UtilKey{text: "⌫"}
            k_0 := CalcKey{text: "0"}
            k_dot := CalcKey{text: "."}
            k_eq := OpKey{text: "="}
        }
    }

    let ScientificPad = View{
        width: Fill height: Fit
        flow: Down spacing: 10
        srow1 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_mc := SciKey{text: "mc"}
            k_mplus := SciKey{text: "m+"}
            k_mminus := SciKey{text: "m−"}
            k_mr := SciKey{text: "mr"}
            k_rad := SciKey{text: "Rad"}
        }
        srow2 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_lparen := SciKey{text: "("}
            k_rparen := SciKey{text: ")"}
            k_square := SciKey{text: "x²"}
            k_cube := SciKey{text: "x³"}
            k_power := SciKey{text: "xʸ"}
        }
        srow3 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_inv := SciKey{text: "1/x"}
            k_sqrt := SciKey{text: "√"}
            k_cbrt := SciKey{text: "∛"}
            k_exp := SciKey{text: "eˣ"}
            k_pow10 := SciKey{text: "10ˣ"}
        }
        srow4 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_fact := SciKey{text: "x!"}
            k_sin := SciKey{text: "sin"}
            k_cos := SciKey{text: "cos"}
            k_tan := SciKey{text: "tan"}
            k_copy_sci := SciKey{text: "Copy"}
        }
        srow5 := View{
            width: Fill height: 85 flow: Right spacing: 10
            k_ln := SciKey{text: "ln"}
            k_log := SciKey{text: "log"}
            k_e := SciKey{text: "e"}
            k_pi := SciKey{text: "π"}
            k_ee := SciKey{text: "EE"}
        }
    }

    let HistoryList = PortalList{
        width: Fill height: Fill
        Item := View{
            width: Fill height: 76
            flow: Overlay
            recall := Button{
                width: Fill height: Fill
                text: "" margin: 0 padding: 0
                grab_key_focus: false
                draw_bg +: {
                    color: theme.color_u_hidden
                    color_hover: theme.color_bg_highlight
                    color_down: theme.color_bg_highlight
                    border_radius: 12.0
                    border_size: 0.0
                }
            }
            labels := View{
                width: Fill height: Fill
                flow: Down spacing: 4
                padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
                expression := Label{
                    width: Fill height: 20
                    padding: 0 max_lines: 1
                    text_overflow: Ellipsis
                    align: Align{x: 1.0}
                    draw_text +: {
                        color: mix(theme.color_text, theme.color_bg_app, 0.2)
                        text_style: theme.font_regular{font_size: 10.5}
                    }
                }
                result := Label{
                    width: Fill height: 30
                    padding: 0 max_lines: 1
                    align: Align{x: 1.0}
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 18}
                    }
                }
            }
        }
    }

    let EmptyHistory = View{
        width: Fill height: Fill
        visible: false
        flow: Down spacing: 8
        align: Align{x: 0.5 y: 0.4}
        empty_title := Label{
            width: Fill
            padding: 0
            align: Align{x: 0.5}
            text: "No calculations yet"
            draw_text +: {
                color: theme.color_text
                text_style: theme.font_regular{font_size: 12.75}
            }
        }
        empty_body := Label{
            width: Fill
            padding: 0
            align: Align{x: 0.5}
            text: "Results appear here after ="
            draw_text +: {
                color: mix(theme.color_text, theme.color_bg_app, 0.2)
                text_style: theme.font_regular{font_size: 10.5}
            }
        }
    }

    let Tape = GlassPanel{
        width: 280 height: Fill
        clip_x: true clip_y: true
        padding: 12 spacing: 12
        draw_bg +: {corner_radius: 20.0 fallback_color: theme.color_bg_container}
        header := View{
            width: Fill height: 44
            flow: Right align: Align{y: 0.5}
            tape_title := Label{
                text: "History"
                padding: 0
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_bold{font_size: 12.75}
                }
            }
            spacer := View{width: Fill height: 1}
            tape_clear := Button{
                text: "Clear" height: 44 width: Fit
                padding: Inset{left: 10 right: 10}
            }
        }
        tape_list := HistoryList{}
        tape_empty := EmptyHistory{}
    }

    let IconBtn = glass.GlassButton{
        width: 44 height: 44 text: "" padding: 0 spacing: 0
        icon_walk: Walk{width: 20 height: 20}
    }

    let CalculatorSurface = View{
        width: Fill height: Fill
        flow: Down
        toolbar := View{
            width: Fill height: 48
            flow: Right align: Align{y: 0.5} spacing: 8
            btn_history := IconBtn{
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/history.svg")
                    color: theme.color_text
                }
            }
            mode_label := Label{
                text: "Scientific"
                padding: 0
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_bold{font_size: 12.75}
                }
            }
            spacer := View{width: Fill height: 1}
            btn_retry := Button{
                visible: false
                text: "Retry" height: 44 width: Fit
                padding: Inset{left: 12 right: 12}
            }
            btn_reset := Button{
                visible: false
                text: "Reset" height: 44 width: Fit
                padding: Inset{left: 12 right: 12}
            }
            btn_copy := IconBtn{
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/copy.svg")
                    color: theme.color_text
                }
            }
        }
        short_bar := View{
            visible: false
            width: Fill height: 44
            flow: Right align: Align{y: 0.5} spacing: 8
            short_history := IconBtn{
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/history.svg")
                    color: theme.color_text
                }
            }
            short_status := Label{
                width: Fit height: Fill
                padding: 0
                align: Align{y: 1.0}
                draw_text +: {
                    color: mix(theme.color_text, theme.color_bg_app, 0.2)
                    text_style: theme.font_regular{font_size: 9}
                }
            }
            short_retry := Button{visible: false text: "Retry" width: Fit height: 44}
            short_reset := Button{visible: false text: "Reset" width: Fit height: 44}
            short_nums := View{
                width: Fill height: Fill
                flow: Down
                short_expr := Label{
                    width: Fill height: 16 padding: 0 max_lines: 1
                    align: Align{x: 1.0 y: 0.5}
                    draw_text +: {
                        color: mix(theme.color_text, theme.color_bg_app, 0.2)
                        text_style: theme.font_regular{font_size: 10.5}
                    }
                }
                short_result := Label{
                    width: Fill height: 28 padding: 0 max_lines: 1
                    align: Align{x: 1.0 y: 0.5}
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 21}
                    }
                }
            }
        }
        air := View{width: Fill height: Fill}
        display := View{
            width: Fill height: 128
            flow: Down
            expression_scroll := ScrollXView{
                width: Fill height: 28
                expression := Label{
                    width: Fit height: Fill padding: 0 max_lines: 1
                    align: Align{x: 1.0 y: 0.5}
                    draw_text +: {
                        color: mix(theme.color_text, theme.color_bg_app, 0.2)
                        text_style: theme.font_regular{font_size: 16.5}
                    }
                }
            }
            result := Label{
                width: Fill height: 84
                padding: 0 max_lines: 1
                align: Align{x: 1.0 y: 1.0}
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_regular{font_size: 60}
                }
            }
            status := Label{
                width: Fill height: 16 padding: 0
                draw_text +: {
                    color: mix(theme.color_text, theme.color_bg_app, 0.2)
                    text_style: theme.font_regular{font_size: 9.75}
                }
            }
        }
        gap := View{width: Fill height: 16}
        pads_scroll := ScrollYView{
            width: Fill height: Fill
            pads := View{
                width: Fill height: Fit
                flow: Right spacing: 16
                scientific := ScientificPad{}
                basic := BasicPad{}
            }
        }
    }

    mod.widgets.CalculatorViewBase = #(CalculatorView::register_widget(vm))
    mod.widgets.CalculatorView = set_type_default() do mod.widgets.CalculatorViewBase{
        width: Fill height: Fill
        flow: Overlay
        text_color: theme.color_text
        error_color: theme.color_error
        background := SolidView{
            width: Fill height: Fill
            draw_bg +: {color: theme.color_bg_app}
        }
        navigation := StackNavigation{
            root_view +: {
                width: Fill height: Fill
                flow: Right spacing: 20 padding: 20
                tape := Tape{}
                calculator := CalculatorSurface{}
            }
            history_page := StackNavigationView{
                header +: {
                    height: 56
                    padding: Inset{top: 6 bottom: 6}
                    content +: {
                        title_container +: {
                            title +: {
                                text: "History"
                                draw_text +: {
                                    text_style: theme.font_bold{font_size: 12.75}
                                }
                            }
                        }
                        button_container +: {
                            width: Fill
                            flow: Right align: Align{y: 0.5}
                            left_button +: {width: 44 height: 44}
                            View{width: Fill height: 1}
                            page_clear := Button{
                                text: "Clear" height: 44 width: Fit
                                padding: Inset{left: 12 right: 12}
                            }
                        }
                    }
                }
                body +: {
                    margin: Inset{top: 56}
                    padding: Inset{left: 16 right: 16}
                    flow: Overlay
                    page_list := HistoryList{}
                    page_empty := EmptyHistory{}
                }
            }
        }
    }
}

const STORAGE_KEY: &str = "calculator.json";

#[derive(Script, ScriptHook, Widget)]
pub struct CalculatorView {
    #[deref]
    view: View,
    #[rust(seed::initial())]
    doc: CalculatorDoc,
    #[rust]
    ui: UiState,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    load_id: Option<StorageRequestId>,
    #[rust]
    save_id: Option<StorageRequestId>,
    #[rust]
    save_revision: u64,
    #[rust]
    revision: u64,
    #[rust]
    acked_revision: u64,
    #[rust]
    loaded: bool,
    #[live]
    text_color: Vec4f,
    #[live]
    error_color: Vec4f,
    #[rust]
    quit_after_save: bool,
    #[rust]
    started: bool,
    #[rust]
    last_layout: Option<LayoutMetrics>,
    #[rust]
    last_size: Vec2d,
    #[rust]
    chrome_gen: u64,
}

impl CalculatorView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    pub fn doc(&self) -> &CalculatorDoc {
        &self.doc
    }

    #[cfg(test)]
    pub(crate) fn apply_command_for_test(&mut self, cmd: Command) {
        let _ = apply(&mut self.doc, &mut self.ui, cmd);
    }

    pub fn ai_summary(&self) -> String {
        format!(
            "Calculator · {} · result {}",
            self.doc.angle.as_str(),
            display_result(&self.doc, &self.ui)
        )
    }

    pub fn ai_answer(
        &self,
        call: &makepad_ai_services::wire::ServiceCall,
    ) -> makepad_ai_services::wire::ToolResult {
        crate::ai::answer(&self.doc, call)
    }

    pub fn shutdown(&mut self, cx: &mut Cx) {
        // Keep the same single-writer ordering as normal saves. A second SET
        // could finish first and then be overwritten by the older document.
        if self.revision > self.acked_revision {
            self.ui.save_failed = false;
            self.persist(cx);
            if self.save_id.is_some() && self.save_revision < self.revision {
                // InstanceParts::shutdown cannot defer teardown. The host
                // must retain this root and deliver storage events to finish.
                log!("calculator: shutdown still has an unsent revision; host must defer teardown until saves complete");
            }
        }
    }

    pub fn handle_close(&mut self, cx: &mut Cx) -> bool {
        if self.save_id.is_some() || self.revision > self.acked_revision {
            self.quit_after_save = true;
            self.persist(cx);
            false
        } else {
            true
        }
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(storage) = self.storage.as_ref() {
            self.load_id = Some(storage.get(cx, STORAGE_KEY));
            self.ui.storage_status = Some("Loading…".into());
        } else {
            self.loaded = true;
        }
        self.sync_chrome(cx);
    }

    fn persist(&mut self, cx: &mut Cx) {
        if !self.loaded || self.ui.load_failed || self.ui.save_failed {
            return;
        }
        if self.save_id.is_some() {
            return;
        }
        if self.revision <= self.acked_revision {
            if self.quit_after_save {
                cx.quit();
            }
            return;
        }
        let Some(storage) = self.storage.as_ref() else {
            self.acked_revision = self.revision;
            if self.quit_after_save {
                cx.quit();
            }
            return;
        };
        let bytes = self.doc.to_bytes();
        self.save_id = Some(storage.set(cx, STORAGE_KEY, bytes));
        self.save_revision = self.revision;
    }

    fn load_error(&mut self, message: String) {
        self.loaded = false;
        self.ui.load_failed = true;
        self.ui.awaiting_reset = true;
        self.ui.storage_status = Some(message);
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.load_id == Some(response.request_id) {
                self.load_id = None;
                match &response.result {
                    Ok(StorageResult::Value(bytes)) => {
                        let parsed = match bytes {
                            None => Ok(seed::initial()),
                            Some(bytes) => CalculatorDoc::from_bytes(bytes),
                        };
                        match parsed {
                            Ok(doc) => {
                                self.doc = doc;
                                self.loaded = true;
                                self.ui.load_failed = false;
                                self.ui.awaiting_reset = false;
                                self.ui.storage_status = None;
                                rebuild_derived(&mut self.doc, &mut self.ui);
                                if bytes.is_none() {
                                    self.revision = self.revision.saturating_add(1);
                                    self.persist(cx);
                                }
                            }
                            Err(error) => self.load_error(format!("Could not load calculator.json: {error}")),
                        }
                    }
                    Err(error) => self.load_error(format!("Load failed: {error}")),
                    _ => self.load_error("Unexpected load response".into()),
                }
            } else if self.save_id == Some(response.request_id) {
                self.save_id = None;
                match &response.result {
                    Ok(StorageResult::Unit) => {
                        self.acked_revision = self.acked_revision.max(self.save_revision);
                        self.ui.save_failed = false;
                        self.ui.storage_status = None;
                        self.persist(cx);
                    }
                    result => {
                        let message = match result {
                            Err(error) => format!("Save failed: {error}"),
                            _ => "Unexpected save response".into(),
                        };
                        self.ui.save_failed = true;
                        self.ui.storage_status = Some(message);
                        self.quit_after_save = false;
                    }
                }
            }
        }
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn dispatch(&mut self, cx: &mut Cx, cmd: Command) {
        // Dispatch is reached only by this instance's command or focused
        // input. Include Retry/Reset, whose handlers return before the reducer.
        self.focus_calculator(cx);
        match cmd {
            Command::RetryStorage => {
                if self.ui.save_failed {
                    self.ui.save_failed = false;
                    self.ui.storage_status = Some("Saving…".into());
                    self.persist(cx);
                } else if self.ui.load_failed && self.load_id.is_none() {
                    if let Some(storage) = self.storage.as_ref() {
                        self.load_id = Some(storage.get(cx, STORAGE_KEY));
                        self.ui.storage_status = Some("Retrying…".into());
                    }
                }
                self.sync_chrome(cx);
                return;
            }
            Command::ResetStorage => {
                if !self.ui.load_failed { return; }
                // Explicit reset invalidates any late load/retry response.
                self.load_id = None;
                self.doc = seed::initial();
                self.ui = UiState::default();
                self.loaded = true;
                self.revision = self.revision.saturating_add(1);
                self.persist(cx);
                self.sync_chrome(cx);
                self.view.redraw(cx);
                return;
            }
            _ => {}
        }
        if !self.loaded || self.ui.load_failed { return; }
        let out = apply(&mut self.doc, &mut self.ui, cmd);
        if out.changed {
            self.revision = self.revision.saturating_add(1);
            self.persist(cx);
        }
        if let Some(text) = out.copied {
            cx.copy_to_clipboard(&text);
        }
        if out.open_history {
            self.stack_navigation(cx, ids!(navigation))
                .push(cx, live_id!(history_page));
        }
        if out.close_history {
            self.stack_navigation(cx, ids!(navigation)).pop_to_root(cx);
        }
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn sync_chrome(&mut self, cx: &mut Cx) {
        let expr = display_expression_line(&self.doc);
        let result = display_result(&self.doc, &self.ui);
        let status = status_line(&self.doc, &self.ui);
        self.label(cx, ids!(expression)).set_text(cx, &expr);
        self.label(cx, ids!(result)).set_text(cx, &result);
        self.label(cx, ids!(status)).set_text(cx, &status);
        self.label(cx, ids!(short_expr)).set_text(cx, &expr);
        self.label(cx, ids!(short_result)).set_text(cx, &result);
        self.label(cx, ids!(short_status)).set_text(cx, &status);
        let rad = if self.doc.angle == AngleMode::Radians {
            "Deg"
        } else {
            "Rad"
        };
        self.button(cx, ids!(k_rad)).set_text(cx, rad);
        let clear = if self.ui.clear_is_ac { "AC" } else { "C" };
        self.button(cx, ids!(k_clear)).set_text(cx, clear);
        let compact = self
            .last_layout
            .map(|l| l.class.width == WidthClass::Compact)
            .unwrap_or(false);
        self.label(cx, ids!(mode_label))
            .set_text(cx, if compact { "Basic" } else { "Scientific" });
        let color = if self.doc.session.phase == Phase::Error { self.error_color } else { self.text_color };
        for id in [ids!(result), ids!(short_result)] {
            self.label(cx, id).set_text_color(cx, color);
        }
        let failed = self.ui.load_failed || self.ui.save_failed;
        for id in [ids!(btn_retry), ids!(short_retry)] {
            self.widget(cx, id).set_visible(cx, failed);
        }
        for id in [ids!(btn_reset), ids!(short_reset)] {
            self.widget(cx, id).set_visible(cx, self.ui.load_failed);
        }
        self.widget(cx, ids!(btn_copy)).set_visible(cx, !failed);
        let empty = self.doc.history.is_empty();
        self.widget(cx, ids!(tape_empty)).set_visible(cx, empty);
        self.widget(cx, ids!(tape_list)).set_visible(cx, !empty);
        self.widget(cx, ids!(page_empty)).set_visible(cx, empty);
        self.widget(cx, ids!(page_list)).set_visible(cx, !empty);
        self.widget(cx, ids!(tape_clear)).set_disabled(cx, empty);
        self.widget(cx, ids!(page_clear)).set_disabled(cx, empty);
        self.chrome_gen = self.chrome_gen.wrapping_add(1);
    }

    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let metrics = LayoutMetrics::for_size(size.x, size.y);
        if self.last_layout.as_ref() == Some(&metrics) && (self.last_size - size).length() < 0.5 {
            return;
        }
        if metrics.class.history_tape() && self.ui.route == Route::History {
            self.ui.route = Route::Calculator;
            self.stack_navigation(cx, ids!(navigation)).pop_to_root(cx);
        }
        self.last_layout = Some(metrics);
        self.last_size = size;
        let pad = metrics.inset;
        if let Some(mut root) = self.view.view(cx, ids!(root_view)).borrow_mut() {
            root.layout.padding = Inset {
                left: pad,
                right: pad,
                top: if metrics.show_short_bar { 4.0 } else if metrics.class.width == WidthClass::Compact { 8.0 } else { pad },
                bottom: pad,
            };
            root.layout.spacing = metrics.col_sep;
        }
        self.widget(cx, ids!(tape))
            .set_visible(cx, metrics.show_tape);
        self.widget(cx, ids!(scientific))
            .set_visible(cx, metrics.show_scientific);
        self.widget(cx, ids!(toolbar))
            .set_visible(cx, metrics.show_toolbar);
        self.widget(cx, ids!(short_bar))
            .set_visible(cx, metrics.show_short_bar);
        self.widget(cx, ids!(display))
            .set_visible(cx, metrics.show_display);
        self.widget(cx, ids!(air))
            .set_visible(cx, metrics.show_display && !metrics.class.short);
        self.widget(cx, ids!(btn_history)).set_visible(
            cx,
            metrics.show_toolbar && metrics.class.width == WidthClass::Compact,
        );
        let mut tape = self.widget(cx, ids!(tape));
        script_apply_eval!(cx, tape, { width: #(metrics.tape_width) });
        let kw = metrics.key.width;
        let kh = metrics.key.height;
        let hg = metrics.key.h_gap;
        let vg = metrics.key.v_gap;
        let r = metrics.key.radius;
        let gap = metrics.key.sci_basic_gap;
        let sci_w = 5.0 * kw + 4.0 * hg;
        let basic_w = 4.0 * kw + 3.0 * hg;
        let mut pads = self.widget(cx, ids!(pads));
        script_apply_eval!(cx, pads, { spacing: #(gap) });
        let mut sci = self.widget(cx, ids!(scientific));
        script_apply_eval!(cx, sci, { width: #(sci_w) spacing: #(vg) });
        let mut basic = self.widget(cx, ids!(basic));
        script_apply_eval!(cx, basic, { width: #(basic_w) spacing: #(vg) });
        for row in [
            ids!(srow1),
            ids!(srow2),
            ids!(srow3),
            ids!(srow4),
            ids!(srow5),
            ids!(brow1),
            ids!(brow2),
            ids!(brow3),
            ids!(brow4),
            ids!(brow5),
        ] {
            let mut w = self.widget(cx, row);
            script_apply_eval!(cx, w, { height: #(kh) spacing: #(hg) });
        }
        for key in Self::all_keys() {
            let mut w = self.widget(cx, key);
            script_apply_eval!(cx, w, {
                width: #(kw) height: #(kh)
                draw_bg +: {border_radius: #(r)}
            });
        }
        let result_fs = font_size_for_pt(metrics.result_pt.max(32.0));
        let expr_fs = font_size_for_pt(metrics.expr_pt);
        let mut result = self.widget(cx, ids!(result));
        script_apply_eval!(cx, result, {
            height: #(if metrics.class.short { 28.0 } else if metrics.class.width == WidthClass::Compact { 92.0 } else { 84.0 })
            draw_text.text_style.font_size: #(result_fs)
        });
        let mut expression = self.widget(cx, ids!(expression));
        script_apply_eval!(cx, expression, { draw_text.text_style.font_size: #(expr_fs) });
        let digit_fs = font_size_for_pt(metrics.digit_pt);
        let op_fs = font_size_for_pt(metrics.op_pt);
        let sci_fs = font_size_for_pt(metrics.sci_pt);
        let util_fs = font_size_for_pt(metrics.util_pt);
        for key in [ids!(k_0), ids!(k_1), ids!(k_2), ids!(k_3), ids!(k_4), ids!(k_5), ids!(k_6), ids!(k_7), ids!(k_8), ids!(k_9), ids!(k_dot)] {
            let mut w = self.widget(cx, key);
            script_apply_eval!(cx, w, { draw_text.text_style.font_size: #(digit_fs) });
        }
        for key in [ids!(k_div), ids!(k_mul), ids!(k_sub), ids!(k_add), ids!(k_eq)] {
            let mut w = self.widget(cx, key);
            script_apply_eval!(cx, w, { draw_text.text_style.font_size: #(op_fs) });
        }
        for key in [
            ids!(k_mc), ids!(k_mplus), ids!(k_mminus), ids!(k_mr), ids!(k_rad),
            ids!(k_lparen), ids!(k_rparen), ids!(k_square), ids!(k_cube), ids!(k_power),
            ids!(k_inv), ids!(k_sqrt), ids!(k_cbrt), ids!(k_exp), ids!(k_pow10),
            ids!(k_fact), ids!(k_sin), ids!(k_cos), ids!(k_tan), ids!(k_copy_sci),
            ids!(k_ln), ids!(k_log), ids!(k_e), ids!(k_pi), ids!(k_ee),
        ] {
            let mut w = self.widget(cx, key);
            script_apply_eval!(cx, w, { draw_text.text_style.font_size: #(sci_fs) });
        }
        for key in [ids!(k_clear), ids!(k_sign), ids!(k_percent), ids!(k_back)] {
            let mut w = self.widget(cx, key);
            script_apply_eval!(cx, w, { draw_text.text_style.font_size: #(util_fs) });
        }
        let top_inset = if metrics.show_short_bar { 4.0 } else if metrics.class.width == WidthClass::Compact { 8.0 } else { pad };
        let toolbar_h = if metrics.show_toolbar { if metrics.class.width == WidthClass::Compact { 44.0 } else { 48.0 } } else if metrics.show_short_bar { 44.0 } else { 0.0 };
        let display_h = if !metrics.show_display { 0.0 } else if metrics.class.width == WidthClass::Compact { 136.0 } else { 128.0 };
        let gap_h = if metrics.show_short_bar { 8.0 } else { 16.0 };
        let keypad_h = 5.0 * kh + 4.0 * vg;
        let remaining = (size.y - top_inset - pad - toolbar_h - display_h - gap_h).max(0.0);
        let viewport_h = keypad_h.min(remaining);
        let mut toolbar = self.widget(cx, ids!(toolbar));
        script_apply_eval!(cx, toolbar, { height: #(toolbar_h) });
        let mut display = self.widget(cx, ids!(display));
        script_apply_eval!(cx, display, { height: #(display_h) });
        let mut air = self.widget(cx, ids!(air));
        script_apply_eval!(cx, air, { height: #((remaining - viewport_h).max(0.0)) });
        let mut viewport = self.widget(cx, ids!(pads_scroll));
        script_apply_eval!(cx, viewport, { height: #(viewport_h) });
        let mut gap = self.widget(cx, ids!(gap));
        script_apply_eval!(cx, gap, { height: #(if metrics.class.short { 8.0 } else { 16.0 }) });
        self.fit_result(cx, metrics);
        self.sync_chrome(cx);
    }

    fn fit_result(&mut self, cx: &mut Cx, metrics: LayoutMetrics) {
        let text = display_result(&self.doc, &self.ui);
        let max_pt = metrics.result_pt;
        let min_pt = if metrics.class.short {
            24.0
        } else if metrics.class.width == WidthClass::Compact {
            28.0
        } else {
            32.0
        };
        let width = self.widget(cx, ids!(result)).area().rect(cx).size.x;
        if width < 8.0 {
            return;
        }
        let mut pt = max_pt;
        if let Some(mut label) = self.label(cx, ids!(result)).borrow_mut() {
            loop {
                label.draw_text.text_style.font_size = font_size_for_pt(pt) as f32;
                let laid = label.draw_text.layout(
                    cx,
                    0.0,
                    0.0,
                    None,
                    false,
                    Align { x: 1.0, y: 0.5 },
                    &text,
                );
                if laid.size_in_lpxs.width as f64 <= width - 4.0 || pt <= min_pt {
                    break;
                }
                pt -= 2.0;
            }
        }
        if let Some(mut label) = self.label(cx, ids!(short_result)).borrow_mut() {
            label.draw_text.text_style.font_size = font_size_for_pt(metrics.result_pt.min(28.0)) as f32;
        }
    }

    fn all_keys() -> [&'static [LiveId]; 45] {
        [
            ids!(k_mc), ids!(k_mplus), ids!(k_mminus), ids!(k_mr), ids!(k_rad),
            ids!(k_lparen), ids!(k_rparen), ids!(k_square), ids!(k_cube), ids!(k_power),
            ids!(k_inv), ids!(k_sqrt), ids!(k_cbrt), ids!(k_exp), ids!(k_pow10),
            ids!(k_fact), ids!(k_sin), ids!(k_cos), ids!(k_tan), ids!(k_copy_sci),
            ids!(k_ln), ids!(k_log), ids!(k_e), ids!(k_pi), ids!(k_ee),
            ids!(k_clear), ids!(k_sign), ids!(k_percent), ids!(k_div),
            ids!(k_7), ids!(k_8), ids!(k_9), ids!(k_mul),
            ids!(k_4), ids!(k_5), ids!(k_6), ids!(k_sub),
            ids!(k_1), ids!(k_2), ids!(k_3), ids!(k_add),
            ids!(k_back), ids!(k_0), ids!(k_dot), ids!(k_eq),
        ]
    }

    fn handle_keys(&mut self, cx: &mut Cx, actions: &Actions) {
        const MAP: &[(&[LiveId], Command)] = &[
            (ids!(k_0), Command::Digit(0)),
            (ids!(k_1), Command::Digit(1)),
            (ids!(k_2), Command::Digit(2)),
            (ids!(k_3), Command::Digit(3)),
            (ids!(k_4), Command::Digit(4)),
            (ids!(k_5), Command::Digit(5)),
            (ids!(k_6), Command::Digit(6)),
            (ids!(k_7), Command::Digit(7)),
            (ids!(k_8), Command::Digit(8)),
            (ids!(k_9), Command::Digit(9)),
            (ids!(k_dot), Command::Decimal),
            (ids!(k_add), Command::Op(BinaryOp::Add)),
            (ids!(k_sub), Command::Op(BinaryOp::Subtract)),
            (ids!(k_mul), Command::Op(BinaryOp::Multiply)),
            (ids!(k_div), Command::Op(BinaryOp::Divide)),
            (ids!(k_eq), Command::Equals),
            (ids!(k_clear), Command::Clear),
            (ids!(k_sign), Command::Sign),
            (ids!(k_percent), Command::Percent),
            (ids!(k_back), Command::Backspace),
            (ids!(k_lparen), Command::LParen),
            (ids!(k_rparen), Command::RParen),
            (ids!(k_square), Command::Square),
            (ids!(k_cube), Command::Cube),
            (ids!(k_power), Command::Power),
            (ids!(k_inv), Command::Func(Function::Reciprocal)),
            (ids!(k_sqrt), Command::Func(Function::Sqrt)),
            (ids!(k_cbrt), Command::Func(Function::Cbrt)),
            (ids!(k_exp), Command::Func(Function::Exp)),
            (ids!(k_pow10), Command::Func(Function::Pow10)),
            (ids!(k_fact), Command::Factorial),
            (ids!(k_sin), Command::Func(Function::Sin)),
            (ids!(k_cos), Command::Func(Function::Cos)),
            (ids!(k_tan), Command::Func(Function::Tan)),
            (ids!(k_ln), Command::Func(Function::Ln)),
            (ids!(k_log), Command::Func(Function::Log10)),
            (ids!(k_e), Command::Constant(crate::model::Constant::E)),
            (ids!(k_pi), Command::Constant(crate::model::Constant::Pi)),
            (ids!(k_ee), Command::Ee),
            (ids!(k_mc), Command::MemoryClear),
            (ids!(k_mplus), Command::MemoryAdd),
            (ids!(k_mminus), Command::MemorySub),
            (ids!(k_mr), Command::MemoryRecall),
            (ids!(k_rad), Command::AngleToggle),
            (ids!(k_copy_sci), Command::Copy),
        ];
        for (id, cmd) in MAP {
            if self.button(cx, *id).clicked(actions) {
                self.dispatch(cx, *cmd);
            }
        }
        if self.button(cx, ids!(tape_clear)).clicked(actions)
            || self.button(cx, ids!(page_clear)).clicked(actions)
        {
            self.dispatch(cx, Command::ClearHistory);
        }
        if self.button(cx, ids!(btn_retry)).clicked(actions) || self.button(cx, ids!(short_retry)).clicked(actions) {
            self.dispatch(cx, Command::RetryStorage);
        }
        if self.button(cx, ids!(btn_reset)).clicked(actions) || self.button(cx, ids!(short_reset)).clicked(actions) {
            self.dispatch(cx, Command::ResetStorage);
        }
        if glass_clicked(&self.widget(cx, ids!(btn_copy)), cx, actions)
            || self.button(cx, ids!(btn_copy)).clicked(actions)
        {
            self.dispatch(cx, Command::Copy);
        }
        if glass_clicked(&self.widget(cx, ids!(btn_history)), cx, actions)
            || self.button(cx, ids!(btn_history)).clicked(actions)
            || glass_clicked(&self.widget(cx, ids!(short_history)), cx, actions)
            || self.button(cx, ids!(short_history)).clicked(actions)
        {
            self.dispatch(cx, Command::OpenHistory);
        }
        self.handle_history_clicks(cx, actions, ids!(tape_list));
        self.handle_history_clicks(cx, actions, ids!(page_list));
    }

    fn handle_history_clicks(&mut self, cx: &mut Cx, actions: &Actions, list_id: &[LiveId]) {
        let n = self.doc.history.len();
        if n == 0 {
            return;
        }
        let mut hit = None;
        if let Some(mut list) = self.portal_list(cx, list_id).borrow_mut() {
            for index in 0..n {
                let item = list.item(cx, index, live_id!(Item));
                if item.button(cx, ids!(recall)).clicked(actions) {
                    hit = self.doc.history.get(index).map(|e| e.id);
                    break;
                }
            }
        }
        if let Some(id) = hit {
            self.dispatch(cx, Command::RecallHistory(id));
        }
    }

    fn focus_calculator(&self, cx: &mut Cx) {
        let area = self.view.area();
        if !area.is_empty() { cx.set_key_focus(area); }
    }

    fn owns_keyboard_focus(&self, cx: &mut Cx, focus: Area) -> bool {
        !focus.is_empty() && (focus == self.view.area()
            || Self::all_keys().iter().any(|id| self.widget(cx, id).area() == focus))
    }

    fn handle_keyboard(&mut self, cx: &mut Cx, event: &Event, focus: Area) {
        if !self.owns_keyboard_focus(cx, focus) { return; }
        self.handle_focused_keyboard(cx, event);
    }

    fn handle_focused_keyboard(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::TextCopy(copy) => {
                *copy.response.borrow_mut() = Some(display_result(&self.doc, &self.ui));
            }
            Event::KeyDown(ke) => {
                if ke.is_repeat {
                    return;
                }
                let cmd = ke.modifiers.logo || ke.modifiers.control;
                if cmd && ke.key_code == KeyCode::KeyC {
                    self.dispatch(cx, Command::Copy);
                    return;
                }
                if cmd {
                    return;
                }
                match ke.key_code {
                    KeyCode::Escape => self.dispatch(cx, Command::AllClear),
                    KeyCode::Backspace | KeyCode::Delete => self.dispatch(cx, Command::Backspace),
                    KeyCode::ReturnKey | KeyCode::NumpadEnter | KeyCode::NumpadEquals => {
                        self.dispatch(cx, Command::Equals)
                    }
                    _ => {}
                }
            }
            Event::TextInput(te) => {
                if te.was_paste {
                    self.paste_expression(cx, &te.input);
                    return;
                }
                for ch in te.input.chars() {
                    if let Some(cmd) = command_from_char(ch) {
                        self.dispatch(cx, cmd);
                    }
                }
            }
            _ => {}
        }
    }

    fn paste_expression(&mut self, cx: &mut Cx, text: &str) {
        if !self.loaded || self.ui.load_failed { return; }
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_SOURCE_BYTES {
            return;
        }
        if tokenize(text).is_err() || matches!(crate::engine::parse(text), Err(crate::model::EvalError { code: crate::model::ErrorCode::Limit, .. })) {
            return;
        }
        self.doc.session.source = text.to_string();
        self.doc.session.phase = Phase::Editing;
        self.doc.session.repeat = None;
        self.doc.session.reuse_repeat = false;
        self.revision = self.revision.saturating_add(1);
        match crate::engine::preview(&self.doc.session.source, self.doc.angle) {
            crate::model::Preview::Value(v) => {
                self.doc.session.value = v;
                self.ui.last_error = None;
            }
            crate::model::Preview::Error(e) => {
                self.doc.session.phase = Phase::Error;
                self.ui.last_error = Some(e);
            }
            crate::model::Preview::Incomplete => {}
        }
        self.persist(cx);
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn draw_history_list(&self, cx: &mut Cx2d, list: &mut PortalList) {
        let n = self.doc.history.len();
        list.set_item_range(cx, 0, n);
        while let Some(index) = list.next_visible_item(cx) {
            if index >= n {
                continue;
            }
            let item = list.item(cx, index, live_id!(Item));
            let entry = &self.doc.history[index];
            item.label(cx, ids!(expression))
                .set_text(cx, &display_expression(&entry.source));
            item.label(cx, ids!(result))
                .set_text(cx, &format_number(entry.value));
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

fn glass_clicked(widget: &WidgetRef, _cx: &Cx, actions: &Actions) -> bool {
    widget
        .borrow::<GlassButton>()
        .is_some_and(|b| b.clicked(actions))
}

fn command_from_char(ch: char) -> Option<Command> {
    match ch {
        '0'..='9' => Some(Command::Digit(ch as u8 - b'0')),
        '.' => Some(Command::Decimal),
        '+' => Some(Command::Op(BinaryOp::Add)),
        '-' | '−' => Some(Command::Op(BinaryOp::Subtract)),
        '*' | '×' => Some(Command::Op(BinaryOp::Multiply)),
        '/' | '÷' => Some(Command::Op(BinaryOp::Divide)),
        '^' => Some(Command::Op(BinaryOp::Power)),
        '%' => Some(Command::Percent),
        '!' => Some(Command::Factorial),
        '(' => Some(Command::LParen),
        ')' => Some(Command::RParen),
        '=' => Some(Command::Equals),
        'e' | 'E' => None, // typed function names go through paste/lexer
        _ => None,
    }
}

impl Widget for CalculatorView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if let Event::WindowCloseRequested(ev) = event {
            if !self.handle_close(cx) {
                ev.accept_close.set(false);
                return;
            }
        }
        if let Event::BackPressed { .. } = event {
            if self.ui.route == Route::History && event.back_pressed() {
                self.dispatch(cx, Command::CloseHistory);
                return;
            }
        }
        let focus = cx.key_focus();
        if self.owns_keyboard_focus(cx, focus) && matches!(event, Event::KeyDown(_) | Event::TextInput(_) | Event::TextCopy(_)) {
            self.handle_keyboard(cx, event, focus);
            return;
        }
        self.view.handle_event(cx, event, scope);
        if let Event::MouseDown(mouse) = event {
            if self.view.area().rect(cx).contains(mouse.abs) { self.focus_calculator(cx); }
        }
        if let Event::Actions(actions) = event {
            self.handle_keys(cx, actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        if size.x > 1.0 && size.y > 1.0 {
            self.apply_layout(cx, size);
            self.fit_result(
                cx,
                self.last_layout
                    .unwrap_or_else(|| LayoutMetrics::for_size(size.x, size.y)),
            );
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_history_list(cx, &mut list);
            }
        }
        if cx.key_focus().is_empty() { self.focus_calculator(cx); }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::classify_layout;

    #[test]
    fn layout_decision_compact_and_wide() {
        assert_eq!(classify_layout(402.0, 780.0).width, WidthClass::Compact);
        assert_eq!(classify_layout(699.0, 800.0).width, WidthClass::Compact);
        assert_eq!(classify_layout(700.0, 800.0).width, WidthClass::Wide);
        assert_eq!(classify_layout(1240.0, 800.0).width, WidthClass::Wide);
        let short = classify_layout(874.0, 300.0);
        assert_eq!(short.width, WidthClass::Wide);
        assert!(short.short && short.scientific() && !short.history_tape());
        assert!(LayoutMetrics::for_size(402.0, 780.0).keys_meet_target());
        assert!(LayoutMetrics::for_size(1240.0, 800.0).keys_meet_target());
        assert!(LayoutMetrics::for_size(874.0, 300.0).keys_meet_target());
    }
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;
    use makepad_widgets::makepad_platform::storage::{StorageError, StorageOp};

    fn fixture() -> (Cx, CalculatorView) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut view = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            let view = CalculatorView::script_new_with_default(vm);
            assert!(vm.take_errors().is_empty());
            view
        });
        // Rejected locally by StorageHandle before backend dispatch: tests
        // control acknowledgements without touching any user's storage jail.
        view.set_storage(cx.storage(""));
        view.ensure_started(&mut cx);
        (cx, view)
    }

    fn response(id: StorageRequestId, op: StorageOp, result: Result<StorageResult, StorageError>) -> StorageResponse {
        StorageResponse { request_id: id, namespace: "".into(), op, result }
    }

    fn load(cx: &mut Cx, view: &mut CalculatorView, doc: &CalculatorDoc) {
        view.on_storage(cx, &[response(view.load_id.unwrap(), StorageOp::Get, Ok(StorageResult::Value(Some(doc.to_bytes()))))]);
    }

    fn draw(cx: &mut Cx, view: &mut CalculatorView, size: Vec2d) -> (DrawPass, DrawList2d, Overlay) {
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        let mut root = DrawList2d::new(cx);
        let overlay = Overlay { draw_list: DrawList::new(cx) };
        cx.redraw_all();
        let event = std::mem::take(&mut cx.new_draw_event);
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, Some(1.0));
        root.begin_always(&mut cx2d);
        overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::default());
        assert!(view.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fill()).is_ok());
        cx2d.end_pass_sized_turtle();
        overlay.end(&mut cx2d);
        root.end(&mut cx2d);
        cx2d.end_pass(&pass);
        (pass, root, overlay)
    }

    #[test]
    fn load_gates_every_mutation_and_paste_until_document_arrives() {
        let (mut cx, mut view) = fixture();
        let before = view.doc.clone();
        for command in [Command::Digit(9), Command::Equals, Command::MemoryAdd, Command::AngleToggle, Command::AllClear, Command::ClearHistory, Command::RecallHistory(1)] {
            view.dispatch(&mut cx, command);
        }
        view.paste_expression(&mut cx, "9+9");
        view.persist(&mut cx);
        assert_eq!(view.doc, before);
        assert_eq!(view.revision, 0);
        assert!(view.save_id.is_none());
        let mut saved = seed::initial();
        let mut ui = UiState::default();
        for command in [Command::Digit(7), Command::Equals, Command::MemoryAdd] { apply(&mut saved, &mut ui, command); }
        load(&mut cx, &mut view, &saved);
        assert_eq!(view.doc, saved);
        assert!(view.save_id.is_none());
        view.dispatch(&mut cx, Command::Digit(2));
        assert_eq!(view.doc.session.source, "2");
        assert_eq!(view.doc.memory, Some(7.0));
        assert_eq!(view.doc.history.len(), 1);
    }

    #[test]
    fn invalid_load_preserves_bytes_until_explicit_reset_and_ignores_late_retry() {
        let (mut cx, mut view) = fixture();
        let id = view.load_id.unwrap();
        view.on_storage(&mut cx, &[response(id, StorageOp::Get, Ok(StorageResult::Value(Some(b"broken".to_vec()))))]);
        view.dispatch(&mut cx, Command::Digit(8));
        view.paste_expression(&mut cx, "8");
        assert_eq!(view.doc, seed::initial());
        assert!(view.save_id.is_none());
        view.dispatch(&mut cx, Command::RetryStorage);
        let retry = view.load_id.unwrap();
        view.dispatch(&mut cx, Command::ResetStorage);
        assert!(view.loaded && !view.ui.load_failed);
        assert_eq!(view.save_revision, 1);
        let mut old = seed::initial();
        old.memory = Some(99.0);
        view.on_storage(&mut cx, &[response(retry, StorageOp::Get, Ok(StorageResult::Value(Some(old.to_bytes()))))]);
        assert_eq!(view.doc.memory, None);
    }

    #[test]
    fn missing_document_seeds_once_and_stale_load_responses_are_ignored() {
        let (mut cx, mut view) = fixture();
        let id = view.load_id.unwrap();
        view.on_storage(&mut cx, &[response(id, StorageOp::Get, Ok(StorageResult::Value(None)))]);
        assert!(view.loaded);
        assert_eq!(view.save_revision, 1);
        view.dispatch(&mut cx, Command::Digit(7));
        view.on_storage(&mut cx, &[response(id, StorageOp::Get, Ok(StorageResult::Value(None)))]);
        assert_eq!(view.doc.session.source, "7");
        assert_eq!(view.revision, 2);
    }

    #[test]
    fn failed_save_retry_saves_newest_revision_without_reloading() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        view.dispatch(&mut cx, Command::Digit(1));
        let first = view.save_id.unwrap();
        view.dispatch(&mut cx, Command::Digit(2));
        assert_eq!(view.save_revision, 1);
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Err(StorageError::Backend("offline".into())))]);
        assert!(view.ui.save_failed);
        assert!(view.widget(&mut cx, ids!(btn_retry)).visible());
        view.dispatch(&mut cx, Command::Digit(3));
        assert!(view.save_id.is_none());
        view.dispatch(&mut cx, Command::RetryStorage);
        let retry = view.save_id.unwrap();
        assert_ne!(retry, first);
        assert!(view.load_id.is_none());
        assert_eq!(view.save_revision, 3);
        assert_eq!(view.doc.session.source, "123");
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 0);
        assert_eq!(view.save_id, Some(retry));
        view.on_storage(&mut cx, &[response(retry, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 3);
        assert!(view.save_id.is_none() && !view.ui.save_failed);
    }

    #[test]
    fn close_coalesces_latest_revision_after_prior_acknowledgement() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        view.dispatch(&mut cx, Command::Digit(1));
        let first = view.save_id.unwrap();
        view.dispatch(&mut cx, Command::Digit(2));
        assert!(!view.handle_close(&mut cx));
        assert_eq!(view.save_id, Some(first));
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 1);
        assert_eq!(view.save_revision, 2);
        let latest = view.save_id.unwrap();
        assert_ne!(latest, first);
        view.on_storage(&mut cx, &[response(latest, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 2);
        assert!(view.handle_close(&mut cx));
    }

    #[test]
    fn loaded_error_rebuilds_cause_preview_and_clear_state() {
        let (mut cx, mut view) = fixture();
        let mut doc = seed::initial();
        let mut ui = UiState::default();
        for command in [Command::Digit(1), Command::Op(BinaryOp::Divide), Command::Digit(0), Command::Equals] { apply(&mut doc, &mut ui, command); }
        load(&mut cx, &mut view, &doc);
        assert_eq!(status_line(view.doc(), &view.ui), "Cannot divide by zero");
        assert_eq!(view.label(&mut cx, ids!(result)).text(), "Error");
        assert_eq!(view.button(&mut cx, ids!(k_clear)).text(), "AC");
        view.dispatch(&mut cx, Command::AllClear);
        assert_eq!(view.label(&mut cx, ids!(result)).text(), "0");
        let mut editing = seed::initial();
        editing.session.source = "2+3".into();
        editing.session.value = 123.0;
        rebuild_derived(&mut editing, &mut view.ui);
        assert_eq!(editing.session.value, 5.0);
        assert!(!view.ui.clear_is_ac);
    }

    #[test]
    fn error_colors_reset_on_both_displays_and_follow_theme_roles() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        // Non-default theme roles make hardcoded red/foreground fail this test.
        view.text_color = vec4(0.1, 0.2, 0.3, 1.0);
        view.error_color = vec4(0.8, 0.4, 0.2, 1.0);
        for command in [Command::Digit(1), Command::Op(BinaryOp::Divide), Command::Digit(0)] { view.dispatch(&mut cx, command); }
        for id in [ids!(result), ids!(short_result)] {
            assert_eq!(view.label(&mut cx, id).borrow().unwrap().draw_text.color, view.error_color);
        }
        view.dispatch(&mut cx, Command::AllClear);
        for id in [ids!(result), ids!(short_result)] {
            assert_eq!(view.label(&mut cx, id).borrow().unwrap().draw_text.color, view.text_color);
        }
    }

    #[test]
    fn actual_view_resize_keeps_all_keys_visible_and_preserves_document() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        for command in [Command::Digit(7), Command::Equals, Command::MemoryAdd, Command::AngleToggle, Command::OpenHistory] { view.dispatch(&mut cx, command); }
        let before = view.doc.clone();
        for size in [dvec2(402.0, 780.0), dvec2(1240.0, 800.0), dvec2(1100.0, 800.0), dvec2(700.0, 800.0), dvec2(874.0, 300.0)] {
            let _drawing = draw(&mut cx, &mut view, size);
            assert_eq!(view.doc, before);
            let wide = size.x >= 700.0;
            assert_eq!(view.last_layout.unwrap().show_scientific, wide);
            let viewport = view.widget(&mut cx, ids!(pads_scroll)).area().rect(&cx);
            for id in CalculatorView::all_keys().into_iter().skip(if wide { 0 } else { 25 }) {
                let rect = view.widget(&mut cx, id).area().rect(&cx);
                assert!(rect.size.x >= 43.99 && rect.size.y >= 43.99, "{size:?} {id:?}: {rect:?}");
                assert!(rect.pos.x >= 0.0 && rect.pos.x + rect.size.x <= size.x + 0.1, "{size:?} {id:?}: {rect:?}");
                assert!(rect.pos.y + rect.size.y <= size.y + 0.1, "{size:?} {id:?}: {rect:?}");
                assert!(rect.pos.y + rect.size.y <= viewport.pos.y + viewport.size.y + 0.1, "key clipped by {viewport:?}: {rect:?}");
            }
            if size.y == 300.0 {
                assert!((viewport.pos.y - 56.0).abs() < 0.1, "{viewport:?}");
                assert!((viewport.size.y - 236.0).abs() < 0.1, "{viewport:?}");
            }
        }
        assert_eq!(view.ui.route, Route::Calculator);
    }

    #[test]
    fn keyboard_routes_from_actual_root_and_key_areas_but_not_another_instance() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        let _drawing = draw(&mut cx, &mut view, dvec2(874.0, 300.0));
        let root = view.view.area();
        let key = view.button(&mut cx, ids!(k_power)).area();
        assert!(!root.is_empty() && !key.is_empty());
        assert!(view.owns_keyboard_focus(&mut cx, root));
        assert!(view.owns_keyboard_focus(&mut cx, key));
        assert!(!view.owns_keyboard_focus(&mut cx, Area::Empty));
        let event = Event::TextInput(TextInputEvent { input: "2^-3=".into(), ..Default::default() });
        view.handle_keyboard(&mut cx, &event, key);
        assert_eq!(view.doc.session.value, 0.125);
        let before = view.doc.clone();
        view.handle_keyboard(&mut cx, &event, Area::Empty);
        assert_eq!(view.doc, before);
        let paste = Event::TextInput(TextInputEvent { input: "2+3".into(), was_paste: true, ..Default::default() });
        view.handle_keyboard(&mut cx, &paste, root);
        assert_eq!(view.doc.session.value, 5.0);
    }

    #[test]
    fn button_click_then_keyboard_and_clipboard_use_the_same_calculator() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        let _drawing = draw(&mut cx, &mut view, dvec2(874.0, 300.0));
        let key = view.button(&mut cx, ids!(k_2));
        cx.widget_action(key.widget_uid(), ButtonAction::Clicked(KeyModifiers::default()));
        let actions = std::mem::take(&mut cx.new_actions);
        view.handle_keys(&mut cx, &actions);
        // Process the focus request through Cx's normal action-cycle boundary.
        cx.action(());
        cx.handle_actions();
        assert_eq!(cx.key_focus(), view.view.area());
        assert_eq!(view.doc.session.source, "2");
        let input = Event::TextInput(TextInputEvent { input: "+3=".into(), ..Default::default() });
        view.handle_event(&mut cx, &input, &mut Scope::empty());
        assert_eq!(view.doc.session.value, 5.0);
        let response = std::rc::Rc::new(std::cell::RefCell::new(None));
        let copy = Event::TextCopy(TextClipboardEvent { response: response.clone() });
        view.handle_event(&mut cx, &copy, &mut Scope::empty());
        assert_eq!(response.borrow().as_deref(), Some("5"));
    }

    #[test]
    fn re_review_action_batches_preserve_the_clicked_instances_actual_focus() {
        use std::{cell::RefCell, rc::Rc};

        let views = Rc::new(RefCell::new(Vec::<CalculatorView>::new()));
        let event_views = views.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            for view in event_views.borrow_mut().iter_mut() {
                view.handle_event(cx, event, &mut Scope::empty());
            }
        }));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            for _ in 0..2 {
                views.borrow_mut().push(CalculatorView::script_new_with_default(vm));
            }
            assert!(vm.take_errors().is_empty());
        });
        let mut drawings = Vec::new();
        for view in views.borrow_mut().iter_mut() {
            view.set_storage(cx.storage(""));
            view.ensure_started(&mut cx);
            load(&mut cx, view, &seed::initial());
            drawings.push(draw(&mut cx, view, dvec2(874.0, 300.0)));
        }

        // B processes A's click last, exactly as in the re-review. Exercise
        // both ownership directions through real action dispatch/focus commits.
        for index in [0, 1] {
            let key = views.borrow()[index].button(&mut cx, ids!(k_2));
            cx.widget_action(key.widget_uid(), ButtonAction::Clicked(KeyModifiers::default()));
            cx.handle_actions();
            let owner = views.borrow()[index].view.area();
            assert_eq!(cx.key_focus(), owner);
            // An unrelated batch must leave the selected calculator focused.
            cx.action(());
            cx.handle_actions();
            assert_eq!(cx.key_focus(), owner);
            let input = Event::TextInput(TextInputEvent { input: "3".into(), ..Default::default() });
            for view in views.borrow_mut().iter_mut() {
                view.handle_event(&mut cx, &input, &mut Scope::empty());
            }
            assert_eq!(views.borrow()[index].doc.session.source, "23");
            let other = 1 - index;
            assert_eq!(views.borrow()[other].doc.session.source, if index == 0 { "0" } else { "23" });
            let response = Rc::new(RefCell::new(None));
            let copy = Event::TextCopy(TextClipboardEvent { response: response.clone() });
            for view in views.borrow_mut().iter_mut() {
                view.handle_event(&mut cx, &copy, &mut Scope::empty());
            }
            assert_eq!(response.borrow().as_deref(), Some("23"));
        }
    }

    #[test]
    fn re_review_unicode_paste_preserves_display_and_clipboard() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        let _drawing = draw(&mut cx, &mut view, dvec2(874.0, 300.0));
        cx.action(());
        cx.handle_actions();
        assert_eq!(cx.key_focus(), view.view.area());
        for (source, expected) in [("−9", "-9"), ("−1234.00", "-1,234.00"), ("−.50", "-0.50"), ("−0.00", "-0.00"), ("−1.20e-3", "-1.20e-3"), ("−9+3", "-6")] {
            let input = Event::TextInput(TextInputEvent { input: source.into(), was_paste: true, ..Default::default() });
            view.handle_event(&mut cx, &input, &mut Scope::empty());
            assert_eq!(view.doc.session.source, source);
            assert_eq!(view.doc.session.value, crate::engine::evaluate(source, view.doc.angle).unwrap());
            for id in [ids!(result), ids!(short_result)] {
                assert_eq!(view.label(&mut cx, id).text(), expected, "{source}");
            }
            let response = std::rc::Rc::new(std::cell::RefCell::new(None));
            view.handle_event(&mut cx, &Event::TextCopy(TextClipboardEvent { response: response.clone() }), &mut Scope::empty());
            assert_eq!(response.borrow().as_deref(), Some(expected), "{source}");
            assert_eq!(apply(&mut view.doc, &mut view.ui, Command::Copy).copied.as_deref(), Some(expected));
        }
    }

    fn two_views() -> (Cx, CalculatorView, CalculatorView) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let (mut a, mut b) = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::script_mod(vm);
            let a = CalculatorView::script_new_with_default(vm);
            let b = CalculatorView::script_new_with_default(vm);
            assert!(vm.take_errors().is_empty());
            (a, b)
        });
        a.set_storage(cx.storage(""));
        b.set_storage(cx.storage(""));
        a.ensure_started(&mut cx);
        b.ensure_started(&mut cx);
        (cx, a, b)
    }

    #[test]
    fn hosted_instances_keep_document_revision_angle_and_tools_independent() {
        use makepad_ai_services::wire::{ServiceCall, ToolOutcome};
        let (mut cx, mut a, mut b) = two_views();
        load(&mut cx, &mut a, &seed::initial());
        load(&mut cx, &mut b, &seed::initial());
        for command in [Command::Digit(9), Command::Equals, Command::MemoryAdd, Command::AngleToggle] {
            a.dispatch(&mut cx, command);
        }
        let _drawing = draw(&mut cx, &mut a, dvec2(874.0, 300.0));
        let focus = a.view.area();
        let _other_drawing = draw(&mut cx, &mut b, dvec2(402.0, 780.0));
        assert!(!b.owns_keyboard_focus(&mut cx, focus));
        let input = Event::TextInput(TextInputEvent { input: "8".into(), ..Default::default() });
        b.handle_keyboard(&mut cx, &input, focus);
        assert_eq!(b.doc, seed::initial());
        assert_eq!(b.revision, 0);
        assert_eq!(a.doc.angle, AngleMode::Radians);
        assert_eq!(a.doc.memory, Some(9.0));
        let before = a.doc.clone();
        let revision = a.revision;
        let call = ServiceCall {
            call_id: "test".into(),
            tool: "eval".into(),
            args: r#"{"expression":"sin(pi/2)"}"#.into(),
        };
        let result = crate::ai::answer(a.doc(), &call);
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert!(result.data.contains("\"angle\":\"rad\""));
        assert!(result.data.contains("\"value\":1"));
        assert_eq!(a.doc, before);
        assert_eq!(a.revision, revision);
    }

    #[test]
    fn hosted_shutdown_flushes_coalesced_revision() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        view.dispatch(&mut cx, Command::Digit(1));
        let first = view.save_id.unwrap();
        view.dispatch(&mut cx, Command::Digit(2));
        assert_eq!(view.save_id, Some(first));
        assert_eq!(view.save_revision, 1);
        assert_eq!(view.revision, 2);
        view.shutdown(&mut cx);
        // This proves the flush only while the root still receives events.
        // The host must retain it until completion; immediate teardown cannot.
        assert_eq!(view.save_id, Some(first));
        assert_eq!(view.save_revision, 1);
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Ok(StorageResult::Unit))]);
        let latest = view.save_id.expect("the acknowledgement must SET the coalesced revision");
        assert_ne!(latest, first);
        assert_eq!(view.save_revision, 2);
        assert_eq!(view.save_id, Some(latest));
        assert_eq!(view.acked_revision, 1);
        view.on_storage(&mut cx, &[response(latest, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 2);
        assert!(view.save_id.is_none());
        // An old/duplicate acknowledgement cannot regress the saved revision.
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 2);
        assert!(view.save_id.is_none());
    }

    #[test]
    fn re_review_shutdown_never_submits_a_newer_write_before_the_pending_one_finishes() {
        let (mut cx, mut view) = fixture();
        load(&mut cx, &mut view, &seed::initial());
        view.dispatch(&mut cx, Command::Digit(1));
        let first = view.save_id.unwrap();
        view.dispatch(&mut cx, Command::Digit(2));
        for _ in 0..2 {
            view.shutdown(&mut cx);
            assert_eq!(view.save_id, Some(first), "concurrent SETs can complete newest-first and restore stale bytes");
            assert_eq!(view.save_revision, 1);
            assert_eq!(view.acked_revision, 0);
            assert_eq!(view.doc.session.source, "12");
            assert_eq!(view.revision, 2);
        }
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Err(StorageError::Backend("offline".into())))]);
        assert!(view.ui.save_failed);
        view.shutdown(&mut cx);
        let retry = view.save_id.unwrap();
        assert_ne!(retry, first);
        assert_eq!(view.save_revision, 2);
        view.on_storage(&mut cx, &[response(first, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.save_id, Some(retry));
        assert_eq!(view.acked_revision, 0);
        view.on_storage(&mut cx, &[response(retry, StorageOp::Set, Ok(StorageResult::Unit))]);
        assert_eq!(view.acked_revision, 2);
        assert!(view.save_id.is_none());
    }

}
