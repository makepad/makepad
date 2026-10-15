//! The visuals pop-out: a second, plain window showing the screen's picture
//! (the spectrum or MilkDrop) edge to edge, for a projector or an
//! external display. It has no chrome and no style: no bezel, no screen
//! filter, no colour map, the picture's own colours. It can be resized,
//! maximised and made fullscreen (a double-click or F; Esc leaves it), and
//! where it was (position, size, screen, fullscreen) is kept in
//! `<data>/popout.txt`, so it opens there again.
//!
//! Nothing renders twice. MilkDrop renders once, at the pop-out's
//! resolution while it is open (large enough to cover the in-app screen as
//! well), into the texture the in-app screen already showed; the screen shows
//! it cropped to its shape and keeps lighting the device from it, and the
//! pop-out blits the same texture. The producing passes are children of the
//! main window's pass, so they render before both windows in the same frame;
//! the pop-out repaints exactly when the screen's picture does, so a paused
//! or silent picture repaints neither window. The spectrum is only quads: the
//! pop-out draws its own, following the same sound.
//!
//! The window is an independent `Window` drawn and fed by the app (no second
//! root); it drags from anywhere by its Window's `drag: WindowDrag.Window`, and a double click
//! makes it fullscreen on its screen and back (`double_click: WindowDoubleClick.Fullscreen`).

use crate::layout::HwStageWidgetRefExt;
use crate::App;
use makepad_milkdrop::shaders::DrawMilkBlit;
use makepad_milkdrop::view::cover_uv;
use makepad_milkdrop::MilkdropViewWidgetRefExt;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PopoutPictureBase = #(PopoutPicture::register_widget(vm))
    // The picture in the pop-out: a texture the screen rendered, raw.
    mod.widgets.PopoutPicture = set_type_default() do mod.widgets.PopoutPictureBase{
        width: Fill
        height: Fill
    }
}

/// The size a first pop-out opens at, in points.
const FIRST_SIZE: DVec2 = dvec2(1280.0, 720.0);

/// Which picture the pop-out shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Spectrum,
    Milk,
}

/// Where the window was: position and inner size in points, fullscreen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    position: DVec2,
    size: DVec2,
    fullscreen: bool,
}

impl Place {
    fn parse(text: &str) -> Option<Self> {
        let mut values = text.split_whitespace().map(|v| v.parse::<f64>());
        let mut next = || values.next().and_then(|v| v.ok()).filter(|v| v.is_finite());
        let (x, y, w, h) = (next()?, next()?, next()?, next()?);
        let fullscreen = next().unwrap_or(0.0) > 0.5;
        (w >= 64.0 && h >= 64.0).then_some(Self { position: dvec2(x, y), size: dvec2(w, h), fullscreen })
    }

    fn serialize(&self) -> String {
        format!("{} {} {} {} {}\n", self.position.x, self.position.y, self.size.x, self.size.y, self.fullscreen as u8)
    }
}

fn place_path() -> Option<std::path::PathBuf> {
    Some(crate::worker::data_dir()?.join("popout.txt"))
}

/// What the app keeps for the pop-out.
#[derive(Default)]
pub struct Popout {
    window: Option<WidgetRef>,
    /// Asked to close; kept until the platform says it closed.
    closing: bool,
    /// The window has key focus.
    focused: bool,
    /// It has drawn once (its draw lists exist).
    drawn: bool,
    /// Its pixels (inner size times its pixel density).
    pixels: Option<DVec2>,
    place: Option<Place>,
    source: Option<Source>,
    /// The render size handed to the screen's picture.
    render: Option<(Source, DVec2)>,
}

impl Popout {
    fn window_id(&self) -> Option<WindowId> {
        self.window.as_ref()?.as_window().window_id()
    }

    fn is_open(&self) -> bool {
        self.window.is_some() && !self.closing
    }
}

impl App {
    fn popout_source(&self) -> Source {
        if self.milk.settings.on {
            Source::Milk
        } else {
            Source::Spectrum
        }
    }

