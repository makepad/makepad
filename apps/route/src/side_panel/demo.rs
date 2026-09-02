use makepad_map_nav::search::SearchResult;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let PanelLabel = Label{
        width: Fill
        draw_text +: {
            color: #x28343f
            text_style: theme.font_regular{font_size: 10}
        }
    }

    let ResultButton = Button{
        width: Fill
        draw_text +: {text_style: theme.font_regular{font_size: 10}}
    }

    mod.widgets.RouteSidePanel = View{
        width: 390
        height: Fill
        flow: Down
        padding: Inset{left: 16, right: 16, top: 18, bottom: 18}
        spacing: 8
        draw_bg +: {color: #xf7f9fc}

        Label{
            text: "Plan a trip"
            draw_text +: {
                color: #x17212b
                text_style: theme.font_bold{font_size: 16}
            }
        }
        PanelLabel{text: "Search hosted places, then route from your current map position."}
        search_input := TextInput{
            width: Fill
            empty_text: "Search a place…"
            draw_text +: {
                color: #x17212b
                color_empty: #x78838d
            }
        }
        search_button := Button{width: Fill, text: "Search"}
        search_status := PanelLabel{text: "Ready"}
        results := ScrollYView{
            width: Fill
            height: Fill
            flow: Down
            spacing: 5
            result_0_wrap := View{visible: false, width: Fill, height: Fit, result_0 := ResultButton{}}
            result_1_wrap := View{visible: false, width: Fill, height: Fit, result_1 := ResultButton{}}
            result_2_wrap := View{visible: false, width: Fill, height: Fit, result_2 := ResultButton{}}
            result_3_wrap := View{visible: false, width: Fill, height: Fit, result_3 := ResultButton{}}
            result_4_wrap := View{visible: false, width: Fill, height: Fit, result_4 := ResultButton{}}
            result_5_wrap := View{visible: false, width: Fill, height: Fit, result_5 := ResultButton{}}
            result_6_wrap := View{visible: false, width: Fill, height: Fit, result_6 := ResultButton{}}
            result_7_wrap := View{visible: false, width: Fill, height: Fit, result_7 := ResultButton{}}
        }
        route_here_wrap := View{
            visible: false
            width: Fill
            height: Fit
            route_here := Button{width: Fill, text: "Route here"}
        }
        PanelLabel{text: "Along this route"}
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            along_chargers := Button{text: "Chargers"}
            along_museums := Button{text: "Museums"}
        }
        along_status := PanelLabel{text: "Plan a route to search along it."}
        weather_wrap := View{
            width: Fill
            height: Fit
            weather_now := PanelLabel{text: "Weather now: not requested"}
        }
        Hr{height: 10}
        rain_toggle := CheckBox{text: "Rain radar"}
        wind_toggle := CheckBox{text: "Wind"}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlongKind {
    Chargers,
    Museums,
}

#[derive(Debug, PartialEq)]
pub enum PanelAction {
    Search(String),
    SelectResult(usize),
    RouteHere(usize),
    Along(AlongKind),
    Rain(bool),
    Wind(bool),
}

#[derive(Default)]
pub struct PanelController {
    results: Vec<SearchResult>,
    selected: Option<usize>,
}

impl PanelController {
    pub fn result(&self, index: usize) -> Option<&SearchResult> {
        self.results.get(index)
    }

    pub fn set_results(&mut self, cx: &mut Cx, ui: &WidgetRef, results: Vec<SearchResult>) {
        self.results = results;
        self.selected = None;
        for index in 0..8 {
            let button = result_button(ui, cx, index);
            let wrapper = result_wrapper(ui, cx, index);
            if let Some(result) = self.results.get(index) {
                wrapper.set_visible(cx, true);
                button.set_text(
                    cx,
                    &format!("{}  ·  {}", result.name, result.category.label()),
                );
            } else {
                wrapper.set_visible(cx, false);
            }
        }
        ui.view(cx, ids!(route_here_wrap)).set_visible(cx, false);
        ui.label(cx, ids!(search_status)).set_text(
            cx,
            if self.results.is_empty() {
                "No places found"
            } else {
                "Choose a result"
            },
        );
    }

    pub fn set_search_status(&self, cx: &mut Cx, ui: &WidgetRef, text: &str) {
        ui.label(cx, ids!(search_status)).set_text(cx, text);
    }

    pub fn set_along_status(&self, cx: &mut Cx, ui: &WidgetRef, text: &str) {
        ui.label(cx, ids!(along_status)).set_text(cx, text);
    }

    pub fn set_weather(&self, cx: &mut Cx, ui: &WidgetRef, text: &str) {
        ui.view(cx, ids!(weather_wrap)).set_visible(cx, true);
        ui.label(cx, ids!(weather_now)).set_text(cx, text);
    }

    pub fn hide_weather(&self, cx: &mut Cx, ui: &WidgetRef) {
        ui.view(cx, ids!(weather_wrap)).set_visible(cx, false);
    }

    pub fn actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) -> Vec<PanelAction> {
        let mut out = Vec::new();
        if ui.button(cx, ids!(search_button)).clicked(actions) {
            let query = ui.text_input(cx, ids!(search_input)).text();
            if !query.trim().is_empty() {
                out.push(PanelAction::Search(query.trim().to_string()));
            }
        }
        if let Some((query, _)) = ui.text_input(cx, ids!(search_input)).returned(actions) {
            if !query.trim().is_empty() {
                out.push(PanelAction::Search(query.trim().to_string()));
            }
        }
        for index in 0..self.results.len().min(8) {
            if result_button(ui, cx, index).clicked(actions) {
                self.selected = Some(index);
                ui.view(cx, ids!(route_here_wrap)).set_visible(cx, true);
                self.set_search_status(cx, ui, &format!("Selected {}", self.results[index].name));
                out.push(PanelAction::SelectResult(index));
            }
        }
        if ui.button(cx, ids!(route_here)).clicked(actions) {
            if let Some(index) = self.selected {
                out.push(PanelAction::RouteHere(index));
            }
        }
        if ui.button(cx, ids!(along_chargers)).clicked(actions) {
            out.push(PanelAction::Along(AlongKind::Chargers));
        }
        if ui.button(cx, ids!(along_museums)).clicked(actions) {
            out.push(PanelAction::Along(AlongKind::Museums));
        }
        if let Some(on) = ui.check_box(cx, ids!(rain_toggle)).changed(actions) {
            out.push(PanelAction::Rain(on));
        }
        if let Some(on) = ui.check_box(cx, ids!(wind_toggle)).changed(actions) {
            out.push(PanelAction::Wind(on));
        }
        out
    }
}

fn result_button(ui: &WidgetRef, cx: &mut Cx, index: usize) -> ButtonRef {
    match index {
        0 => ui.button(cx, ids!(result_0)),
        1 => ui.button(cx, ids!(result_1)),
        2 => ui.button(cx, ids!(result_2)),
        3 => ui.button(cx, ids!(result_3)),
        4 => ui.button(cx, ids!(result_4)),
        5 => ui.button(cx, ids!(result_5)),
        6 => ui.button(cx, ids!(result_6)),
        _ => ui.button(cx, ids!(result_7)),
    }
}

fn result_wrapper(ui: &WidgetRef, cx: &mut Cx, index: usize) -> ViewRef {
    match index {
        0 => ui.view(cx, ids!(result_0_wrap)),
        1 => ui.view(cx, ids!(result_1_wrap)),
        2 => ui.view(cx, ids!(result_2_wrap)),
        3 => ui.view(cx, ids!(result_3_wrap)),
        4 => ui.view(cx, ids!(result_4_wrap)),
        5 => ui.view(cx, ids!(result_5_wrap)),
        6 => ui.view(cx, ids!(result_6_wrap)),
        _ => ui.view(cx, ids!(result_7_wrap)),
    }
}
