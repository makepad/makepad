//! The forecast surface: the DSL for both resident faces — the full
//! forecast and the compact home-widget tile the host switches to with
//! `HostedViewMode` over `Event::Custom` (the same message a standalone
//! window and a module's isolate both receive; the `HostedView` child
//! below reads it and swaps faces on its own — nothing here parses it) —
//! and the fetch, the tick timer and the last good forecast.
//!
//! The full face is the sky with the city and temperature leading, then
//! the hourly strip, the ten-day list, the detail cards and the footer,
//! all in one scroller; a glass button opens the locations sheet. The city
//! is chosen from a short, named list (never inferred from the machine's
//! location) and persisted to whatever storage a host gives this instance.
//! [`WeatherView::ensure_started`] seeds the first fetch on the first
//! event or draw, whichever the host gives it first: a window has a
//! `Startup` event, a module instance does not.

use crate::daily::ForecastDays;
use crate::hourly::{GlyphIcon, HourlyStrip};
use crate::model::{compass, describe, hhmm_of, temp, uv_text, Day, Forecast, SkyInputs, WeatherState, ATTRIBUTION, CITIES, MAX_BODY_BYTES};
use crate::parts::{PageDots, SolarArc};
use crate::sky::SkyView;
use makepad_widgets::makepad_platform::storage::{StorageHandle, StorageRequestId, StorageResponse, StorageResult};
use makepad_widgets::*;