    /// Open the pop-out, or close it.
    pub(crate) fn toggle_popout(&mut self, cx: &mut Cx) {
        if self.popout.window.is_some() {
            self.close_popout(cx);
        } else {
            self.open_popout(cx);
        }
    }

    fn open_popout(&mut self, cx: &mut Cx) {
        if self.popout.place.is_none() {
            self.popout.place = place_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| Place::parse(&t));
        }
        let window = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Window{
                    window.title: "Amp Visuals"
                    // No caption bar: the picture and nothing else. The whole
                    // window drags (WindowDragQuery answers Caption).
                    show_caption_bar: false
                    // Chromeless in every state, under a window manager too; it drags
                    // from anywhere, and a double click makes it fullscreen on its screen
                    // (over bars and dock) and again restores it (the Window's own).
                    chrome: WindowChrome.None
                    drag: WindowDrag.Window
                    double_click: WindowDoubleClick.Fullscreen
                    pass +: {clear_color: #x000000}
                    body +: {
                        width: Fill
                        height: Fill
                        flow: Overlay
                        popout_spectrum := Visualizer{visible: false}
                        popout_picture := PopoutPicture{}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        // Where it was last, before the platform creates it.
        if let Some(id) = window.as_window().window_id() {
            if cx.windows.is_valid(id) {
                let place = self.popout.place;
                let w = &mut cx.windows[id];
                w.create_inner_size = Some(place.map(|p| p.size).unwrap_or(FIRST_SIZE));
                if let Some(place) = place {
                    w.create_position = Some(place.position);
                    w.is_fullscreen = place.fullscreen;
                }
            }
        }
        self.popout.window = Some(window);
        self.popout.closing = false;
        self.popout.focused = false;
        self.popout.drawn = false;
        self.popout.pixels = None;
        self.popout.source = None;
        self.popout_sync(cx);
        self.popout_key_light(cx);
        log!("amp: visuals pop-out opened");
    }

    fn close_popout(&mut self, cx: &mut Cx) {
        if self.popout.window.is_none() || self.popout.closing {
            return;
        }
        self.popout.closing = true;
        if let Some(window) = &self.popout.window {
            window.as_window().close(cx);
        }
        self.popout_release(cx);
        self.popout_key_light(cx);
    }

    /// The screen's picture goes back to its own size.
    fn popout_release(&mut self, cx: &mut Cx) {
        self.popout.pixels = None;
        self.popout_render(cx);
    }

    /// The PARTY key's light (its dot) follows the window.
    fn popout_key_light(&mut self, cx: &mut Cx) {
        let lit = if self.popout.is_open() { 1.0 } else { 0.0 };
        self.ui.relief_view(cx, ids!(popout_cap)).set_lit(cx, lit);
    }

    /// The render size of the screen's picture: the pop-out's, scaled up
    /// to also cover the in-app screen.
    fn popout_render(&mut self, cx: &mut Cx) {
        let source = self.popout_source();
        let open = self.popout.is_open();
        let want = match source {
            Source::Milk if open => self.popout.pixels.map(|px| {
                let area = self.ui.widget(cx, ids!(milk)).area();
                let dpi = self.ui.window(cx, ids!(main_window)).window_id().filter(|id| cx.windows.is_valid(*id)).map(|id| cx.windows[id].effective_dpi_factor()).unwrap_or(1.0);
                cover(px, if area.is_valid(cx) { area.rect(cx).size * dpi } else { DVec2::default() })
            }),
            _ => None,
        };
        let now = want.map(|size| (source, dvec2(size.x.round(), size.y.round())));
        if self.popout.render == now {
            return;
        }
        let milk = match now {
            Some((Source::Milk, size)) => Some((size.x as usize, size.y as usize)),
            _ => None,
        };
        self.popout.render = now;
        self.ui.milkdrop_view(cx, ids!(milk)).set_render_pixels(cx, milk);
    }

    /// The picture shown and its render size, after a mode or size change.
    fn popout_sync(&mut self, cx: &mut Cx) {
        let Some(window) = self.popout.window.clone() else { return };
        let source = self.popout_source();
        if self.popout.source != Some(source) {
            self.popout.source = Some(source);
            let spectrum = window.widget(cx, ids!(popout_spectrum));
            spectrum.set_visible(cx, source == Source::Spectrum);
            let picture = window.widget(cx, ids!(popout_picture));
            picture.set_visible(cx, source != Source::Spectrum);
            window.redraw(cx);
        }
        self.popout_render(cx);
    }

    /// Keys for the pop-out while it has focus: F toggles fullscreen, Esc
    /// leaves it, the arrows step the visualisation. `true` when the key was
    /// the pop-out's.
    pub(crate) fn popout_key(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Event::KeyDown(key) = event else { return false };
        if !self.popout.focused || !self.popout.is_open() || key.modifiers.is_primary() {
            return false;
        }
        let Some(window) = self.popout.window.clone() else { return false };
        let window = window.as_window();
        match key.key_code {
            KeyCode::KeyF => {
                popout_fullscreen(cx, &window, None);
                true
            }
            KeyCode::Escape => {
                popout_fullscreen(cx, &window, Some(false));
                true
            }
            KeyCode::ArrowRight => {
                self.visual_step(cx, 1);
                true
            }
            KeyCode::ArrowLeft => {
                self.visual_step(cx, -1);
                true
            }
            _ => false,
        }
    }

    /// The key on the display wing.
    pub(crate) fn popout_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(popout_key)).clicked(actions) {
            self.toggle_popout(cx);
        }
    }

