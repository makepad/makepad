//! Scroll containment harness: a page and a list with scrollables nested inside them, plus popups,
//! a stack view, a sliding panel and a modal. `status` reports every offset, for `--remote` tests.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const ROW_HEIGHT: f64 = 30.0;
const OUTER_ROWS: usize = 40;
const NEST_ROW: usize = 3;
const SHORT_ROW: usize = 6;
const STRIP_ROW: usize = 9;
/// A code block, a row, then another strip, far enough down to start out of view.
const CODE_ROW: usize = 20;
const FAR_STRIP_ROW: usize = 22;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.RowListBase = #(RowList::register_widget(vm))
    mod.widgets.RowList = set_type_default() do mod.widgets.RowListBase{
        width: Fill
        height: 160
        flow: Down
        list := PortalList{
            width: Fill
            height: Fill
            Row := View{
                width: Fill
                height: 30
                title := Label{text: ""}
            }
        }
    }

    mod.widgets.NestedListsBase = #(NestedLists::register_widget(vm))
    mod.widgets.NestedLists = set_type_default() do mod.widgets.NestedListsBase{
        width: 300
        height: Fill
        flow: Down
        list := PortalList{
            width: Fill
            height: Fill
            Row := View{
                width: Fill
                height: 30
                title := Label{text: ""}
            }
            Nest := View{
                width: Fill
                height: 150
                nested := mod.widgets.RowList{height: 150 row_count: 50 row_prefix: "Nested"}
            }
            Short := View{
                width: Fill
                height: 150
                nested_short := mod.widgets.RowList{height: 150 row_count: 2 row_prefix: "NestShort"}
            }
            Strip := View{
                width: Fill
                height: 40
                strip := ScrollXView{
                    width: Fill
                    height: 40
                    flow: Right
                    strip_start := Label{width: 120 text: "StripStart"}
                    Label{width: 120 text: "Strip 1"}
                    Label{width: 120 text: "Strip 2"}
                    Label{width: 120 text: "Strip 3"}
                    Label{width: 120 text: "Strip 4"}
                    Label{width: 120 text: "Strip 5"}
                    Label{width: 120 text: "Strip 6"}
                    Label{width: 120 text: "Strip 7"}
                }
            }
            FarStrip := View{
                width: Fill
                height: 40
                far_strip := ScrollXView{
                    width: Fill
                    height: 40
                    flow: Right
                    Label{width: 120 text: "FarStrip 0"}
                    Label{width: 120 text: "FarStrip 1"}
                    Label{width: 120 text: "FarStrip 2"}
                    Label{width: 120 text: "FarStrip 3"}
                    Label{width: 120 text: "FarStrip 4"}
                }
            }
            Code := View{
                width: Fill
                height: 120
                code := ScrollYView{
                    width: Fill
                    height: 120
                    flow: Down
                    code_top := Label{height: 30 text: "CodeTop"}
                    Label{height: 30 text: "Code 1"}
                    Label{height: 30 text: "Code 2"}
                    Label{height: 30 text: "Code 3"}
                    Label{height: 30 text: "Code 4"}
                    Label{height: 30 text: "Code 5"}
                    Label{height: 30 text: "Code 6"}
                    Label{height: 30 text: "Code 7"}
                    Label{height: 30 text: "Code 8"}
                    Label{height: 30 text: "Code 9"}
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1360, 760)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            status := Label{width: Fill text: "status"}
                            View{
                                width: Fill
                                height: Fill
                                flow: Right
                                spacing: 20
                                page := ScrollYView{
                                    width: 380
                                    height: Fill
                                    flow: Down
                                    spacing: 10
                                    page_top := Label{height: 30 text: "PageTop"}
                                    log_list := mod.widgets.RowList{row_count: 100 row_prefix: "Log"}
                                    short_list := mod.widgets.RowList{row_count: 2 row_prefix: "ShortLog"}
                                    inner_view := ScrollYView{
                                        width: Fill
                                        height: 160
                                        flow: Down
                                        inner_top := Label{height: 30 text: "InnerTop"}
                                        Label{height: 30 text: "Inner 1"}
                                        Label{height: 30 text: "Inner 2"}
                                        Label{height: 30 text: "Inner 3"}
                                        Label{height: 30 text: "Inner 4"}
                                        Label{height: 30 text: "Inner 5"}
                                        Label{height: 30 text: "Inner 6"}
                                        Label{height: 30 text: "Inner 7"}
                                        Label{height: 30 text: "Inner 8"}
                                        Label{height: 30 text: "Inner 9"}
                                    }
                                    notes := TextInput{width: Fill height: 100 is_multiline: true}
                                    short_notes := TextInput{width: Fill height: 100 is_multiline: true}
                                    carousel := ScrollXView{
                                        width: Fill
                                        height: 40
                                        flow: Right
                                        carousel_start := Label{width: 120 text: "CarouselStart"}
                                        Label{width: 120 text: "Carousel 1"}
                                        Label{width: 120 text: "Carousel 2"}
                                        Label{width: 120 text: "Carousel 3"}
                                        Label{width: 120 text: "Carousel 4"}
                                        Label{width: 120 text: "Carousel 5"}
                                        Label{width: 120 text: "Carousel 6"}
                                    }
                                    Label{height: 40 text: "Page 1"}
                                    Label{height: 40 text: "Page 2"}
                                    Label{height: 40 text: "Page 3"}
                                    Label{height: 40 text: "Page 4"}
                                    Label{height: 40 text: "Page 5"}
                                    Label{height: 40 text: "Page 6"}
                                    Label{height: 40 text: "Page 7"}
                                    Label{height: 40 text: "Page 8"}
                                    Label{height: 40 text: "Page 9"}
                                    Label{height: 40 text: "Page 10"}
                                    Label{height: 40 text: "PageEnd"}
                                }
                                lists := mod.widgets.NestedLists{}
                                View{
                                    width: 200
                                    height: Fill
                                    flow: Down
                                    spacing: 10
                                    fit_list := mod.widgets.RowList{height: 120 row_count: 3 row_prefix: "Fit"}
                                    open_modal := Button{text: "OpenModal"}
                                    dd := DropDown2{labels: ["D0" "D1" "D2" "D3" "D4" "D5" "D6" "D7" "D8" "D9" "D10" "D11" "D12" "D13" "D14" "D15" "D16" "D17" "D18" "D19" "D20" "D21" "D22" "D23" "D24" "D25" "D26" "D27" "D28" "D29"]}
                                    under_popup := ScrollYView{
                                        width: Fill
                                        height: 300
                                        flow: Down
                                        under_top := Label{height: 30 text: "UnderTop"}
                                        Label{height: 30 text: "Under 1"}
                                        Label{height: 30 text: "Under 2"}
                                        Label{height: 30 text: "Under 3"}
                                        Label{height: 30 text: "Under 4"}
                                        Label{height: 30 text: "Under 5"}
                                        Label{height: 30 text: "Under 6"}
                                        Label{height: 30 text: "Under 7"}
                                        Label{height: 30 text: "Under 8"}
                                        Label{height: 30 text: "Under 9"}
                                        Label{height: 30 text: "Under 10"}
                                        Label{height: 30 text: "Under 11"}
                                        Label{height: 30 text: "Under 12"}
                                    }
                                }
                                stack_box := View{
                                    width: 200
                                    height: Fill
                                    flow: Down
                                    stack := StackNavigation{
                                        width: Fill
                                        height: Fill
                                        root_view +: {
                                            flow: Down
                                            Label{text: "StackRoot"}
                                        }
                                        detail_view := StackNavigationView{
                                            body +: {
                                                flow: Down
                                                swipe_strip := ScrollXView{
                                                    width: Fill
                                                    height: 60
                                                    flow: Right
                                                    swipe_strip_start := Label{width: 150 text: "SwipeStart"}
                                                    Label{width: 150 text: "Swipe 1"}
                                                    Label{width: 150 text: "Swipe 2"}
                                                    Label{width: 150 text: "Swipe 3"}
                                                    Label{width: 150 text: "Swipe 4"}
                                                    Label{width: 150 text: "Swipe 5"}
                                                }
                                                Label{text: "BelowSwipeStrip"}
                                            }
                                        }
                                    }
                                }
                                panel_box := ExpandablePanel{
                                    width: 180
                                    height: Fill
                                    initial_offset: 100.0
                                    SolidView{width: Fill height: Fill draw_bg.color: #224}
                                    panel := RoundedView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        panel_list := mod.widgets.RowList{height: Fill row_count: 60 row_prefix: "Panel"}
                                    }
                                }
                            }
                        }
                        modal := Modal{
                            content +: {
                                width: 300
                                height: 400
                                flow: Down
                                modal_page := ScrollYView{
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    modal_top := Label{height: 30 text: "ModalTop"}
                                    Label{height: 30 text: "Modal 1"}
                                    Label{height: 30 text: "Modal 2"}
                                    Label{height: 30 text: "Modal 3"}
                                    Label{height: 30 text: "Modal 4"}
                                    modal_list := mod.widgets.RowList{height: 200 row_count: 50 row_prefix: "ModalList"}
                                    Label{height: 30 text: "Modal 5"}
                                    Label{height: 30 text: "Modal 6"}
                                    Label{height: 30 text: "Modal 7"}
                                    Label{height: 30 text: "Modal 8"}
                                    Label{height: 30 text: "Modal 9"}
                                    Label{height: 30 text: "Modal 10"}
                                    Label{height: 30 text: "Modal 11"}
                                    Label{height: 30 text: "Modal 12"}
                                    Label{height: 30 text: "Modal 13"}
                                    Label{height: 30 text: "Modal 14"}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A fixed-height list of `row_count` rows titled "`row_prefix` N".
#[derive(Script, ScriptHook, Widget)]
pub struct RowList {
    #[deref]
    view: View,
    #[live]
    row_count: f64,
    #[live]
    row_prefix: String,
}

impl RowList {
    /// How far the list is scrolled from its top, in pixels (negative while stretched past it).
    fn position(&self, cx: &mut Cx) -> f64 {
        let list = self.view.portal_list(cx, ids!(list));
        list.first_id() as f64 * ROW_HEIGHT - list.scroll_position()
    }
}

impl Widget for RowList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let count = self.row_count as usize;
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, count);
                while let Some(index) = list.next_visible_item(cx) {
                    if index >= count {
                        continue;
                    }
                    let item = list.item(cx, index, id!(Row));
                    item.label(cx, ids!(title))
                        .set_text(cx, &format!("{} {index}", self.row_prefix));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// A list whose rows hold another list, a list that fits, horizontal strips and a code block.
#[derive(Script, ScriptHook, Widget)]
pub struct NestedLists {
    #[deref]
    view: View,
    #[rust]
    nested: [WidgetRef; 4],
}

/// The height of a row of [`NestedLists`].
fn outer_row_height(index: usize) -> f64 {
    match index {
        NEST_ROW | SHORT_ROW => 150.0,
        STRIP_ROW | FAR_STRIP_ROW => 40.0,
        CODE_ROW => 120.0,
        _ => ROW_HEIGHT,
    }
}

impl Widget for NestedLists {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, OUTER_ROWS);
                while let Some(index) = list.next_visible_item(cx) {
                    if index >= OUTER_ROWS {
                        continue;
                    }
                    let template = match index {
                        NEST_ROW => id!(Nest),
                        SHORT_ROW => id!(Short),
                        STRIP_ROW => id!(Strip),
                        CODE_ROW => id!(Code),
                        FAR_STRIP_ROW => id!(FarStrip),
                        _ => id!(Row),
                    };
                    let item = list.item(cx, index, template);
                    match index {
                        NEST_ROW => self.nested[0] = item.clone(),
                        SHORT_ROW => self.nested[1] = item.clone(),
                        STRIP_ROW => self.nested[2] = item.clone(),
                        CODE_ROW => self.nested[3] = item.clone(),
                        FAR_STRIP_ROW => {}
                        _ => item.label(cx, ids!(title)).set_text(cx, &format!("Outer {index}")),
                    }
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// `child`'s offset from the top-left of `parent`, which shrinks as `parent` scrolls.
fn offset(cx: &mut Cx, parent: &WidgetRef, child: &WidgetRef) -> Vec2d {
    child.area().rect(cx).pos - parent.area().rect(cx).pos
}

fn row_list_position(cx: &mut Cx, widget: &WidgetRef) -> f64 {
    widget.borrow::<RowList>().map_or(-1.0, |list| list.position(cx))
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    /// Every scroll offset, for tests to read through `/snap`.
    fn status(&self, cx: &mut Cx) -> String {
        let ui = &self.ui;
        let page = ui.widget(cx, ids!(page));
        let page_y = -offset(cx, &page, &ui.widget(cx, ids!(page_top))).y;
        let log = row_list_position(cx, &ui.widget(cx, ids!(log_list)));
        let short = row_list_position(cx, &ui.widget(cx, ids!(short_list)));
        let inner = ui.widget(cx, ids!(inner_view));
        let inner_y = -offset(cx, &inner, &ui.widget(cx, ids!(inner_top))).y;
        let carousel = ui.widget(cx, ids!(carousel));
        let carousel_x = -offset(cx, &carousel, &ui.widget(cx, ids!(carousel_start))).x;
        let lists = ui.widget(cx, ids!(lists));
        let (outer, nested, nested_short, strip_x, code_y) = match lists.borrow::<NestedLists>() {
            Some(lists) => {
                let outer = lists.view.portal_list(cx, ids!(list));
                let rows_above: f64 = (0..outer.first_id()).map(outer_row_height).sum();
                let outer = rows_above - outer.scroll_position();
                let nested = row_list_position(cx, &lists.nested[0].widget(cx, ids!(nested)));
                let nested_short =
                    row_list_position(cx, &lists.nested[1].widget(cx, ids!(nested_short)));
                let strip = lists.nested[2].widget(cx, ids!(strip));
                let strip_x = -offset(cx, &strip, &strip.widget(cx, ids!(strip_start))).x;
                let code = lists.nested[3].widget(cx, ids!(code));
                let code_y = -offset(cx, &code, &code.widget(cx, ids!(code_top))).y;
                (outer, nested, nested_short, strip_x, code_y)
            }
            None => (-1.0, -1.0, -1.0, -1.0, -1.0),
        };
        let fit_list = ui.widget(cx, ids!(fit_list));
        let fit = row_list_position(cx, &fit_list);
        let fit_travel = ui.portal_list(cx, ids!(fit_list.list)).user_scroll_travel();
        let under = ui.widget(cx, ids!(under_popup));
        let under_y = -offset(cx, &under, &ui.widget(cx, ids!(under_top))).y;
        let swipe_strip = ui.widget(cx, ids!(swipe_strip));
        let swipe_x = -offset(cx, &swipe_strip, &ui.widget(cx, ids!(swipe_strip_start))).x;
        let stack_depth = ui.stack_navigation(cx, ids!(stack)).depth();
        let panel = ui.expandable_panel(cx, ids!(panel_box)).get_current_offset();
        let panel_list = row_list_position(cx, &ui.widget(cx, ids!(panel_list)));
        let modal_page = ui.widget(cx, ids!(modal_page));
        let modal_y = -offset(cx, &modal_page, &ui.widget(cx, ids!(modal_top))).y;
        let modal_list = row_list_position(cx, &ui.widget(cx, ids!(modal_list)));
        format!(
            "page={page_y:.0} log={log:.0} short={short:.0} inner={inner_y:.0} carousel={carousel_x:.0} \
             outer={outer:.0} nested={nested:.0} nested_short={nested_short:.0} strip={strip_x:.0} \
             code={code_y:.0} fit={fit:.1} fit_travel={fit_travel:.0} under={under_y:.0} \
             swipe={swipe_x:.0} stack_depth={stack_depth} panel={panel:.0} panel_list={panel_list:.0} \
             modal={modal_y:.0} modal_list={modal_list:.0}"
        )
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let notes: Vec<String> = (0..40).map(|i| format!("Note line {i}")).collect();
        self.ui.text_input(cx, ids!(notes)).set_text(cx, &notes.join("\n"));
        self.ui.text_input(cx, ids!(short_notes)).set_text(cx, "Short note");
        self.ui.stack_navigation(cx, ids!(stack)).push_without_animation(cx, live_id!(detail_view));
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(open_modal)).clicked(actions) {
            self.ui.modal(cx, ids!(modal)).open(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // Also after a draw, which clamps a list scrolled past its end.
        let status = self.status(cx);
        let label = self.ui.label(cx, ids!(status));
        if label.text() != status {
            label.set_text(cx, &status);
        }
    }
}
