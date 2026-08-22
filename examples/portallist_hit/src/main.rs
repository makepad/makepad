//! PortalList hit-area regression harness.
//!
//! A fixed-height `PortalList` of named rows, with a `Button` parked directly
//! underneath it. `PortalList` deliberately draws recycled rows past both
//! edges of its viewport and relies on the clip rect to hide them, so a
//! scrolled list has rows *drawn* on top of `outside_button` that a viewer
//! never sees.
//!
//! Both the row buttons and `outside_button` report what was pressed into
//! `last_played`, so any consumer that locates a row by its reported rect and
//! clicks the middle of it — the `--remote` `/snap` + `/click` loop, a
//! `makepad_test` locator — can be checked against what the app actually did.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

pub const ROW_COUNT: usize = 200;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.HitListBase = #(HitList::register_widget(vm))
    mod.widgets.HitList = set_type_default() do mod.widgets.HitListBase{
        width: Fill
        height: Fill
        flow: Down

        list := PortalList{
            width: Fill
            height: Fill

            Row := View{
                width: Fill
                height: 40
                flow: Right
                spacing: 8
                padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                align: Align{y: 0.5}

                title := Label{
                    width: Fill
                    text: ""
                }
                play_button := Button{
                    text: "Play"
                }
            }
        }
    }

    // A scrolling view whose OWN rect is partially clipped away by an
    // ancestor: a 200px ScrollYView inside a 120px clipping view, so its
    // unclipped rect covers an 80px band below the crop where other widgets
    // really live. The scroll-gate repro: `ScrollBar::handle_scroll_event`
    // contains() a wheel against the scroll area's rect, and a wheel in that
    // band must not scroll this view. (PortalList wheels ride the hit
    // system, which already tests the clipped rect — this is the ScrollBar
    // path.)
    mod.widgets.CroppedScroll = View{
        width: Fill
        height: 120
        clip_x: true
        clip_y: true
        cropped_scroll := ScrollYView{
            width: Fill
            height: 200
            flow: Down
            Label{height: 30, text: "CropRow 0"}
            Label{height: 30, text: "CropRow 1"}
            Label{height: 30, text: "CropRow 2"}
            Label{height: 30, text: "CropRow 3"}
            Label{height: 30, text: "CropRow 4"}
            Label{height: 30, text: "CropRow 5"}
            Label{height: 30, text: "CropRow 6"}
            Label{height: 30, text: "CropRow 7"}
            Label{height: 30, text: "CropRow 8"}
            Label{height: 30, text: "CropRow 9"}
            Label{height: 30, text: "CropRow 10"}
            Label{height: 30, text: "CropRow 11"}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(420, 560)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        last_played := Label{
                            text: "none"
                        }
                        hit_list := mod.widgets.HitList{
                            height: 200
                        }
                        outside_button := Button{
                            width: Fill
                            text: "Outside"
                        }
                        crop := mod.widgets.CroppedScroll{}
                        under_crop_button := Button{
                            width: Fill
                            text: "UnderCrop"
                        }
                        View{
                            width: Fill
                            height: Fill
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct HitList {
    #[deref]
    view: View,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HitListAction {
    Play(usize),
    None,
}

impl HitList {
    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) -> HitListAction {
        let list = self.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(play_button)).clicked(actions) {
                return HitListAction::Play(index);
            }
        }
        HitListAction::None
    }
}

impl Widget for HitList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, ROW_COUNT);
                while let Some(index) = list.next_visible_item(cx) {
                    if index >= ROW_COUNT {
                        continue;
                    }
                    let item = list.item(cx, index, id!(Row));
                    item.label(cx, ids!(title))
                        .set_text(cx, &format!("Row {index}"));
                    item.button(cx, ids!(play_button))
                        .set_text(cx, &format!("Play {index}"));
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

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let hit_list = self.ui.widget(cx, ids!(hit_list));
        let action = hit_list
            .borrow_mut::<HitList>()
            .map(|mut list| list.handle_actions(cx, actions))
            .unwrap_or(HitListAction::None);
        if let HitListAction::Play(index) = action {
            log!("PLAYED Row {index}");
            self.ui
                .label(cx, ids!(last_played))
                .set_text(cx, &format!("Row {index}"));
        }
        if self.ui.button(cx, ids!(outside_button)).clicked(actions) {
            log!("PLAYED outside");
            self.ui.label(cx, ids!(last_played)).set_text(cx, "outside");
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
    }
}