    /// The window's events, its drawing, and its repaints: after the main
    /// window has handled (and drawn) the event, so the picture it shows is
    /// the one the screen rendered this frame.
    pub(crate) fn popout_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::LiveEdit) {
            // A restyle put the key's light back to its template.
            self.popout_key_light(cx);
        }
        let main_id = self.ui.window(cx, ids!(main_window)).window_id();
        let Some(window) = self.popout.window.clone() else { return };
        let id = self.popout.window_id();
        match event {
            Event::WindowClosed(closed) if Some(closed.window_id) == id => {
                self.popout.window = None;
                self.popout.closing = false;
                self.popout.focused = false;
                self.popout_release(cx);
                self.popout_key_light(cx);
                log!("amp: visuals pop-out closed");
                return;
            }
            // The device closing takes its pop-out with it.
            Event::WindowCloseRequested(request) if Some(request.window_id) == main_id && request.accept_close.get() => {
                self.close_popout(cx);
            }
            Event::WindowGeomChange(change) if Some(change.window_id) == id => {
                let geom = &change.new_geom;
                if geom.inner_size.x > 0.0 && geom.inner_size.y > 0.0 {
                    self.popout.pixels = Some(geom.inner_size * geom.dpi_factor.max(0.5));
                    let place = Place {
                        // Fullscreen keeps the windowed place to come back to.
                        position: if geom.is_fullscreen { self.popout.place.map(|p| p.position).unwrap_or(geom.position) } else { geom.position },
                        size: if geom.is_fullscreen { self.popout.place.map(|p| p.size).unwrap_or(FIRST_SIZE) } else { geom.inner_size },
                        fullscreen: geom.is_fullscreen,
                    };
                    if self.popout.place != Some(place) {
                        self.popout.place = Some(place);
                        if let Some(path) = place_path() {
                            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
                            let _ = std::fs::write(path, place.serialize());
                        }
                    }
                }
                self.popout_render(cx);
                window.redraw(cx);
            }
            Event::WindowGotFocus(focus) => self.popout.focused = Some(*focus) == id,
            Event::WindowLostFocus(focus) if Some(*focus) == id => self.popout.focused = false,
            // A click says which window the keys go to as well.
            Event::MouseDown(down) => self.popout.focused = Some(down.window_id) == id,
            _ => {}
        }
        if self.popout.window.is_none() {
            return;
        }
        if let Event::Draw(draw) = event {
            self.popout_feed(cx);
            let created = id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created);
            let mut cx_draw = CxDraw::new(cx, draw);
            window.draw_all(&mut Cx2d::new(&mut cx_draw), &mut Scope::with_data(&mut self.model));
            if created && draw.redraw_all {
                self.popout.drawn = true;
            }
            return;
        }
        window.handle_event(cx, event, &mut Scope::with_data(&mut self.model));
        // A window's first draw: its lists do not exist yet, so a redraw of
        // it alone would not reach it.
        if !self.popout.drawn && id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created) {
            cx.redraw_all();
        }
        self.popout_sync(cx);
        // Repaint with the screen's picture: when its list draws again this
        // frame, the picture rendered again.
        let source = self.popout_source();
        if source != Source::Spectrum && !self.popout.closing {
            let area = self.ui.widget(cx, ids!(milk)).area();
            let redraws = cx.new_draw_event.redraw_all || area.draw_list_id().is_some_and(|list| cx.new_draw_event.draw_list_will_redraw(cx, list));
            if redraws {
                window.widget(cx, ids!(popout_picture)).redraw(cx);
            }
        }
    }

    /// Hand the pop-out the textures the screen drew with this frame.
    fn popout_feed(&mut self, cx: &mut Cx) {
        let Some(window) = self.popout.window.clone() else { return };
        let shown = match self.popout_source() {
            Source::Milk => self.ui.milkdrop_view(cx, ids!(milk)).shown(),
            Source::Spectrum => None,
        };
        // A device without a screen (the mini player, the widget) does not
        // draw the picture: the pop-out draws it instead.
        let guest = match self.popout_source() {
            Source::Spectrum => None,
            _ if self.ui.hw_stage(cx, ids!(stage)).shown("screen").is_some() => None,
            Source::Milk => Some(self.ui.widget(cx, ids!(milk))),
        };
        let widget = window.widget(cx, ids!(popout_picture));
        if let Some(mut picture) = widget.borrow_mut::<PopoutPicture>() {
            picture.shown = shown;
            picture.guest = guest;
        };
    }
}

