//! weather — current conditions and a five-day forecast from Open-Meteo for
//! a chosen city. One process, one model, two resident faces on the same
//! window: the full forecast and a compact home-widget tile. The host picks
//! the face with `HostedViewMode`; the fetch, its timers and the last good
//! data live in the shared `App` regardless of which face is showing.

pub use makepad_widgets;
use makepad_widgets::*;

mod model;
mod sky;
use model::{describe, sky_kind, temp, WeatherState, ATTRIBUTION, CITIES, MAX_BODY_BYTES};
use sky::SkyView;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Card = PanelView {
        width: Fill height: Fit
        flow: Down spacing: 4 padding: 12
        draw_bg +: { color: theme.color_bg_container }
    }
    let Meta = Label {
        padding: 0
        draw_text +: { color: theme.color_text_disabled text_style: theme.font_regular{font_size: 10.0} }
    }
    let Body = Label {
        padding: 0
        draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 12.0} }
    }
    let Value = Label {
        padding: 0
        draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 20.0} }
    }
    let OnSky = Label {
        padding: 0
        draw_text +: { color: #ffffff text_style: theme.font_regular{font_size: 12.0} }
    }
    let DayCard = PanelView {
        width: 108 height: Fit
        flow: Down spacing: 3 padding: 10
        draw_bg +: { color: theme.color_bg_container }
        name := Body{text: "–"}
        cond := Meta{text: ""}
        hilo := Body{text: ""}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Weather"
                window.inner_size: vec2(420, 800)
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    padding: 0 margin: 0 spacing: 0
                    app_view := HostedView{
                        full: ScrollYView{
                            width: Fill height: Fill
                            flow: Down spacing: 12 padding: 16
                            show_bg: true
                            draw_bg +: { color: theme.color_bg_app }

                            header := View{
                                width: Fill height: Fit flow: Right{wrap: true} spacing: 10 align: Align{y: 0.5}
                                AppIcon{name: "weather" width: 32 height: 32}
                                Label{padding: 0 text: "Weather" draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 19.0}}}
                                city_select := DropDown{
                                    width: 150 height: 44
                                    labels: ["Amsterdam" "London" "New York" "Tokyo"]
                                    selected_item: 0
                                }
                                refresh := Button{
                                    height: 44 width: Fit
                                    padding: Inset{left: 18 right: 18 top: 0 bottom: 0}
                                    text: "Refresh"
                                }
                            }
                            city_note := Meta{text: "Selected city: Amsterdam, Netherlands (chosen from the list, not your location)"}
                            status_full := Meta{text: "Loading…"}

                            hero := View{
                                width: Fill height: 180 flow: Overlay
                                sky_full := SkyView{ width: Fill height: Fill }
                                hero_text := View{
                                    width: Fill height: Fill flow: Down spacing: 2 padding: 16
                                    hero_city := OnSky{ draw_text +: { text_style: theme.font_regular{font_size: 14.0} } }
                                    hero_temp := OnSky{ draw_text +: { text_style: theme.font_regular{font_size: 46.0} } }
                                    hero_cond := OnSky{}
                                    hero_hilo := OnSky{}
                                }
                            }

                            metrics := View{
                                width: Fill height: Fit flow: Right{wrap: true} spacing: 8
                                Card{ width: 120 Meta{text: "WIND"} wind_value := Value{text: "–"} }
                                Card{ width: 120 Meta{text: "HUMIDITY"} humidity_value := Value{text: "–"} }
                                Card{ width: 120 Meta{text: "PRECIPITATION"} precip_value := Value{text: "–"} }
                            }

                            Meta{text: "5-DAY FORECAST"}
                            forecast := View{
                                width: Fill height: Fit flow: Right{wrap: true} spacing: 8
                                day0 := DayCard{}
                                day1 := DayCard{}
                                day2 := DayCard{}
                                day3 := DayCard{}
                                day4 := DayCard{}
                            }

                            attribution := Meta{text: "Weather data by Open-Meteo.com (CC BY 4.0)"}
                        }

                        tile: View{
                            width: Fill height: Fill flow: Overlay
                            sky_tile := SkyView{ width: Fill height: Fill draw_sky +: {border_radius: 0.0} }
                            tile_text := View{
                                width: Fill height: Fill flow: Down spacing: 2 padding: 16
                                align: Align{y: 0.5}
                                tile_city := OnSky{draw_text.text_style.font_size: 11.0}
                                tile_temp := OnSky{ draw_text +: { text_style: theme.font_regular{font_size: 32.0} } }
                                tile_cond := OnSky{draw_text.text_style.font_size: 10.0}
                                tile_hilo := OnSky{draw_text.text_style.font_size: 10.0}
                                tile_status := OnSky{visible: false}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: WeatherState,
    /// A slow tick that re-checks the 15 minute refresh budget and ages the
    /// "updated N min ago" line.
    #[rust]
    tick: Option<Timer>,
}

impl App {
    fn fetch(&mut self, cx: &mut Cx) {
        let (id, url) = self.state.begin_fetch();
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_header("Accept".into(), "application/json".into());
        cx.http_request(id, request);
        self.render(cx);
    }

    fn handle_response(&mut self, cx: &mut Cx, id: LiveId, result: Result<HttpResponse, String>) {
        if !self.state.owns(id) {
            // A reply for a request we already superseded (city switch or
            // manual refresh): drop it without touching the model.
            return;
        }
        let parsed = result.and_then(|response| {
            if response.status_code != 200 {
                return Err(format!("HTTP {}", response.status_code));
            }
            let body = response.body.as_ref().ok_or_else(|| "empty response".to_string())?;
            if body.len() > MAX_BODY_BYTES {
                return Err(format!("response too large ({} bytes)", body.len()));
            }
            let text = std::str::from_utf8(body).map_err(|_| "response is not UTF-8".to_string())?;
            model::Forecast::parse(text)
        });
        self.state.complete(id, parsed);
        self.render(cx);
    }

    /// Push the model into every label of both faces.
    fn render(&mut self, cx: &mut Cx) {
        let city = self.state.city();
        let status = self.state.status_text();
        let note = format!("{}, {}", city.name, city.country);
        self.ui.label(cx, ids!(city_note)).set_text(cx, &note);
        self.ui.label(cx, ids!(status_full)).set_text(cx, &status);
        self.ui.label(cx, ids!(tile_status)).set_text(cx, &status);
        self.ui.label(cx, ids!(hero_city)).set_text(cx, city.name);
        self.ui.label(cx, ids!(tile_city)).set_text(cx, city.name);
        self.ui.label(cx, ids!(attribution)).set_text(cx, ATTRIBUTION);

        let Some((forecast, _)) = self.state.current().map(|(f, t)| (f.clone(), t)) else {
            for id in [ids!(hero_temp), ids!(tile_temp)] {
                self.ui.label(cx, id).set_text(cx, "––°");
            }
            let placeholder = if self.state.loading() { "Loading…" } else { "No data" };
            for id in [ids!(hero_cond), ids!(tile_cond)] {
                self.ui.label(cx, id).set_text(cx, placeholder);
            }
            for id in [ids!(hero_hilo), ids!(tile_hilo), ids!(wind_value), ids!(humidity_value), ids!(precip_value)] {
                self.ui.label(cx, id).set_text(cx, "–");
            }
            for i in 0..5 {
                let day = &["day0", "day1", "day2", "day3", "day4"][i];
                let card = self.ui.widget(cx, &[LiveId::from_str(day)]);
                card.label(cx, ids!(name)).set_text(cx, "–");
                card.label(cx, ids!(cond)).set_text(cx, "");
                card.label(cx, ids!(hilo)).set_text(cx, "");
            }
            let kind = sky_kind(0);
            for id in [ids!(sky_full), ids!(sky_tile)] {
                if let Some(mut sky) = self.ui.widget(cx, id).borrow_mut::<SkyView>() {
                    sky.set_conditions(cx, kind, true);
                }
            }
            self.ui.redraw(cx);
            return;
        };

        let cur = &forecast.current;
        let code = forecast.code();
        let (hi, lo) = forecast.today_high_low();
        let temp_text = temp(cur.temperature_2m);
        let cond = describe(code);
        let hilo = format!("H {}  L {}", temp(hi), temp(lo));
        self.ui.label(cx, ids!(hero_temp)).set_text(cx, &temp_text);
        self.ui.label(cx, ids!(tile_temp)).set_text(cx, &temp_text);
        self.ui.label(cx, ids!(hero_cond)).set_text(cx, cond);
        self.ui.label(cx, ids!(tile_cond)).set_text(cx, cond);
        self.ui.label(cx, ids!(hero_hilo)).set_text(cx, &hilo);
        self.ui.label(cx, ids!(tile_hilo)).set_text(cx, &hilo);
        let wind = cur.wind_speed_10m.map(|w| format!("{:.0} km/h", w)).unwrap_or_else(|| "–".into());
        let humidity = cur.relative_humidity_2m.map(|h| format!("{:.0}%", h)).unwrap_or_else(|| "–".into());
        let precip = cur.precipitation.map(|p| format!("{:.1} mm", p)).unwrap_or_else(|| "–".into());
        self.ui.label(cx, ids!(wind_value)).set_text(cx, &wind);
        self.ui.label(cx, ids!(humidity_value)).set_text(cx, &humidity);
        self.ui.label(cx, ids!(precip_value)).set_text(cx, &precip);

        let days = forecast.days();
        for (i, day) in ["day0", "day1", "day2", "day3", "day4"].iter().enumerate() {
            let card = self.ui.widget(cx, &[LiveId::from_str(day)]);
            match days.get(i) {
                Some(d) => {
                    let name = if i == 0 { "Today".to_string() } else if d.weekday.is_empty() { d.date.clone() } else { d.weekday.to_string() };
                    card.label(cx, ids!(name)).set_text(cx, &name);
                    card.label(cx, ids!(cond)).set_text(cx, d.code.map(describe).unwrap_or("–"));
                    card.label(cx, ids!(hilo)).set_text(cx, &format!("{} / {}", temp(d.high), temp(d.low)));
                }
                None => {
                    card.label(cx, ids!(name)).set_text(cx, "–");
                    card.label(cx, ids!(cond)).set_text(cx, "");
                    card.label(cx, ids!(hilo)).set_text(cx, "");
                }
            }
        }

        let kind = sky_kind(code);
        let is_day = forecast.is_day();
        for id in [ids!(sky_full), ids!(sky_tile)] {
            if let Some(mut sky) = self.ui.widget(cx, id).borrow_mut::<SkyView>() {
                sky.set_conditions(cx, kind, is_day);
            }
        }
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        makepad_wm_api::set_title(cx, "Weather");
        self.ui.drop_down(cx, ids!(city_select)).set_labels(cx, CITIES.iter().map(|c| c.name.to_string()).collect());
        self.ui.drop_down(cx, ids!(city_select)).set_selected_item(cx, self.state.city);
        self.tick = Some(cx.start_interval(30.0));
        self.fetch(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(index) = self.ui.drop_down(cx, ids!(city_select)).selected(actions) {
            if self.state.select_city(index) {
                self.fetch(cx);
            }
        }
        if self.ui.button(cx, ids!(refresh)).clicked(actions) {
            self.fetch(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        sky::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowGeomChange(e) = event {
            let short = e.new_geom.inner_size.y < 150.0;
            self.ui.widget(cx, ids!(tile_hilo)).set_visible(cx, !short);
            if let Some(mut text) = self.ui.view(cx, ids!(tile_text)).borrow_mut() {
                text.layout.padding.top = if short {6.0} else {16.0};
                text.layout.padding.bottom = if short {6.0} else {16.0};
            }
            let size = if short {22.0} else {32.0};
            let mut temperature = self.ui.widget(cx, ids!(tile_temp));
            script_apply_eval!(cx, temperature, {draw_text.text_style.font_size: #(size)});
        }
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        if let Event::NetworkResponses(responses) = event {
            for response in responses {
                match response {
                    NetworkResponse::HttpResponse { request_id, response } => {
                        self.handle_response(cx, *request_id, Ok(response.clone()));
                    }
                    NetworkResponse::HttpError { request_id, error } => {
                        self.handle_response(cx, *request_id, Err(error.message.clone()));
                    }
                    _ => {}
                }
            }
        }
        if self.tick.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            if self.state.due_for_refresh() {
                self.fetch(cx);
            } else {
                self.render(cx);
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
