//! makepad-example-fractal-zoom: a deep-zooming fractal explorer rendered on
//! the CPU, in parallel SIMD tiles, and coloured on the GPU.
//!
//! ```text
//! makepad-example-fractal-zoom [--type N] [--view X Y WIDTH] [--threads N] [--place N] [--tour] [--palette N]
//!                              [--trap] [--local] [--show-workers] [--bench [RUNS]]
//! ```
//!
//! Click to fly in 3x, drag to pan, scroll or pinch to zoom, press and hold
//! to zoom in (right button or shift to zoom out). Keys: arrows
//! pan, + and - zoom, R resets the view, T toggles the tour. `--bench` renders two fixed views
//! as full layers, prints their wall times and quits.

use makepad_widgets::*;

mod deep;
mod kernels;
mod palette;
#[cfg(test)]
mod parity;
mod remote;
mod simd;
mod splash_math;
mod tiles;
mod view;

use kernels::{FractalKind, Params};
use palette::{PaletteGrid, PaletteGridAction, PALETTES};
use view::{Bench, FractalView, FractalViewAction, Look, Quality};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Section = Label{
        margin: Inset{top: 6}
        text: ""
        draw_text +: {
            color: #x9aa4b8
            text_style: theme.font_bold{ font_size: 8 }
        }
    }
    let Knob = Slider{ width: Fill height: 34 }
    let PlaceButton = ButtonFlat{ width: Fill text: "" }
    let Hint = Label{
        width: Fill
        text: ""
        draw_text +: { color: #x6c7488 text_style: theme.font_regular{ font_size: 8 } }
    }
    // one fixed row per stat: the panel's layout never follows the numbers
    let Stat = Label{
        width: Fill
        height: 14
        text: ""
        draw_text +: {
            color: #xc8d2e6
            text_style: theme.font_code{ font_size: 8 }
        }
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 900)
                window.title: "Fractal Zoom"
                pass.clear_color: #x07080c
                body +: {
                    flow: Right
                    view_box := View{
                        width: Fill height: Fill
                        clip_x: true clip_y: true
                        fractal := FractalView{}
                    }
                    panel := ScrollYView{
                        width: 330 height: Fill
                        flow: Down padding: Inset{left: 16 right: 16 top: 14 bottom: 14} spacing: 6
                        show_bg: true
                        draw_bg +: { color: #x11141b }

                        View{
                            width: Fill height: Fit flow: Right align: Align{y: 0.5}
                            H3{ width: Fill text: "Fractal Zoom" }
                            reset := ButtonFlat{ text: "Reset" }
                        }
                        stats := RoundedView{
                            width: Fill height: 206 flow: Down padding: 8 spacing: 2 clip_y: true
                            show_bg: true
                            draw_bg +: { color: #x0a0c11 border_radius: 6.0 }
                            stat_fps := Stat{}
                            stat_render := Stat{}
                            stat_threads := Stat{}
                            stat_view := Stat{}
                            stat_iter := Stat{}
                            node0 := Stat{}
                            node1 := Stat{}
                            node2 := Stat{}
                            node3 := Stat{}
                            node4 := Stat{}
                            node5 := Stat{}
                            node6 := Stat{}
                            node7 := Stat{}
                            node8 := Stat{}
                            node9 := Stat{}
                        }

                        Section{ text: "PLACES" }
                        View{ width: Fill height: Fit flow: Right spacing: 6
                            place0 := PlaceButton{ text: "Overview" }
                            place1 := PlaceButton{ text: "Seahorse Valley" }
                        }
                        View{ width: Fill height: Fit flow: Right spacing: 6
                            place2 := PlaceButton{ text: "Elephant Valley" }
                            place3 := PlaceButton{ text: "Spiral" }
                        }
                        View{ width: Fill height: Fit flow: Right spacing: 6
                            place4 := PlaceButton{ text: "Mini Mandelbrot" }
                            place5 := PlaceButton{ text: "Deep Seahorse" }
                        }
                        View{ width: Fill height: Fit flow: Right spacing: 6
                            place6 := PlaceButton{ text: "Dendrite 1e-20" }
                            place7 := PlaceButton{ text: "Julia Rabbit" }
                        }
                        View{ width: Fill height: Fit flow: Right spacing: 6
                            place8 := PlaceButton{ text: "Julia Dragon" }
                            place9 := PlaceButton{ text: "Ship Armada" }
                        }
                        tour := Button{ width: Fill text: "Start tour" }

                        Section{ text: "FRACTAL" }
                        kind := DropDown{ width: Fill labels: ["Mandelbrot" "Julia" "Burning Ship" "Tricorn" "Multibrot" "Newton"] }
                        power_box := View{ width: Fill height: Fit visible: false power := Knob{ text: "Power" min: 2 max: 8 step: 1 precision: 0 default: 3 } }
                        julia_box := View{
                            width: Fill height: Fit flow: Down spacing: 4
                            visible: false
                            Hint{ text: "Drag on the map to pick the Julia constant" }
                            julia_frame := RoundedView{
                                width: Fill height: 180
                                show_bg: true
                                draw_bg +: { color: #x000000 border_radius: 6.0 }
                                clip_x: true clip_y: true
                                picker := FractalView{ picker: true }
                            }
                            julia_c := Stat{ text: "c = -0.7269 + 0.1889i" }
                        }
                        iter := Knob{ text: "Iterations" min: 32 max: 20000 step: 1 precision: 0 default: 500 }
                        auto_iter := CheckBox{ text: "Raise iterations with depth" }

                        Section{ text: "COLOUR" }
                        palettes := PaletteGrid{}
                        mapping := DropDown{ width: Fill labels: ["Original colouring" "Smooth colouring" "Logarithmic colouring"] }
                        density := Knob{ text: "Density" min: 0.05 max: 8.0 step: 0.01 default: 1.0 }
                        offset := Knob{ text: "Offset" min: 0.0 max: 1.0 step: 0.001 default: 0.0 }
                        cycle := Knob{ text: "Cycle speed" min: -0.5 max: 0.5 step: 0.005 default: 0.05 }
                        hue := Knob{ text: "Hue shift" min: -3.1416 max: 3.1416 step: 0.01 default: 0.0 }
                        de := CheckBox{ text: "Distance estimation" }
                        de_strength := Knob{ text: "Edge shading" min: 0.0 max: 1.0 step: 0.01 default: 0.6 }
                        glow := Knob{ text: "Filament glow" min: 0.0 max: 1.5 step: 0.01 default: 0.15 }
                        trap := CheckBox{ text: "Orbit trap colouring" }
                        inside := DropDown{ width: Fill labels: ["Inside: black" "Inside: midnight" "Inside: white"] }

                        Section{ text: "RENDERING" }
                        quality := DropDown{ width: Fill labels: ["Draft (half resolution)" "Full resolution" "Supersampled 2x2"] }
                        network := CheckBox{ text: "Network rendering on the AI hub nodes" }
                        stat_network := Stat{}
                        ns0 := Stat{} ns1 := Stat{} ns2 := Stat{} ns3 := Stat{}
                        ns4 := Stat{} ns5 := Stat{} ns6 := Stat{} ns7 := Stat{}
                        show_workers := CheckBox{ text: "Show which worker rendered each tile" }
                        Hint{
                            margin: Inset{top: 8}
                            text: "Click to fly in. Drag to pan, scroll or pinch to zoom. Hold to zoom in, right-hold or shift to zoom out. Arrows pan, + and - zoom, R resets, T starts or stops the tour."
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
    params: Params,
    #[rust]
    look: Look,
    #[rust]
    base_iter: u32,
    #[rust]
    auto_iter: bool,
}

/// Iterations for a view `width` across: more with depth, in quarter-octave
/// steps so a zoom changes them (and re-renders) only now and then.
fn auto_iterations(base: u32, width: f64) -> u32 {
    let depth = (3.0 / width.max(1e-300)).log10().max(0.0);
    let want = base as f64 * (1.0 + 0.6 * depth);
    let q = (want.log2() * 4.0).round() / 4.0;
    (q.exp2() as u32).clamp(base, 100_000)
}

const INSIDE: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [0.02, 0.03, 0.08], [1.0, 1.0, 1.0]];

/// A famous place: fractal, view (center, width) and the Julia constant.
struct Place {
    kind: FractalKind,
    view: (f64, f64, f64),
    julia: (f64, f64),
}

const NO_JULIA: (f64, f64) = (-0.7269, 0.1889);

const PLACES: [Place; 10] = [
    Place { kind: FractalKind::Mandelbrot, view: (-0.6, 0.0, 3.4), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (-0.75, 0.1, 0.05), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (0.2925, 0.0164, 0.024), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (-0.77568377, 0.13646737, 0.0006), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (-1.7548776, 0.0, 0.045), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (-0.743_643_887_037_158_7, 0.131_825_904_205_311_97, 4.0e-11), julia: NO_JULIA },
    Place { kind: FractalKind::Mandelbrot, view: (0.0, 1.0, 1.0e-20), julia: NO_JULIA },
    Place { kind: FractalKind::Julia, view: (0.0, 0.0, 3.2), julia: (-0.123, 0.745) },
    Place { kind: FractalKind::Julia, view: (0.0, 0.0, 3.2), julia: (-0.8, 0.156) },
    Place { kind: FractalKind::BurningShip, view: (-1.762, -0.028, 0.06), julia: NO_JULIA },
];

/// The tour: the Mandelbrot places, then back out.
const TOUR: [usize; 7] = [1, 3, 5, 4, 2, 6, 0];

impl App {
    fn with_view<R>(&self, cx: &mut Cx, id: &[LiveId], f: impl FnOnce(&mut Cx, &mut FractalView) -> R) -> Option<R> {
        let w = self.ui.widget(cx, id);
        let mut v = w.borrow_mut::<FractalView>()?;
        Some(f(cx, &mut v))
    }

    fn push_params(&mut self, cx: &mut Cx) {
        let mut p = self.params;
        if self.auto_iter {
            if let Some((_, _, w)) = self.with_view(cx, ids!(fractal), |_, v| v.view()) {
                if w > 0.0 {
                    p.max_iter = auto_iterations(self.base_iter, w);
                }
            }
        }
        self.with_view(cx, ids!(fractal), |cx, v| v.set_params(cx, p));
        let picker = Params { kind: FractalKind::Mandelbrot, max_iter: 300, ..p };
        self.with_view(cx, ids!(picker), |cx, v| v.set_params(cx, picker));
        self.ui.view(cx, ids!(power_box)).set_visible(cx, p.kind.uses_power());
        let julia = self.params.kind == FractalKind::Julia;
        self.ui.view(cx, ids!(julia_box)).set_visible(cx, julia);
        self.ui.label(cx, ids!(julia_c)).set_text(cx, &format!("c = {:.5} {:+.5}i", p.julia.0, p.julia.1));
    }

    fn push_look(&mut self, cx: &mut Cx) {
        self.look.norm_iter = self.base_iter;
        let look = self.look;
        self.with_view(cx, ids!(fractal), |cx, v| v.set_look(cx, look));
        self.with_view(cx, ids!(picker), |cx, v| v.set_look(cx, Look { cycle: 0.0, ..look }));
    }

    fn update_stats(&mut self, cx: &mut Cx) {
        let Some(s) = self.with_view(cx, ids!(fractal), |_, v| v.stats.clone()) else { return };
        // fixed-width fields, one row each, short enough never to wrap
        let mut lines = vec![
            (ids!(stat_fps), format!("{:5.1} fps  {:6.2} ms/frame", s.fps, s.frame_ms)),
            (ids!(stat_render), format!("view {:8.1} ms  {:6.1} Mpix/s", s.layer_ms, s.mpix_s)),
            (ids!(stat_threads), format!("{:2} threads {:4} tiles @{:<3} {:4} left", s.threads, s.layer_tiles, s.layer_res, s.pending)),
            (ids!(stat_view), format!("width {:9.2e}  {}", s.width, s.precision)),
            (ids!(stat_iter), format!("iterations {:6}", self.current_iter(cx))),
            (ids!(stat_network), format!("{:<20} {:5} hedged {:4} dropped", s.network, s.hedged, s.dropped)),
        ];
        let ns_ids = [ids!(ns0), ids!(ns1), ids!(ns2), ids!(ns3), ids!(ns4), ids!(ns5), ids!(ns6), ids!(ns7)];
        for (k, id) in ns_ids.iter().enumerate() {
            let text = s
                .node_status
                .get(k)
                .map(|(addr, state)| {
                    let short = state.split(" [").next().unwrap_or(state).split(" (").next().unwrap_or(state).split(':').next().unwrap_or(state);
                    let addr: String = addr.chars().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("{:<20} {}", addr, short.chars().take(26).collect::<String>())
                })
                .unwrap_or_default();
            lines.push((*id, text));
        }
        // every runner, here and each connected node, with its share of the
        // last full view (a node that got no tile of it still shows, at 0)
        let mut runners: Vec<(u16, String)> = vec![(0, "local".to_string())];
        runners.extend(s.node_names.iter().map(|(id, name)| (*id, name.split(':').next().unwrap_or(name).to_string())));
        let node_ids = [ids!(node0), ids!(node1), ids!(node2), ids!(node3), ids!(node4), ids!(node5), ids!(node6), ids!(node7), ids!(node8), ids!(node9)];
        for (k, id) in node_ids.iter().enumerate() {
            let text = match runners.get(k) {
                Some((n, name)) => {
                    let (c, r) = s.node_rates.iter().find(|(m, _, _)| m == n).map(|(_, c, r)| (*c, *r)).unwrap_or((0, 0.0));
                    format!("{:<15}{:5} tiles {:7.0}/s", name, c, r)
                }
                None => String::new(),
            };
            lines.push((*id, text));
        }
        for (id, text) in lines {
            let label = self.ui.label(cx, id);
            if label.text() != text {
                label.set_text(cx, &text);
            }
        }
    }

    fn go_to(&mut self, cx: &mut Cx, place: &Place) {
        if self.params.kind != place.kind || (place.kind == FractalKind::Julia && self.params.julia != place.julia) {
            self.params.kind = place.kind;
            self.params.julia = place.julia;
            let i = FractalKind::ALL.iter().position(|k| *k == place.kind).unwrap_or(0);
            self.ui.drop_down(cx, ids!(kind)).set_selected_item(cx, i);
            self.with_view(cx, ids!(picker), |cx, v| v.set_pick(cx, place.julia));
            self.push_params(cx);
        }
        let (x, y, w) = place.view;
        self.with_view(cx, ids!(fractal), |cx, v| v.fly_to(cx, x, y, w));
    }

    fn toggle_tour(&mut self, cx: &mut Cx) {
        let touring = self.with_view(cx, ids!(fractal), |_, v| v.touring()).unwrap_or(false);
        if touring {
            self.with_view(cx, ids!(fractal), |_, v| v.stop_tour());
        } else {
            // the tour is in the Mandelbrot set
            if self.params.kind != FractalKind::Mandelbrot {
                self.go_to(cx, &PLACES[0]);
            }
            let places: Vec<(f64, f64, f64)> = TOUR.iter().map(|i| PLACES[*i].view).collect();
            self.with_view(cx, ids!(fractal), |cx, v| v.start_tour(cx, places));
        }
        self.set_tour_label(cx, !touring);
    }

    fn set_tour_label(&mut self, cx: &mut Cx, on: bool) {
        self.ui.button(cx, ids!(tour)).set_text(cx, if on { "Stop tour" } else { "Start tour" });
    }

    fn current_iter(&self, cx: &mut Cx) -> u32 {
        self.with_view(cx, ids!(fractal), |_, v| v.params.max_iter).unwrap_or(0)
    }
}

impl MatchEvent for App {
    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        // keys drive the view whatever widget has focus
        let used = self.with_view(cx, ids!(fractal), |cx, v| v.key(cx, e.key_code)).unwrap_or(false);
        if used {
            self.set_tour_label(cx, false);
        } else if e.key_code == KeyCode::KeyT {
            self.toggle_tour(cx);
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        self.params = Params::default();
        self.base_iter = self.params.max_iter;
        self.look = Look { palette_count: PALETTES.len(), ..Look::default() };
        let atlas = palette::palette_atlas(cx);
        self.with_view(cx, ids!(fractal), |cx, v| v.set_palette_atlas(cx, atlas.clone()));
        self.with_view(cx, ids!(picker), |cx, v| {
            v.set_palette_atlas(cx, atlas.clone());
            v.set_pick(cx, Params::default().julia);
        });
        if let Some(mut g) = self.ui.widget(cx, ids!(palettes)).borrow_mut::<PaletteGrid>() {
            g.set_atlas(cx, atlas.clone());
        }
        self.ui.drop_down(cx, ids!(quality)).set_selected_item(cx, 2);
        self.auto_iter = true;
        self.ui.check_box(cx, ids!(auto_iter)).set_active(cx, true, Animate::No);
        self.ui.check_box(cx, ids!(de)).set_active(cx, self.params.de, Animate::No);
        self.ui.check_box(cx, ids!(network)).set_active(cx, true, Animate::No);
        self.push_params(cx);
        self.push_look(cx);

        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            if a == "--view" {
                let v: Vec<f64> = (0..3).filter_map(|_| args.next().and_then(|s| s.parse().ok())).collect();
                if let [x, y, w] = v[..] {
                    self.with_view(cx, ids!(fractal), |cx, view| {
                        view.start_view = Some((x, y, w));
                        view.reset_view();
                        view.redraw(cx);
                    });
                }
            }
            if a == "--type" {
                let i: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                self.params.kind = FractalKind::ALL[i.min(FractalKind::ALL.len() - 1)];
                self.ui.drop_down(cx, ids!(kind)).set_selected_item(cx, i);
                self.push_params(cx);
            }
            if a == "--show-workers" {
                self.ui.check_box(cx, ids!(show_workers)).set_active(cx, true, Animate::No);
                self.with_view(cx, ids!(fractal), |_, v| v.show_workers = true);
            }
            if a == "--place" {
                let i: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                self.go_to(cx, &PLACES[i.min(PLACES.len() - 1)]);
            }
            if a == "--tour" {
                let places: Vec<(f64, f64, f64)> = TOUR.iter().map(|i| PLACES[*i].view).collect();
                self.with_view(cx, ids!(fractal), |cx, v| v.start_tour(cx, places));
                self.set_tour_label(cx, true);
            }
            if a == "--palette" {
                let i: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                self.look.palette = i.min(PALETTES.len() - 1);
                if let Some(mut g) = self.ui.widget(cx, ids!(palettes)).borrow_mut::<PaletteGrid>() {
                    g.selected = self.look.palette;
                }
                self.push_look(cx);
            }
            if a == "--look" {
                // --look MAPPING DENSITY EDGE GLOW: colouring presets for comparisons
                let v: Vec<f64> = (0..4).filter_map(|_| args.next().and_then(|s| s.parse().ok())).collect();
                if let [m, d, e, g] = v[..] {
                    self.look.mapping = m as u32;
                    self.look.density = d;
                    self.look.de_strength = e;
                    self.look.glow = g;
                    self.push_look(cx);
                }
            }
            if a == "--trap" {
                self.params.trap = true;
                self.ui.check_box(cx, ids!(trap)).set_active(cx, true, Animate::No);
                self.push_params(cx);
            }
            if a == "--local" {
                self.ui.check_box(cx, ids!(network)).set_active(cx, false, Animate::No);
                self.with_view(cx, ids!(fractal), |cx, v| v.set_network(cx, false));
            }
            if a == "--threads" {
                let n: Option<usize> = args.next().and_then(|s| s.parse().ok());
                self.with_view(cx, ids!(fractal), |_, v| v.threads = n.filter(|n| *n > 0));
            }
            if a == "--bench" {
                let runs = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
                self.with_view(cx, ids!(fractal), |cx, v| v.start_bench(cx, Bench::new(runs)));
            }
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut params = false;
        let mut look = false;
        if let Some(i) = self.ui.drop_down(cx, ids!(kind)).selected(actions) {
            self.params.kind = FractalKind::ALL[i.min(FractalKind::ALL.len() - 1)];
            if self.params.kind == FractalKind::Multibrot && self.params.power < 3 {
                self.params.power = 3;
            }
            params = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(power)).slided(actions) {
            self.params.power = v.round() as u32;
            params = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(iter)).slided(actions) {
            self.base_iter = v.round() as u32;
            self.params.max_iter = self.base_iter;
            params = true;
            look = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(auto_iter)).changed(actions) {
            self.auto_iter = b;
            self.params.max_iter = self.base_iter;
            params = true;
        }
        if let Some(i) = self.ui.drop_down(cx, ids!(quality)).selected(actions) {
            let q = [Quality::Draft, Quality::Full, Quality::Supersampled][i.min(2)];
            self.with_view(cx, ids!(fractal), |cx, v| v.set_quality(cx, q));
        }
        for action in actions {
            if let PaletteGridAction::Selected(i) = action.as_widget_action().cast::<PaletteGridAction>() {
                self.look.palette = i;
                look = true;
            }
            match action.as_widget_action().cast::<FractalViewAction>() {
                FractalViewAction::Picked(x, y) => {
                    self.params.julia = (x, y);
                    params = true;
                }
                FractalViewAction::TourStopped => self.set_tour_label(cx, false),
                FractalViewAction::None => {}
            }
        }
        if let Some(v) = self.ui.slider(cx, ids!(density)).slided(actions) {
            self.look.density = v;
            look = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(offset)).slided(actions) {
            self.look.offset = v;
            look = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(cycle)).slided(actions) {
            self.look.cycle = v;
            look = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(hue)).slided(actions) {
            self.look.hue = v;
            look = true;
        }
        if let Some(i) = self.ui.drop_down(cx, ids!(mapping)).selected(actions) {
            self.look.mapping = i as u32;
            look = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(de)).changed(actions) {
            self.params.de = b;
            params = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(de_strength)).slided(actions) {
            self.look.de_strength = v;
            look = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(glow)).slided(actions) {
            self.look.glow = v;
            look = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(trap)).changed(actions) {
            self.params.trap = b;
            params = true;
        }
        let place_ids = [ids!(place0), ids!(place1), ids!(place2), ids!(place3), ids!(place4), ids!(place5), ids!(place6), ids!(place7), ids!(place8), ids!(place9)];
        for (i, id) in place_ids.iter().enumerate() {
            if self.ui.button(cx, *id).clicked(actions) {
                self.with_view(cx, ids!(fractal), |_, v| v.stop_tour());
                self.set_tour_label(cx, false);
                self.go_to(cx, &PLACES[i]);
            }
        }
        if self.ui.button(cx, ids!(tour)).clicked(actions) {
            self.toggle_tour(cx);
        }
        if let Some(i) = self.ui.drop_down(cx, ids!(inside)).selected(actions) {
            self.look.inside = INSIDE[i.min(2)];
            look = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(network)).changed(actions) {
            self.with_view(cx, ids!(fractal), |cx, v| v.set_network(cx, b));
        }
        if let Some(b) = self.ui.check_box(cx, ids!(show_workers)).changed(actions) {
            self.with_view(cx, ids!(fractal), |cx, v| {
                v.show_workers = b;
                v.redraw(cx);
            });
        }
        if self.ui.button(cx, ids!(reset)).clicked(actions) {
            self.with_view(cx, ids!(fractal), |cx, v| {
                v.reset_view();
                v.redraw(cx);
            });
        }
        if params {
            self.push_params(cx);
        }
        if look {
            // the cycling offset lives in the view: keep it unless the slider moved
            if let Some(o) = self.with_view(cx, ids!(fractal), |_, v| v.look.offset) {
                if self.ui.slider(cx, ids!(offset)).slided(actions).is_none() {
                    self.look.offset = o;
                }
            }
            self.push_look(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        crate::palette::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::NextFrame(_) = event {
            if self.auto_iter {
                self.push_params(cx);
            }
            self.update_stats(cx);
            if let Some(report) = self.with_view(cx, ids!(fractal), |_, v| v.bench_report()).flatten() {
                print!("{report}");
                log!("{report}");
                cx.quit();
            }
        }
    }
}