/// Fullscreen on or off (`None`: the other way).
fn popout_fullscreen(cx: &mut Cx, window: &WindowRef, on: Option<bool>) {
    let was = window.is_fullscreen(cx);
    let on = on.unwrap_or(!was);
    if on == was {
        return;
    }
    log!("amp: visuals pop-out fullscreen {}", if on { "on" } else { "off" });
    // Maximize and restore, as a presentation window does: the ops every
    // desktop backend implements (macOS: native fullscreen, its own space).
    if on {
        window.maximize(cx);
    } else {
        window.restore(cx);
    }
}

/// `size` scaled up (keeping its shape) until it covers `other`.
fn cover(size: DVec2, other: DVec2) -> DVec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return size;
    }
    let k = (other.x / size.x).max(other.y / size.y).max(1.0);
    size * k
}

/// The pop-out's picture: the screen's texture, blitted raw.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct PopoutPicture {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_blit: DrawMilkBlit,
    #[live(true)]
    visible: bool,
    #[rust]
    area: Area,
    /// Pictures a and b, the mix toward b, their width over height.
    #[rust]
    shown: Option<(Texture, Texture, f32, f64)>,
    /// The screen's own picture widget, drawn here (a point, under the
    /// picture) while the device shows no screen, so it still renders.
    #[rust]
    guest: Option<WidgetRef>,
}

impl WidgetNode for PopoutPicture {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn visible(&self) -> bool {
        self.visible
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.area.redraw(cx);
        }
    }
}

impl Widget for PopoutPicture {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if let Some(guest) = &self.guest {
            let _ = guest.draw_walk(cx, scope, Walk::abs_rect(Rect { pos: rect.pos, size: dvec2(1.0, 1.0) }));
        }
        let Some((a, b, mix, shape)) = self.shown.clone() else { return DrawStep::done() };
        if rect.size.x < 1.0 || rect.size.y < 1.0 {
            return DrawStep::done();
        }
        let blit = &mut self.draw_blit;
        blit.draw_vars.set_texture(0, &a);
        blit.draw_vars.set_texture(1, &b);
        blit.draw_vars.set_uniform(cx, live_id!(u_mix), &[mix, 0.0, 0.0, 0.0]);
        blit.draw_vars.set_uniform(cx, live_id!(u_uv), &cover_uv(shape, rect.size.x / rect.size.y));
        blit.draw_abs(cx, rect);
        DrawStep::done()
    }
}
