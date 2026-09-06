pub use makepad_widgets::desktop_style::DesktopStyle;
use makepad_widgets::*;

#[derive(Clone, Debug)]
pub struct StyleTween {
    pub target: DesktopStyle,
    pub dark: bool,
    pub weights: [f64; 7],
    from: [f64; 7],
    elapsed: f64,
}
impl Default for StyleTween {
    fn default() -> Self {
        Self {
            target: DesktopStyle::Omarchy,
            dark: false,
            weights: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            from: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            elapsed: 1.0,
        }
    }
}
impl StyleTween {
    pub fn select(&mut self, style: DesktopStyle) {
        self.from = self.weights;
        self.target = style;
        self.elapsed = 0.0;
    }
    pub fn step(&mut self, dt: f64) -> bool {
        self.elapsed = (self.elapsed + dt / 0.65).min(1.0);
        let t = self.elapsed * self.elapsed * (3.0 - 2.0 * self.elapsed);
        for i in 0..self.weights.len() {
            self.weights[i] = self.from[i]
                + ((if i == self.target as usize { 1.0 } else { 0.0 }) - self.from[i]) * t;
        }
        self.elapsed < 1.0
    }
    pub fn active(&self) -> bool {
        self.elapsed < 1.0
    }
    // Desktop-only geometry arrays have no contribution in phone modes.
    pub fn value<const N: usize>(&self, values: [f64; N]) -> f64 {
        self.weights.iter().zip(values).map(|(w, v)| w * v).sum()
    }
    pub fn reserved_height(&self) -> f64 {
        self.value([0.0, 0.0, 54.0, 34.0, 0.0])
    }
    pub fn title_height(&self) -> f64 {
        self.value([0.0, 32.0, 34.0, 20.0, 22.0])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_switch_starts_from_the_visible_mix_and_lands_exactly() {
        let mut t = StyleTween::default();
        t.select(DesktopStyle::Macos);
        t.step(0.2);
        let old = t.weights;
        t.select(DesktopStyle::Windows2000);
        t.step(0.0);
        assert_eq!(t.weights, old);
        t.step(1.0);
        assert_eq!(t.weights, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    }
}

use crate::desk::WmState;
use crate::hub::ClientId;
use crate::shell::{
    alpha, rgb,
    ui::{rect, HAlign, Ico, ShellDraw},
};
use makepad_widgets::app_icon::AppIconDraw;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum MacCaption {
    #[pick]
    None = 0,
    Close = 1,
    Minimize = 2,
    Maximize = 3,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let MacCaption = set_type_default() do #(MacCaption::script_api(vm))
    set_type_default() do #(DrawDesktopChrome::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #eeeeee
        radius: 6.0
        bevel: 0.0
        pressed: 0.0
        selected: 0.0
        top_only: 0.0
        title_gradient: 0.0
        title_gradient_end: #a6caf0
        frame_width: 0.0
        caption: MacCaption.None
        caption_ink: #0000
        pixel: fn() {
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            // The content quad has hard, pixel-aligned edges. Extend the
            // title's SDF by half a device pixel so its straight edges and
            // bottom junction cover the same pixels without a dark AA seam.
            let bleed = if self.top_only > 0.5 {0.5 / self.draw_pass.dpi_factor} else {0.0}
            sdf.box_y(-bleed, -bleed, self.rect_size.x+2.0*bleed, self.rect_size.y+2.0*bleed, self.radius*0.5, self.radius*0.5*(1.0-self.top_only))
            if self.frame_width > 0.0 {
                sdf.rect(self.frame_width, self.frame_width, self.rect_size.x - 2.0*self.frame_width, self.rect_size.y - 2.0*self.frame_width)
                sdf.subtract()
            }
            let pixel=floor(p*self.draw_pass.dpi_factor)
            let checker=modf(pixel.x+pixel.y,2.0)
            let fill=mix(self.color.rgb,vec3(1.0),self.selected*checker)
            let base=mix(fill,self.title_gradient_end.rgb,self.pos.x*self.title_gradient)
            let tl=min(p.x,p.y)
            let br=min(self.rect_size.x-p.x,self.rect_size.y-p.y)
            let outer=if tl<br {mix(vec3(0.831,0.816,0.784),vec3(0.502),self.pressed)}else{mix(vec3(0.251),vec3(1.0),self.pressed)}
            let inner=if tl<br {mix(vec3(1.0),vec3(0.251),self.pressed)}else{mix(vec3(0.502),vec3(0.831,0.816,0.784),self.pressed)}
            let distance=min(tl,br)
            let edge=if distance<1.0 {outer}else{inner}
            let color=mix(base,edge,(1.0-step(2.0,distance))*self.bevel)
            // Window titles are rectangular content inside the complete
            // window's compositor mask. A second SDF coverage ramp here
            // exposes the wallpaper along the join with the app surface.
            if self.top_only > 0.5 && self.radius == 0.0 {
                return vec4(color*self.color.w,self.color.w)
            }
            sdf.fill(vec4(color,self.color.w))
            let c = self.rect_size * 0.5
            match self.caption {
                MacCaption.Close => {
                    sdf.move_to(c.x-2.8,c.y-2.8)
                    sdf.line_to(c.x+2.8,c.y+2.8)
                    sdf.move_to(c.x-2.8,c.y+2.8)
                    sdf.line_to(c.x+2.8,c.y-2.8)
                    sdf.stroke(self.caption_ink,1.35)
                }
                MacCaption.Minimize => {
                    sdf.move_to(c.x-3.25,c.y)
                    sdf.line_to(c.x+3.25,c.y)
                    sdf.stroke(self.caption_ink,1.5)
                }
                MacCaption.Maximize => {
                    sdf.move_to(c.x-3.1,c.y-0.9)
                    sdf.line_to(c.x+0.9,c.y+3.1)
                    sdf.line_to(c.x-3.1,c.y+3.1)
                    sdf.close_path()
                    sdf.fill(self.caption_ink)
                    sdf.move_to(c.x+3.1,c.y+0.9)
                    sdf.line_to(c.x-0.9,c.y-3.1)
                    sdf.line_to(c.x+3.1,c.y-3.1)
                    sdf.close_path()
                    sdf.fill(self.caption_ink)
                }
                _ => {}
            }
            return sdf.result
        }
    }
    mod.widgets.DesktopShelfBase = #(DesktopShelf::register_widget(vm))
    mod.widgets.DesktopShelf = set_type_default() do mod.widgets.DesktopShelfBase {
        width: Fill height: Fill
        d +: {text.text_style: theme.font_regular text_bold.text_style: theme.font_bold}
        chrome +: {}
        glass: GlassPanel {
            width: Fill height: Fill
            draw_bg +: {
                blur_level: 4.5
                corner_radius: 12.0
                lensing_strength: 0.0
                diffraction_strength: 0.0
                tint_color: #dddded
                tint_alpha: 0.22
                surface_alpha: 0.88
                specular_strength: 0.025
                border_alpha: 0.20
                border_width: 0.65
                shadow_radius: 10.0
                shadow_color: #0005
            }
        }
    }
}
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDesktopChrome {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub radius: f32,
    #[live]
    pub bevel: f32,
    #[live]
    pub pressed: f32,
    #[live]
    pub selected: f32,
    #[live]
    pub top_only: f32,
    #[live]
    pub title_gradient: f32,
    #[live]
    pub title_gradient_end: Vec4f,
    #[live]
    pub frame_width: f32,
    #[live]
    pub caption: MacCaption,
    #[live]
    pub caption_ink: Vec4f,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ShelfHit {
    Launcher,
    App(String),
    Window(ClientId),
    ShowDesktop,
}
#[derive(Script, ScriptHook, Widget)]
pub struct DesktopShelf {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    chrome: DrawDesktopChrome,
    #[rust]
    app_icons: AppIconDraw,
    #[rust]
    window_apps: HashMap<ClientId, String>,
    #[rust]
    active_window: Option<ClientId>,
    #[live]
    d: ShellDraw,
    #[live]
    glass: WidgetRef,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    hits: Vec<(Rect, ShelfHit)>,
    #[rust]
    bounds: Rect,
    #[rust]
    overlay: Option<DrawList2d>,
    #[rust]
    hover: Option<ShelfHit>,
    #[rust]
    dark: bool,
    #[rust]
    hover_mix: Vec<(ShelfHit, f64)>,
    #[rust]
    hover_frame: NextFrame,
    #[rust]
    hover_time: f64,
}
impl DesktopShelf {
    pub fn hit(&self, p: Vec2d) -> Option<ShelfHit> {
        self.hits
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h.clone())
    }
    pub fn contains(&self, p: Vec2d) -> bool {
        self.bounds.contains(p) && self.bounds.size.y > 0.0
    }
    pub fn hover_at(&mut self, cx: &mut Cx, p: Vec2d) {
        let h = self.hit(p);
        if h != self.hover {
            self.hover = h;
            self.hover_time = 0.0;
            self.hover_frame = cx.new_next_frame();
            self.redraw(cx);
        }
    }
    fn button(
        &mut self,
        cx: &mut Cx2d,
        r: Rect,
        hit: ShelfHit,
        _icon: Ico,
        label: &str,
        running: bool,
        style: DesktopStyle,
        opacity: f32,
    ) {
        let hover = self.hover.as_ref() == Some(&hit);
        let amount = self
            .hover_mix
            .iter()
            .find(|(h, _)| h == &hit)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        let mac = style == DesktopStyle::Macos;
        let classic = style == DesktopStyle::Windows2000;
        let selected = classic && matches!(&hit, ShelfHit::Window(c) if Some(*c) == self.active_window);
        let inset = if selected { 1.0 } else { 0.0 };
        self.chrome.pressed = if selected { 1.0 } else { 0.0 };
        self.chrome.selected = self.chrome.pressed;
        let ink = alpha(
            if style.supports_dark() && self.dark {
                rgb(240, 240, 245)
            } else {
                rgb(28, 30, 36)
            },
            opacity,
        );
        if !mac && style != DesktopStyle::NextStep && (classic || hover) {
            self.chrome.radius = if classic { 0.0 } else { 5.0 };
            self.chrome.bevel = if classic { 1.0 } else { 0.0 };
            self.chrome.color = alpha(
                if classic {
                    rgb(212, 208, 200)
                } else {
                    rgb(255, 255, 255)
                },
                opacity * if classic { 1.0 } else { 0.4 },
            );
            self.chrome.draw_abs(cx, r);
        }
        let icon_rect = if classic {
            rect(r.pos.x + 4.0 + inset, r.pos.y + inset, 22.0, r.size.y)
        } else {
            rect(
                r.pos.x,
                r.pos.y - if mac { 7.0 * amount } else { 0.0 },
                r.size.x,
                if mac { 50.0 } else { r.size.y },
            )
        };
        let app_id = match &hit {
            ShelfHit::App(app) => app.as_str(),
            ShelfHit::Window(client) => self
                .window_apps
                .get(client)
                .map(String::as_str)
                .unwrap_or("app"),
            ShelfHit::Launcher => "applications",
            _ => "app",
        };
        let size = if style == DesktopStyle::NextStep { r.size.x.min(r.size.y) } else if mac {
            54.0 + 8.0 * amount
        } else if classic {
            16.0
        } else {
            26.0
        };
        let icon_box = if mac {
            mac_icon_box(r, amount)
        } else {
            rect(
                icon_rect.pos.x + (icon_rect.size.x - size) * 0.5,
                icon_rect.pos.y + (icon_rect.size.y - size) * 0.5,
                size,
                size,
            )
        };
        self.app_icons
            .draw(cx, app_id, style, icon_box, opacity, ink);
        if classic {
            self.d.label_elided(
                cx,
                rect(
                    r.pos.x + 28.0 + inset,
                    r.pos.y + inset,
                    (r.size.x - 32.0).max(1.0),
                    r.size.y,
                ),
                selected,
                12.0,
                ink,
                HAlign::Left,
                label,
            );
        }
        if running && !classic && style != DesktopStyle::NextStep {
            self.chrome.radius = 2.0;
            self.chrome.bevel = 0.0;
            self.chrome.color = if mac {
                alpha(rgb(220, 220, 230), opacity * 0.82)
            } else {
                ink
            };
            self.chrome.draw_abs(
                cx,
                rect(
                    r.pos.x + r.size.x * 0.5 - 1.8,
                    if mac { mac_icon_box(r, 0.0).pos.y + 59.0 } else { r.pos.y + r.size.y - 4.0 },
                    3.6,
                    3.6,
                ),
            );
        }
        if hover && !classic && style != DesktopStyle::NextStep {
            let width = (label.len() as f64 * 7.0 + 22.0).max(60.0);
            let tip = rect(
                r.pos.x + (r.size.x - width) * 0.5,
                r.pos.y - 35.0,
                width,
                25.0,
            );
            self.chrome.color = alpha(
                if style.supports_dark() && self.dark {
                    rgb(48, 48, 52)
                } else {
                    rgb(240, 240, 244)
                },
                opacity,
            );
            self.chrome.radius = 6.0;
            self.chrome.bevel = 0.0;
            self.chrome.draw_abs(cx, tip);
            self.d
                .label(cx, tip, false, 12.0, ink, HAlign::Center, label);
        }
        self.hits.push((r, hit));
    }
}
pub fn app_icon(id: &str) -> Ico {
    match id {
        "browser" => Ico::Globe,
        "photos" | "image" => Ico::Photo,
        "terminal" => Ico::Keyboard,
        "files" => Ico::Menu,
        "sheets" => Ico::Calendar,
        "task" => Ico::Pulse,
        "video" => Ico::Play,
        "pdf" => Ico::Check,
        "mixer" => Ico::Speaker,
        "vj" => Ico::Headphone,
        "score" => Ico::Bell,
        "route" => Ico::Globe,
        "fabric" => Ico::Shirt,
        "fab" => Ico::Refresh,
        "studio" => Ico::Moon,
        _ => Ico::Monitor,
    }
}
fn shelf_geometry(screen: Rect, style: &StyleTween, app_count: usize) -> Rect {
    let dock_width = (((app_count + 1) as f64) * 62.0 + 20.0).min((screen.size.x - 24.0).max(1.0));
    let next_height = ((app_count + 1) as f64 * 56.0).min((screen.size.y - 48.0).max(1.0));
    rect(
        screen.pos.x + style.value([8.0, (screen.size.x - dock_width) * 0.5, 0.0, 0.0, screen.size.x - 64.0]),
        screen.pos.y + style.value([0.0, screen.size.y - 88.0, screen.size.y - 54.0, screen.size.y - 34.0, 40.0]),
        style.value([32.0, dock_width, screen.size.x, screen.size.x, 56.0]),
        style.value([0.0, 78.0, 54.0, 34.0, next_height]),
    )
}

/// Center the icon and its running indicator as one group inside each dock cell.
fn mac_icon_box(cell: Rect, hover: f64) -> Rect {
    let size = 54.0 + 8.0 * hover;
    let top = cell.pos.y + (cell.size.y - 63.0) * 0.5;
    rect(cell.pos.x + (cell.size.x-size)*0.5, top - 8.0*hover, size, size)
}

/// The compositor samples exactly the shelf it will paint this frame, including
/// an interrupted style tween. Glass outside the dock cannot add a blur stack.
fn dock_app_ids(state: &WmState) -> Vec<String> {
    let mut apps: Vec<_> = crate::shell::launcher::apps()
        .into_iter().filter(|app| !app.disabled).map(|app| app.id).collect();
    for client in state.layout.clients_on(state.layout.active) {
        if let Some(client) = state.clients.get(&client) {
            let id=format!("apps.{}",client.app);
            if !apps.contains(&id) {apps.push(id);}
        }
    }
    apps
}
pub fn dock_bounds(state: &WmState, size: Vec2d) -> Rect {
    shelf_geometry(rect(0.0,0.0,size.x,size.y), &state.style, dock_app_ids(state).len())
}
pub fn dock_icon_bounds(state: &WmState, size: Vec2d, app: &str) -> Rect {
    let apps=dock_app_ids(state);
    let dock=shelf_geometry(rect(0.0,0.0,size.x,size.y), &state.style, apps.len());
    let slot=apps.iter().position(|id| id==&format!("apps.{app}")).map(|i|i+1).unwrap_or(0);
    let cell=(dock.size.x-20.0)/(apps.len()+1) as f64;
    mac_icon_box(rect(dock.pos.x+10.0+slot as f64*cell,dock.pos.y+6.0,cell,dock.size.y-12.0), 0.0)
}

impl Widget for DesktopShelf {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let screen = cx.turtle().rect();
        self.hits.clear();
        self.bounds = Rect::default();
        if let Some(state) = scope.data.get_mut::<WmState>() {
            self.active_window = state.layout.focused_client()
                .filter(|c| !state.layout.desktop.minimized(*c));
            self.window_apps = state
                .clients
                .iter()
                .map(|(c, s)| (*c, s.app.clone()))
                .collect();
            let t = &state.style;
            let style = t.target;
            let dark = style.supports_dark() && t.dark;
            if self.dark != dark {
                self.dark = dark;
                let tint = if dark {
                    rgb(24, 24, 28)
                } else {
                    rgb(221, 221, 237)
                };
                let tint_alpha = if dark { 0.32 } else { 0.20 };
                script_apply_eval!(cx,self.glass,{draw_bg +: {tint_color: #(tint) tint_alpha: #(tint_alpha)}});
            }
            let opacity = (1.0 - t.weights[0]) as f32;
            if opacity > 0.001 && !style.mobile() {
                let mut apps: Vec<_> = crate::shell::launcher::apps()
                    .into_iter()
                    .filter(|a| !a.disabled)
                    .collect();
                let clients: Vec<_> = state
                    .layout
                    .clients_on(state.layout.active)
                    .into_iter()
                    .filter_map(|c| {
                        state
                            .clients
                            .get(&c)
                            .map(|s| (c, s.app.clone(), s.display_title().to_string()))
                    })
                    .collect();
                for (_, app, title) in &clients {
                    if !apps.iter().any(|item| item.id == format!("apps.{app}")) {
                        apps.push(crate::shell::menu::MenuItem {
                            id: format!("apps.{app}"),
                            label: title.clone(),
                            icon: Some(app_icon(app)),
                            kind: crate::shell::menu::MenuKind::App,
                            checked: false,
                            disabled: false,
                            description: String::new(),
                            aliases: vec![app.clone()],
                        });
                    }
                }
                let n = (apps.len() + 1).max(1) as f64;
                let r = shelf_geometry(screen, t, apps.len());
                let (x, y, w, h) = (r.pos.x, r.pos.y, r.size.x, r.size.y);
                self.bounds = r;
                // Window-backed Gaussian blur, sampled from the live desktop.
                if t.weights[1] > 0.01 {
                    if let Some(mut glass) = self.glass.borrow_mut::<gauss_view::GaussRoundedView>()
                    {
                        glass.draw_surface_with_backdrop(
                            cx,
                            r,
                            state.dock_backdrop.clone(),
                            t.weights[1] as f32,
                        );
                    }
                }
                // Glass redirects drawing through its backdrop pass; keep its
                // foreground in a separate list. Opaque shelves stay in the
                // scene's recording so closing a child cannot detach them.
                let glass_foreground = t.weights[1] > 0.001;
                if glass_foreground {
                    if self.overlay.is_none() {
                        self.overlay = Some(DrawList2d::new(cx));
                    }
                    self.overlay.as_mut().unwrap().begin_always(cx);
                }
                self.chrome.new_draw_call(cx);
                self.chrome.pressed = 0.0;
                self.chrome.selected = 0.0;
                self.chrome.color = alpha(
                    if style == DesktopStyle::Windows2000 {
                        rgb(212, 208, 200)
                    } else if dark {
                        rgb(32, 32, 32)
                    } else {
                        rgb(234, 238, 245)
                    },
                    opacity * (1.0 - t.weights[1]) as f32,
                );
                self.chrome.radius = t.value([0.0, 18.0, 0.0, 0.0, 0.0]) as f32;
                self.chrome.bevel = t.weights[3] as f32;
                self.chrome.draw_abs(cx, r);
                for style in [
                    DesktopStyle::Macos,
                    DesktopStyle::Windows,
                    DesktopStyle::Windows2000,
                    DesktopStyle::NextStep,
                ] {
                    let opacity = t.weights[style as usize] as f32;
                    if opacity < 0.001 {
                        continue;
                    }
                    let hit_start = self.hits.len();
                    if style == DesktopStyle::NextStep {
                        let cell = h / n;
                        self.button(cx, rect(x,y,w,cell), ShelfHit::Launcher, Ico::Menu, "Workspace", false, style, opacity);
                        for (i, app) in apps.iter().enumerate() {
                            let id = app.id.trim_start_matches("apps.");
                            self.button(cx, rect(x,y+(i+1) as f64*cell,w,cell), ShelfHit::App(id.into()), app_icon(id), &app.label, clients.iter().any(|(_,a,_)| a==id), style, opacity);
                        }
                    } else if style == DesktopStyle::Windows2000 {
                        self.button(
                            cx,
                            rect(x + 3.0, y + 3.0, 76.0, (h - 6.0).max(1.0)),
                            ShelfHit::Launcher,
                            Ico::Menu,
                            "Start",
                            false,
                            style,
                            opacity,
                        );
                        let available = (w - 126.0).max(1.0);
                        let cell = (available / clients.len().max(1) as f64).min(180.0);
                        for (i, (c, app, title)) in clients.iter().enumerate() {
                            self.button(
                                cx,
                                rect(
                                    x + 86.0 + i as f64 * cell,
                                    y + 3.0,
                                    cell - 3.0,
                                    (h - 6.0).max(1.0),
                                ),
                                ShelfHit::Window(*c),
                                app_icon(app),
                                title,
                                !state.layout.desktop.minimized(*c),
                                style,
                                opacity,
                            );
                        }
                    } else {
                        let cell = if style == DesktopStyle::Macos {
                            (w - 20.0) / n
                        } else {
                            48.0_f64.min((w - 36.0) / n)
                        };
                        let start = if style == DesktopStyle::Macos {
                            x + 10.0
                        } else {
                            x + (w - cell * n) * 0.5
                        };
                        self.button(
                            cx,
                            rect(start, y + 6.0, cell, (h - 12.0).max(1.0)),
                            ShelfHit::Launcher,
                            Ico::Menu,
                            if style == DesktopStyle::Macos {
                                "Applications"
                            } else {
                                "Start"
                            },
                            false,
                            style,
                            opacity,
                        );
                        for (i, app) in apps.iter().enumerate() {
                            let id = app.id.trim_start_matches("apps.");
                            self.button(
                                cx,
                                rect(
                                    start + (i + 1) as f64 * cell,
                                    y + 6.0,
                                    cell,
                                    (h - 12.0).max(1.0),
                                ),
                                ShelfHit::App(id.into()),
                                app_icon(id),
                                &app.label,
                                clients.iter().any(|(_, a, _)| a == id),
                                style,
                                opacity,
                            );
                        }
                    }
                    if style != DesktopStyle::Macos && style != DesktopStyle::NextStep {
                        let button = rect(x + w - 22.0, y + 3.0, 19.0, (h - 6.0).max(1.0));
                        self.hits.push((button, ShelfHit::ShowDesktop));
                        self.d.solid(
                            cx,
                            rect(button.pos.x, button.pos.y, 1.0, button.size.y),
                            alpha(rgb(128, 128, 128), opacity),
                        );
                    }
                    if style != t.target {
                        self.hits.truncate(hit_start);
                    }
                }
                if glass_foreground { self.overlay.as_mut().unwrap().end(cx); }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.hover_frame.is_event(event) {
            if let Some(hit) = &self.hover {
                if !self.hover_mix.iter().any(|(h, _)| h == hit) {
                    self.hover_mix.push((hit.clone(), 0.0));
                }
            }
            let dt = if self.hover_time == 0.0 {
                1.0 / 60.0
            } else {
                (ne.time - self.hover_time).min(0.05)
            };
            self.hover_time = ne.time;
            let mut active = false;
            for (hit, value) in &mut self.hover_mix {
                let target = if Some(&*hit) == self.hover.as_ref() {
                    1.0
                } else {
                    0.0
                };
                *value += (target - *value) * (1.0 - (-dt * 20.0).exp());
                active |= (target - *value).abs() > 0.001;
            }
            self.hover_mix
                .retain(|(hit, value)| *value > 0.001 || Some(hit) == self.hover.as_ref());
            if active {
                self.hover_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
    }
}