/// The hero fades out and the compact header pins in over this scroll band.
const HEADER_PIN_START: f64 = 120.0;
const HEADER_PIN_END: f64 = 180.0;
/// The sky drifts for this long after an interaction or a data change,
/// then rests (the no-repaint-at-rest law).
const SKY_ANIM_TAIL: f64 = 1.2;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Inter 300 for the temperatures: the theme's family with its latin
    // member re-weighted, every fallback member kept.
    let Light = theme.font_regular{
        font_family +: {latin := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") weight: 300.0 asc: 0.0 desc: 0.0}}
    }
    let SkyText = Label{padding: 0 draw_text +: {
        color: theme.color_text_on_accent
        text_style: theme.font_regular{font_size: 12.75}
    }}
    let SkyDim = SkyText{draw_text.color: #ffffffe0}
    let SkyBold = SkyText{draw_text.text_style: theme.font_bold{font_size: 9.75}}
    let Surface = Label{padding: 0 draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 12.75}}}
    let SurfaceDim = Surface{draw_text.color: theme.color_text_disabled draw_text.text_style.font_size: 9.75}

    // One glass profile for every panel over the sky (the glass lane owns
    // the material underneath; these are the design's numbers).
    let ForecastPanel = GaussRoundedView{
        width: Fill height: Fit flow: Down padding: 16
        draw_bg +: {
            corner_radius: 12.0
            blur_level: 2.0 lensing_effect: 1.0
            lensing_strength: 1.5 lensing_width: 6.0
            tint_color: #102b48 tint_alpha: 0.20 surface_alpha: 1.0
            border_alpha: 0.10 border_width: 0.5 specular_strength: 0.0
        }
    }
    let Hairline = View{width: Fill height: 0.5 show_bg: true draw_bg.color: #ffffff1f}
    let DetailCard = ForecastPanel{
        width: Fill height: 179 flow: Down spacing: 0
        head := View{width: Fill height: 20 flow: Right spacing: 6 align: Align{y: 0.5}
            icon := Icon{width: 14 height: 14 icon_walk: Walk{width: 14 height: 14}}
            title := SkyBold{draw_text.color: #ffffffcc}
        }
        value := SkyText{margin: Inset{top: 8} draw_text.text_style: theme.font_regular{font_size: 24}}
        note := SkyText{width: Fill max_lines: 3 margin: Inset{top: 6} draw_text.text_style: theme.font_regular{font_size: 11.25}}
    }
    let Sheet = RoundedView{
        width: Fill height: Fit flow: Down
        draw_bg +: {color: theme.color_inset border_radius: 14.0}
    }
    let SheetRow = View{
        width: Fill height: 64 flow: Right align: Align{y: 0.5} padding: Inset{left: 20 right: 20}
        cursor: MouseCursor.Hand
        View{width: Fill height: Fit flow: Down spacing: 2
            city := Surface{draw_text.text_style: theme.font_bold{font_size: 12.75}}
            country := SurfaceDim{}
        }
        check := Surface{text: "✓" visible: false draw_text.color: theme.color_focus draw_text.text_style: theme.font_bold{font_size: 15}}
    }

    mod.widgets.WeatherViewBase = #(WeatherView::register_widget(vm))
    mod.widgets.WeatherView = set_type_default() do mod.widgets.WeatherViewBase{
        width: Fill height: Fill
        app_view := HostedView{
            full: View{width: Fill height: Fill flow: Overlay
                sky_full := SkyView{width: Fill height: Fill draw_sky +: {border_radius: 0.0 feather: 56.0}}
                forecast_scroll := ScrollYView{width: Fill height: Fill flow: Down
                    // The phone gets no desktop scroll bar: the handle is
                    // painted only when the scroller is wide (its clip rect
                    // is the room the face got); scrolling itself stays.
                    scroll_bars +: {scroll_bar_y +: {draw_bg +: {
                        pixel: fn() {
                            let wide = step(600.0, self.draw_clip.z - self.draw_clip.x)
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(1.0, self.rect_size.y * self.norm_scroll, self.size, self.rect_size.y * self.norm_handle, self.border_radius)
                            sdf.fill_keep(mix(self.color, mix(self.color_hover, self.color_drag, self.drag), self.hover))
                            sdf.stroke(mix(self.border_color, mix(self.border_color_hover, self.border_color_drag, self.drag), self.hover), self.border_size)
                            return sdf.result * wide
                        }
                    }}}
                    hero := View{width: Fill height: Fit flow: Down
                        padding: Inset{top: 24 left: 16 right: 16 bottom: 22}
                        align: Align{x: 0.5}
                        hero_city := SkyText{draw_text.text_style.font_size: 21}
                        hero_temp := SkyText{draw_text.text_style: Light{font_size: 75}}
                        hero_cond := SkyText{margin: Inset{top: 6} draw_text.text_style: theme.font_bold{font_size: 15}}
                        hero_hilo := SkyText{margin: Inset{top: 6}}
                    }
                    panels := View{width: Fill height: Fit flow: Down spacing: 12
                        padding: Inset{left: 16 right: 16 bottom: 88}
                        hourly := ForecastPanel{
                            summary := SkyText{width: Fill max_lines: 2 draw_text.text_style: theme.font_regular{font_size: 11.25}}
                            Hairline{margin: Inset{top: 8}}
                            hours := HourlyStrip{margin: Inset{top: 12}}
                        }
                        daily := ForecastPanel{
                            heading := SkyBold{text: "10-DAY FORECAST" draw_text.color: #ffffffcc}
                            days := ForecastDays{margin: Inset{top: 8}}
                        }
                        details := View{width: Fill height: Fit flow: Down spacing: 12
                            View{width: Fill height: Fit flow: Right spacing: 12
                                uv_card := DetailCard{head.title.text: "UV INDEX" head.icon.draw_icon.svg: crate_resource("self:resources/icons/uv.svg")
                                    uv_bar := View{width: Fill height: 4 margin: Inset{top: 10} show_bg: true
                                        draw_bg +: {
                                            level: uniform(0.0)
                                            pixel: fn(){
                                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 1.0)
                                                let c = mix(mix(vec4(0.36, 0.78, 0.45, 1.0), vec4(0.95, 0.83, 0.32, 1.0), smoothstep(0.0, 0.5, self.pos.x)), vec4(0.93, 0.35, 0.35, 1.0), smoothstep(0.5, 1.0, self.pos.x))
                                                sdf.fill(mix(vec4(1.0, 1.0, 1.0, 0.18), c, step(self.pos.x, self.level)))
                                                return sdf.result
                                            }
                                        }
                                    }
                                }
                                sun_card := DetailCard{head.title.text: "SUNRISE" head.icon.draw_icon.svg: crate_resource("self:resources/icons/sunrise.svg")
                                    arc := SolarArc{width: 80 height: 36 margin: Inset{top: 6}}
                                }
                            }
                            View{width: Fill height: Fit flow: Right spacing: 12
                                wind_card := DetailCard{head.title.text: "WIND" head.icon.draw_icon.svg: crate_resource("self:resources/icons/wind.svg")
                                    arrow := IconRotated{width: 32 height: 32 icon_walk: Walk{width: 32 height: 32} margin: Inset{top: 6}
                                        draw_icon +: {svg: crate_resource("self:resources/icons/arrow.svg")}}
                                }
                                humidity_card := DetailCard{head.title.text: "HUMIDITY" head.icon.draw_icon.svg: crate_resource("self:resources/icons/humidity.svg")}
                            }
                            View{width: Fill height: Fit flow: Right spacing: 12
                                precip_card := DetailCard{head.title.text: "PRECIPITATION" head.icon.draw_icon.svg: crate_resource("self:resources/icons/precip.svg")}
                                feels_card := DetailCard{head.title.text: "FEELS LIKE" head.icon.draw_icon.svg: crate_resource("self:resources/icons/feels.svg")}
                            }
                        }
                        footer := SkyDim{width: Fill draw_text.text_style.font_size: 9}
                    }
                }
                // The compact header pins in once the hero has scrolled away.
                header := View{visible: false width: Fill height: 92 flow: Overlay
                    scrim := View{width: Fill height: Fill show_bg: true
                        draw_bg +: {
                            color: #102b48
                            pixel: fn(){ return vec4(self.color.rgb * 0.55, 0.55) * (1.0 - smoothstep(0.45, 1.0, self.pos.y)) }
                        }
                    }
                    View{width: Fill height: 44 flow: Down align: Align{x: 0.5} padding: Inset{top: 4}
                        header_city := SkyText{draw_text.text_style: theme.font_bold{font_size: 12.75}}
                        header_line := SkyText{draw_text.text_style.font_size: 9.75}
                    }
                }
                chrome := glass.Layer{width: Fill height: Fill flow: Overlay
                    View{width: Fill height: Fill align: Align{x: 1.0 y: 1.0} padding: Inset{right: 16 bottom: 20}
                        locations := glass.GlassButton{width: 44 height: 44 text: "" padding: 0 spacing: 0
                            icon_walk: Walk{width: 22 height: 22}
                            draw_icon +: {svg: crate_resource("self:resources/icons/locations.svg")}
                        }
                    }
                    View{width: Fill height: Fill align: Align{x: 0.5 y: 1.0} padding: Inset{bottom: 42}
                        dots := PageDots{width: 44 height: 16}
                    }
                }
                sheet_layer := View{visible: false width: Fill height: Fill flow: Overlay
                    scrim_sheet := View{width: Fill height: Fill show_bg: true draw_bg.color: #00000066 cursor: MouseCursor.Default}
                    View{width: Fill height: Fill align: Align{y: 1.0}
                        sheet := Sheet{
                            View{width: Fill height: 56 align: Align{x: 0.5 y: 0.5}
                                Surface{text: "Locations" draw_text.text_style: theme.font_bold{font_size: 16.5}}
                            }
                            city_0 := SheetRow{}
                            city_1 := SheetRow{}
                            city_2 := SheetRow{}
                            city_3 := SheetRow{}
                            View{width: Fill height: 0.5 show_bg: true draw_bg.color: theme.color_text_disabled margin: Inset{left: 20 right: 20}}
                            refresh := SheetRow{height: 44}
                            View{width: Fill height: 20}
                        }
                    }
                }
            }
            tile: View{width: Fill height: Fill flow: Overlay
                sky_tile := SkyView{width: Fill height: Fill draw_sky +: {border_radius: 0.0 feather: 20.0}}
                tile_text := View{width: Fill height: Fill flow: Down
                    padding: Inset{left: 16 right: 16 top: 14}
                    tile_city := SkyText{width: Fill max_lines: 1 draw_text.text_style: theme.font_bold{font_size: 12}}
                    tile_temp := SkyText{margin: Inset{top: 2} draw_text.text_style: Light{font_size: 39}}
                    condition := View{width: Fill height: 24 margin: Inset{top: 10} flow: Right spacing: 6 align: Align{y: 0.5}
                        condition_icon := GlyphIcon{width: 24 height: 24}
                        tile_cond := SkyText{width: Fill max_lines: 1 draw_text.color: #ffffffe0 draw_text.text_style.font_size: 9.75}
                    }
                    tile_hilo := SkyText{margin: Inset{top: 6} draw_text.color: #ffffffe0 draw_text.text_style: theme.font_bold{font_size: 9.75}}
                }
            }
        }
    }
}

/// The forecast, the city choice and the sky art: one widget with two
/// resident faces (`app_view`, a `HostedView`). Owns everything the
/// standalone window's `App` used to own, so a module host gets the same
/// weather a window does.
#[derive(Script, ScriptHook, Widget)]
pub struct WeatherView {
    #[deref]
    view: View,
    /// Set once, on the first event or draw: a window seats this widget
    /// before its `Startup` event, a module host after `create` mints it.
    #[rust]
    started: bool,
    #[rust]
    state: WeatherState,
    /// A slow tick that re-checks the 15 minute refresh budget and ages
    /// the "updated N min ago" line.
    #[rust]
    tick: Option<Timer>,
    #[rust]
    last_size: Vec2d,
    /// The instance's storage jail (module) or its own namespace
    /// (standalone): the chosen city persists there instead of only for
    /// the session. Set before the first event by the module's `create`
    /// or the standalone window's startup.
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    city_load: Option<StorageRequestId>,
    #[rust]
    city_write: Option<StorageRequestId>,
    /// True once the person picks a city: a load response landing later
    /// (a slow jail on a cold start) must never clobber a fresh pick.
    #[rust]
    city_changed: bool,
    /// The sky's drift clock and the NextFrame chain that advances it —
    /// only for a short tail after an interaction or a data change.
    #[rust]
    sky_frame: NextFrame,
    #[rust]
    sky_time: f64,
    #[rust]
    sky_last: f64,
    #[rust]
    sky_until: f64,
    /// Whether the pinned header is up, so scroll changes redraw once.
    #[rust]
    header_shown: bool,
    #[rust]
    sheet_open: bool,
    /// A dev override of the sky's inputs (the standalone's `--sky` flag),
    /// so every condition can be looked at without waiting for weather.
    #[rust]
    sky_override: Option<SkyInputs>,
}

impl WeatherView {
    /// A host handed this instance its storage: the chosen city loads
    /// from and saves to it from now on.
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    /// Force the sky's inputs regardless of the forecast (dev only).
    pub fn set_sky_override(&mut self, cx: &mut Cx, inputs: Option<SkyInputs>) {
        self.sky_override = inputs;
        self.render(cx);
    }

    /// The text zone the sky keeps its art out of: one box around this
    /// frame's text (the ink, not the rows), so the sky follows the labels
    /// wherever the face puts them; the glass panels are a soft zone. A
    /// change redraws once more.
    fn sync_zones(&mut self, cx: &mut Cx2d) {
        // The ink bounds of a drawn widget; an undrawn one (no text yet, a
        // hidden row) adds nothing. A label's area is its glyph run, whose
        // rect is the first glyph's, so its box is that glyph's origin plus
        // the measured run (the zone's feather absorbs the slack); anything
        // else answers with its own rect.
        let ink = |view: &View, cx: &mut Cx2d, id: &[LiveId]| -> Option<Rect> {
            let widget = view.widget(cx, id);
            let area = widget.area();
            if !area.is_valid(cx) {
                return None;
            }
            let mut r = area.rect(cx);
            if let Some(label) = widget.borrow::<Label>() {
                let text = label.text();
                let run = label.draw_text.prepare_single_line_run(cx, &text)?;
                r.pos.y -= 2.0;
                r.size = dvec2(run.width_in_lpxs as f64, (run.ascender_in_lpxs - run.descender_in_lpxs) as f64);
            }
            (r.size.x > 0.0 && r.size.y > 0.0).then_some(r)
        };
        let union = |rects: &[Option<Rect>]| -> Rect {
            let mut out: Option<Rect> = None;
            for r in rects.iter().flatten() {
                out = Some(match out {
                    None => *r,
                    Some(u) => {
                        let p = dvec2(u.pos.x.min(r.pos.x), u.pos.y.min(r.pos.y));
                        let q = dvec2((u.pos.x + u.size.x).max(r.pos.x + r.size.x), (u.pos.y + u.size.y).max(r.pos.y + r.size.y));
                        Rect { pos: p, size: q - p }
                    }
                });
            }
            out.unwrap_or_default()
        };
        match self.face(cx) {
            HostedViewMode::Tile => {
                // The city and temperature are large type in a box that
                // leaves the top-right corner to the art; the condition and
                // H/L rows are small type in a full-width band to the
                // bottom, so their scrim reads as a vignette, not a box.
                let large = union(&[ink(&self.view, cx, ids!(tile_city)), ink(&self.view, cx, ids!(tile_temp))]);
                let rows = union(&[
                    ink(&self.view, cx, ids!(condition_icon)),
                    ink(&self.view, cx, ids!(tile_cond)),
                    ink(&self.view, cx, ids!(tile_hilo)),
                ]);
                let sky_rect = self.view.widget(cx, ids!(sky_tile)).area().rect(cx);
                let band = Rect { pos: dvec2(sky_rect.pos.x, rows.pos.y), size: dvec2(sky_rect.size.x, sky_rect.pos.y + sky_rect.size.y - rows.pos.y) };
                let small = if rows.size.y > 0.0 { band } else { Rect::default() };
                if let Some(mut sky) = self.view.widget(cx, ids!(sky_tile)).borrow_mut::<SkyView>() {
                    sky.set_zones(cx, &[large], &[small], &[]);
                }
            }
            HostedViewMode::Full => {
                let hero = union(&[
                    ink(&self.view, cx, ids!(hero_city)),
                    ink(&self.view, cx, ids!(hero_temp)),
                    ink(&self.view, cx, ids!(hero_cond)),
                    ink(&self.view, cx, ids!(hero_hilo)),
                ]);
                let footer = ink(&self.view, cx, ids!(footer)).unwrap_or_default();
                let panels = ink(&self.view, cx, ids!(panels)).unwrap_or_default();
                if let Some(mut sky) = self.view.widget(cx, ids!(sky_full)).borrow_mut::<SkyView>() {
                    sky.set_zones(cx, &[hero], &[footer], &[panels]);
                }
            }
        }
    }

    /// The face the host asked for (`HostedViewMode`, read off the
    /// embedded `HostedView`).
    pub fn face(&self, cx: &Cx) -> HostedViewMode {
        self.view.widget(cx, ids!(app_view)).borrow::<HostedView>().map(|h| h.mode()).unwrap_or_default()
    }

    /// One line for the assistant: the selected city, its status, and the
    /// current conditions and ten-day outlook when there is good data.
    pub fn ai_summary(&self) -> String {
        let city = self.state.city();
        let mut out = format!("{}, {} — {}", city.name, city.country, self.state.status_text());
        if let Some((forecast, _)) = self.state.current() {
            let cur = &forecast.current;
            out.push_str(&format!("; now {} {}", temp(cur.temperature_2m), describe(forecast.code())));
            let (hi, lo) = forecast.today_high_low();
            out.push_str(&format!(" (H {} L {})", temp(hi), temp(lo)));
            let days: Vec<String> = forecast
                .days()
                .iter()
                .map(|d| {
                    let name = if d.weekday.is_empty() { d.date.clone() } else { d.weekday.to_string() };
                    format!("{} {}/{}", name, temp(d.high), temp(d.low))
                })
                .collect();
            if !days.is_empty() {
                out.push_str(&format!("; forecast: {}", days.join(", ")));
            }
        }
        out
    }

    /// The timer stops and any fetch in flight is cancelled; nothing else
    /// this widget owns outlives its isolate.
    pub fn shutdown(&mut self, cx: &mut Cx) {
        if let Some(t) = self.tick.take() {
            cx.stop_timer(t);
        }
        if let Some((id, _)) = self.state.pending.take() {
            cx.cancel_http_request(id);
        }
    }

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
            Forecast::parse(text)
        });
        self.state.complete(id, parsed);
        self.render(cx);
        self.wake_sky(cx);
    }

    /// Select a city, persist the choice, and start fetching it.
    fn select_city(&mut self, cx: &mut Cx, index: usize) {
        if !self.state.select_city(index) {
            return;
        }
        self.city_changed = true;
        self.persist_city(cx);
        self.fetch(cx);
    }

    fn persist_city(&mut self, cx: &mut Cx) {
        if let Some(storage) = self.storage.as_ref() {
            self.city_write = Some(storage.set(cx, "city", self.state.city.to_string().into_bytes()));
        }
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.city_load == Some(response.request_id) {
                self.city_load = None;
                if !self.city_changed {
                    if let Ok(StorageResult::Value(Some(bytes))) = &response.result {
                        if let Some(index) = std::str::from_utf8(bytes).ok().and_then(|s| s.trim().parse::<usize>().ok()) {
                            if self.state.select_city(index) {
                                self.fetch(cx);
                            }
                        }
                    }
                }
            }
            if self.city_write == Some(response.request_id) {
                self.city_write = None;
                if let Err(e) = &response.result {
                    log!("weather: could not save the selected city: {}", e);
                }
            }
        }
    }

    fn set_sheet(&mut self, cx: &mut Cx, open: bool) {
        self.sheet_open = open;
        self.view.widget(cx, ids!(sheet_layer)).set_visible(cx, open);
        if open {
            let rows = [ids!(city_0), ids!(city_1), ids!(city_2), ids!(city_3)];
            for (i, row) in rows.iter().enumerate() {
                let row = self.view.widget(cx, *row);
                let city = &CITIES[i];
                row.label(cx, ids!(city)).set_text(cx, city.name);
                row.label(cx, ids!(country)).set_text(cx, city.country);
                row.widget(cx, ids!(check)).set_visible(cx, i == self.state.city);
            }
            let refresh = self.view.widget(cx, ids!(refresh));
            refresh.label(cx, ids!(city)).set_text(cx, "Refresh");
            refresh.widget(cx, ids!(country)).set_visible(cx, false);
        }
        self.view.redraw(cx);
    }

    fn wake_sky(&mut self, cx: &mut Cx) {
        self.sky_until = Cx::monotonic_now() + SKY_ANIM_TAIL;
        self.sky_last = 0.0;
        self.sky_frame = cx.new_next_frame();
    }

    /// The one-line summary over the hourly strip: what the next hours
    /// bring, and the wind when it gusts.
    fn summary_text(forecast: &Forecast) -> String {
        let cells = forecast.hour_cells();
        let mut change: Option<String> = None;
        let now_code = forecast.code();
        for cell in cells.iter().skip(1) {
            if let crate::model::HourCell::Hour { label, code, .. } = cell {
                if crate::model::sky_kind(*code) != crate::model::sky_kind(now_code) {
                    change = Some(format!("{} expected around {}:00.", describe(*code), label));
                    break;
                }
            }
        }
        let mut text = change.unwrap_or_else(|| format!("{} conditions will continue for the next hours.", describe(now_code)));
        if let Some(g) = forecast.current.wind_gusts_10m.filter(|g| *g >= 30.0) {
            text.push_str(&format!(" Wind gusts up to {} km/h.", g.round() as i64));
        }
        text
    }

    fn set_card(&mut self, cx: &mut Cx, card: &[LiveId], value: &str, note: &str) {
        let card = self.view.widget(cx, card);
        card.label(cx, ids!(value)).set_text(cx, value);
        card.label(cx, ids!(note)).set_text(cx, note);
    }

    /// Push the model into every label of both faces.
    fn render(&mut self, cx: &mut Cx) {
        let city = self.state.city();
        let status = self.state.status_text();
        self.view.label(cx, ids!(hero_city)).set_text(cx, city.name);
        self.view.label(cx, ids!(tile_city)).set_text(cx, city.name);
        self.view.label(cx, ids!(header_city)).set_text(cx, city.name);
        let freshness = self.state.tile_freshness();

        let Some((forecast, _)) = self.state.current().map(|(f, t)| (f.clone(), t)) else {
            for id in [ids!(hero_temp), ids!(tile_temp)] {
                self.view.label(cx, id).set_text(cx, "—°");
            }
            let placeholder = if self.state.loading() { "Loading" } else { "Unavailable" };
            for id in [ids!(hero_cond), ids!(tile_cond), ids!(header_line)] {
                self.view.label(cx, id).set_text(cx, placeholder);
            }
            for id in [ids!(hero_hilo), ids!(tile_hilo), ids!(summary)] {
                self.view.label(cx, id).set_text(cx, "");
            }
            self.view.widget(cx, ids!(condition_icon)).set_visible(cx, false);
            for card in [ids!(uv_card), ids!(sun_card), ids!(wind_card), ids!(humidity_card), ids!(precip_card), ids!(feels_card)] {
                self.set_card(cx, card, "—", "Unavailable");
            }
            self.view.label(cx, ids!(footer)).set_text(cx, &format!("{status} · {ATTRIBUTION}"));
            if let Some(mut hours) = self.view.widget(cx, ids!(hours)).borrow_mut::<HourlyStrip>() {
                hours.set_cells(cx, Vec::new());
            }
            if let Some(mut days) = self.view.widget(cx, ids!(days)).borrow_mut::<ForecastDays>() {
                days.set_days(cx, Vec::new(), None);
            }
            for id in [ids!(sky_full), ids!(sky_tile)] {
                if let Some(mut sky) = self.view.widget(cx, id).borrow_mut::<SkyView>() {
                    match self.sky_override {
                        Some(inputs) => sky.set_conditions(cx, inputs),
                        None => sky.set_unknown(cx),
                    }
                }
            }
            self.view.redraw(cx);
            return;
        };

        let cur = &forecast.current;
        let code = forecast.code();
        let (hi, lo) = forecast.today_high_low();
        let temp_text = temp(cur.temperature_2m);
        let cond = describe(code);
        let hilo = format!("H:{}  L:{}", temp(hi), temp(lo));
        self.view.label(cx, ids!(hero_temp)).set_text(cx, &temp_text);
        self.view.label(cx, ids!(tile_temp)).set_text(cx, &temp_text);
        self.view.label(cx, ids!(hero_cond)).set_text(cx, cond);
        self.view.label(cx, ids!(tile_cond)).set_text(cx, cond);
        self.view.label(cx, ids!(hero_hilo)).set_text(cx, &hilo);
        self.view.label(cx, ids!(tile_hilo)).set_text(cx, freshness.as_deref().unwrap_or(&hilo));
        self.view.label(cx, ids!(header_line)).set_text(cx, &format!("{} · {}", temp_text, cond));
        self.view.label(cx, ids!(summary)).set_text(cx, &Self::summary_text(&forecast));
        self.view.widget(cx, ids!(condition_icon)).set_visible(cx, true);
        if let Some(mut icon) = self.view.widget(cx, ids!(condition_icon)).borrow_mut::<GlyphIcon>() {
            icon.set_glyph(cx, Some(crate::model::glyph(code, forecast.is_day())));
        }

        if let Some(mut hours) = self.view.widget(cx, ids!(hours)).borrow_mut::<HourlyStrip>() {
            hours.set_cells(cx, forecast.hour_cells());
        }
        let days: Vec<Day> = forecast.days();
        if let Some(mut list) = self.view.widget(cx, ids!(days)).borrow_mut::<ForecastDays>() {
            list.set_days(cx, days.clone(), cur.temperature_2m);
        }

        // The detail cards.
        let uv = forecast.current_uv();
        let (uv_value, uv_word) = uv_text(uv);
        let uv_note = match (uv, days.first().and_then(|d| d.uv_max)) {
            (Some(_), Some(max)) => format!("{uv_word}. Peaks at {} today.", max.round() as i64),
            (Some(_), None) => uv_word.to_string(),
            _ => "Unavailable".into(),
        };
        self.set_card(cx, ids!(uv_card), &uv_value, &uv_note);
        let mut uv_bar = self.view.widget(cx, ids!(uv_bar));
        let level = (uv.unwrap_or(0.0) / 11.0).clamp(0.0, 1.0) as f32;
        script_apply_eval!(cx, uv_bar, { draw_bg.level: #(level) });

        let today = days.first();
        let (rise, set) = today.map(|d| (d.sunrise.clone(), d.sunset.clone())).unwrap_or((None, None));
        match (&rise, &set) {
            (Some(r), Some(s)) => {
                self.set_card(cx, ids!(sun_card), &hhmm_of(r), &format!("Sunset {}", hhmm_of(s)));
                let phase = cur.time.as_deref().map(|now| crate::model::solar_phase(now, Some(r), Some(s), forecast.is_day())).unwrap_or(1.0);
                let t = day_fraction(cur.time.as_deref(), r, s).unwrap_or(if phase > 0.5 { 0.5 } else { -1.0 });
                if let Some(mut arc) = self.view.widget(cx, ids!(arc)).borrow_mut::<SolarArc>() {
                    arc.set_progress(cx, t as f32);
                }
            }
            _ => self.set_card(cx, ids!(sun_card), "—", "Unavailable"),
        }

        match cur.wind_speed_10m {
            Some(w) => {
                let dir = cur.wind_direction_10m;
                let note = match (dir, cur.wind_gusts_10m) {
                    (Some(d), Some(g)) => format!("From the {}. Gusts {} km/h.", compass(d), g.round() as i64),
                    (Some(d), None) => format!("From the {}.", compass(d)),
                    _ => "".into(),
                };
                self.set_card(cx, ids!(wind_card), &format!("{} km/h", w.round() as i64), &note);
                let mut arrow = self.view.widget(cx, ids!(arrow));
                // The arrow points where the wind blows TO: the reported
                // direction is where it comes from.
                let angle = (dir.unwrap_or(0.0) + 180.0).to_radians() as f32;
                script_apply_eval!(cx, arrow, { draw_icon.rotation_angle: #(angle) });
            }
            None => self.set_card(cx, ids!(wind_card), "—", "Unavailable"),
        }
        match cur.relative_humidity_2m {
            Some(h) => self.set_card(cx, ids!(humidity_card), &format!("{}%", h.round() as i64), if h >= 70.0 { "Humid." } else if h <= 30.0 { "Dry air." } else { "Comfortable." }),
            None => self.set_card(cx, ids!(humidity_card), "—", "Unavailable"),
        }
        match today.and_then(|d| d.precip_sum) {
            Some(sum) => {
                let chance = today.and_then(|d| d.precip_chance).map(|c| format!(" {}% chance.", c.round() as i64)).unwrap_or_default();
                self.set_card(cx, ids!(precip_card), &format!("{:.1} mm", sum), &format!("Forecast today.{chance}"));
            }
            None => self.set_card(cx, ids!(precip_card), "—", "Unavailable"),
        }
        match cur.apparent_temperature {
            Some(a) => {
                let note = match cur.temperature_2m {
                    Some(t) if a < t - 1.5 => "Wind makes it feel cooler.",
                    Some(t) if a > t + 1.5 => "Humidity makes it feel warmer.",
                    _ => "Similar to the actual temperature.",
                };
                self.set_card(cx, ids!(feels_card), &temp(Some(a)), note);
            }
            None => self.set_card(cx, ids!(feels_card), "—", "Unavailable"),
        }
        let updated = cur.time.as_deref().map(hhmm_of).unwrap_or_else(|| "—".into());
        self.view.label(cx, ids!(footer)).set_text(cx, &format!("Updated {updated} · {ATTRIBUTION}"));

        let inputs = self.sky_override.unwrap_or_else(|| forecast.sky_inputs());
        for id in [ids!(sky_full), ids!(sky_tile)] {
            if let Some(mut sky) = self.view.widget(cx, id).borrow_mut::<SkyView>() {
                sky.set_conditions(cx, inputs);
            }
        }
        self.view.redraw(cx);
    }

    /// The faces follow the room they are given: the tile's breathing
    /// room and its landscape/medium variants, the full face's landscape.
    fn apply_sizing(&mut self, cx: &mut Cx, size: Vec2d) {
        let landscape_tile = size.y < 100.0 && size.y >= 60.0;
        let medium = size.x >= 300.0 && size.y < 200.0;
        let pad = if size.x >= 176.0 && size.y >= 176.0 && !medium { 18.0 } else { 14.0 };
        if let Some(mut text) = self.view.view(cx, ids!(tile_text)).borrow_mut() {
            text.layout.padding.top = if landscape_tile { 10.0 } else { pad };
            text.layout.padding.left = if landscape_tile { 12.0 } else { 16.0 };
            text.layout.padding.right = if landscape_tile { 12.0 } else { 16.0 };
        }
        // Type in makepad points: the design's logical sizes times 3/4.
        let temp_size = if landscape_tile { 22.5 } else if size.y < 176.0 && size.y < 200.0 && size.x < 300.0 { 30.0 } else { 39.0 };
        let mut temperature = self.view.widget(cx, ids!(tile_temp));
        script_apply_eval!(cx, temperature, { draw_text.text_style.font_size: #(temp_size) });
        self.view.widget(cx, ids!(condition)).set_visible(cx, !landscape_tile);
        self.view.widget(cx, ids!(tile_hilo)).set_visible(cx, size.y >= 60.0);
        // The full face in landscape: the hero shrinks so the panels get room.
        let landscape_full = size.x >= 650.0 && size.y < 500.0;
        let mut hero_temp = self.view.widget(cx, ids!(hero_temp));
        let temp_full = if landscape_full { 60.0 } else { 75.0 };
        script_apply_eval!(cx, hero_temp, { draw_text.text_style.font_size: #(temp_full) });
    }

    /// The compact header pins in as the hero scrolls out.
    fn sync_header(&mut self, cx: &mut Cx) {
        let hero = self.view.widget(cx, ids!(hero)).area().rect(cx);
        let root = self.view.widget(cx, ids!(forecast_scroll)).area().rect(cx);
        let scrolled = (root.pos.y - hero.pos.y).max(0.0);
        let t = ((scrolled - HEADER_PIN_START) / (HEADER_PIN_END - HEADER_PIN_START)).clamp(0.0, 1.0);
        let shown = t > 0.0;
        if shown != self.header_shown {
            self.header_shown = shown;
            self.view.widget(cx, ids!(header)).set_visible(cx, shown);
        }
        if shown {
            let alpha = t as f32;
            let mut header = self.view.widget(cx, ids!(header));
            script_apply_eval!(cx, header, { header_city.draw_text.color: #(vec4(1.0, 1.0, 1.0, alpha)) header_line.draw_text.color: #(vec4(1.0, 1.0, 1.0, alpha)) });
        }
        // The hero fades as the header arrives, never both at full ink.
        let hero_alpha = (1.0 - t) as f32;
        let mut hero = self.view.widget(cx, ids!(hero));
        let c = vec4(1.0, 1.0, 1.0, hero_alpha);
        script_apply_eval!(cx, hero, { hero_city.draw_text.color: #(c) hero_temp.draw_text.color: #(c) hero_cond.draw_text.color: #(c) hero_hilo.draw_text.color: #(c) });
    }

    /// Start on the first event or draw, whichever comes first: a window
    /// seats this view before its `Startup` event, a module host after
    /// `create` mints it — either way before anything else reaches it.
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(storage) = self.storage.as_ref() {
            self.city_load = Some(storage.get(cx, "city"));
        }
        self.tick = Some(cx.start_interval(30.0));
        if let Some(mut dots) = self.view.widget(cx, ids!(dots)).borrow_mut::<PageDots>() {
            dots.set(cx, CITIES.len(), self.state.city);
        }
        self.fetch(cx);
    }
}

/// Where `now` sits between sunrise (0) and sunset (1); none outside.
fn day_fraction(now: Option<&str>, sunrise: &str, sunset: &str) -> Option<f64> {
    let minute = |s: &str| -> Option<f64> {
        let t = s.split('T').nth(1)?;
        let mut p = t.split(':');
        Some(p.next()?.parse::<f64>().ok()? * 60.0 + p.next()?.get(0..2)?.parse::<f64>().ok()?)
    };
    let (n, r, s) = (minute(now?)?, minute(sunrise)?, minute(sunset)?);
    if s <= r || n < r || n > s {
        return None;
    }
    Some((n - r) / (s - r))
}

impl Widget for WeatherView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
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
        if let Some(frame) = self.sky_frame.is_event(event) {
            if self.sky_last > 0.0 {
                self.sky_time += (frame.time - self.sky_last).clamp(0.0, 0.05);
            }
            self.sky_last = frame.time;
            if let Some(mut sky) = self.view.widget(cx, ids!(sky_full)).borrow_mut::<SkyView>() {
                sky.set_time(cx, self.sky_time);
            }
            if Cx::monotonic_now() < self.sky_until && self.face(cx) == HostedViewMode::Full {
                self.sky_frame = cx.new_next_frame();
            }
        }
        if matches!(event, Event::MouseDown(_) | Event::TouchUpdate(_) | Event::Scroll(_)) {
            self.wake_sky(cx);
        }
        if let Event::BackPressed { .. } = event {
            if self.sheet_open && event.back_pressed() {
                self.set_sheet(cx, false);
                return;
            }
        }
        if let Event::Actions(actions) = event {
            if self.view.widget(cx, ids!(locations)).borrow::<GlassButton>().is_some_and(|b| b.clicked(actions)) {
                self.set_sheet(cx, true);
            }
            if self.view.widget(cx, ids!(scrim_sheet)).view(cx, &[]).finger_down(actions).is_some() {
                self.set_sheet(cx, false);
            }
            for (i, row) in [ids!(city_0), ids!(city_1), ids!(city_2), ids!(city_3)].iter().enumerate() {
                if self.view.view(cx, *row).finger_up(actions).is_some() {
                    self.select_city(cx, i);
                    if let Some(mut dots) = self.view.widget(cx, ids!(dots)).borrow_mut::<PageDots>() {
                        dots.set(cx, CITIES.len(), self.state.city);
                    }
                    self.set_sheet(cx, false);
                }
            }
            if self.view.view(cx, ids!(refresh)).finger_up(actions).is_some() {
                self.fetch(cx);
                self.set_sheet(cx, false);
            }
        }
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::Scroll(_) | Event::TouchUpdate(_) | Event::MouseMove(_) | Event::MouseUp(_)) && self.face(cx) == HostedViewMode::Full {
            self.sync_header(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        if size.x >= 1.0 && (size.x - self.last_size.x).abs() + (size.y - self.last_size.y).abs() > 0.5 {
            self.last_size = size;
            self.apply_sizing(cx, size);
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() {
            self.sync_zones(cx);
        }
        step
    }
}

