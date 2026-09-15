//! A tile hosting an in-process module instance (aicontrol.md §3).
//!
//! Where `MpRunView` presents a child process's swapchain, this widget
//! draws an instance's ROOT — a widget minted inside the instance's own
//! splash isolate by `module_host.rs` — as a subtree of the desk, at the
//! rect the layout gives the tile. Pointer events reach the root as they
//! reach any widget (coordinates stay absolute: `Area` hit-testing is
//! `Cx`-absolute); keys reach it only while the window manager's focus is
//! on this tile, which is the gate a process tile gets from its swapchain
//! forwarding and a module tile has to make explicit. A press inside the
//! tile tells the WM to focus it, the way a press on a process tile does.
//!
//! The root is set by the host after `create` and cleared BEFORE the
//! instance's isolate is freed: the tile outlives the instance by its
//! close animation, and a widget whose heap is gone must not be drawn.
//!
//! The instance draws into a texture of its own (`WindowFrame`): its root,
//! every list it lifts as an overlay, the gauss pyramid those may ask for
//! — all inside that texture's pass tree, nothing in the window's overlay
//! slot — and the tile presents the texture. On the phone the desk is
//! already recording the tile into such a texture (a card, a home tile,
//! the full screen); then the instance draws straight into that one.
//!
//! The tile is also the fault line. A root that panics — in an event, in
//! its draw — is caught HERE, not by the platform's last-resort catcher:
//! the isolate scope and the script VM come back on their own on the way
//! up (`with_isolate`, `with_vm`), the tile cuts the draw stacks back to
//! where its own draw stood, lets go of the root, shows "crashed" in the
//! instance's place and tells the WM, which tears the instance down in
//! the host's order. The desk, the other tiles and the phone shell never
//! see the panic.

use crate::desk::phone::DrawPhoneApp;
use crate::dock_warp::WindowFrame;
use crate::hub::ClientId;
use crate::run_view::MpRunViewAction;
use crate::tile::TileHost;
use makepad_widgets::gauss_view::CaptureGauss;
use makepad_widgets::widget_async::with_isolate;
use makepad_widgets::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpModuleViewBase = #(MpModuleView::register_widget(vm))

    mod.widgets.MpModuleView = set_type_default() do mod.widgets.MpModuleViewBase {
        width: Fill
        height: Fill
        // The instance's ground: the theme's window background, so a root
        // that paints only its own chrome still sits on the desk's colour.
        draw_bg +: { color: mod.wm_theme.background }
        // What a tile says in the instance's place once its root panicked;
        // its ink is chosen against the instance's ground at draw time.
        draw_crash +: {
            text_style: theme.font_regular
            text_style.font_size: 15
        }
    }
}

/// What a panic said, for the log and the WM's action.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpModuleView {
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
    draw_bg: DrawColor,
    /// The instance's texture, presented by this quad — on a desk that is
    /// not already recording the tile into a texture of the host's.
    #[live]
    draw_capture: DrawPhoneApp,
    #[live]
    draw_crash: DrawText,
    /// The root panicked, with what it said: the tile shows "crashed" in
    /// its place until the WM has torn the instance down.
    #[rust]
    crashed: Option<String>,
    #[rust]
    frame: Option<WindowFrame>,
    #[rust]
    root: Option<WidgetRef>,
    #[rust]
    client: Option<ClientId>,
    /// The isolate the root was minted in: installed on `Cx` around every
    /// draw and every event the root sees, so its lazily made children,
    /// first-draw shader compiles and callbacks resolve in their own heap.
    #[rust]
    vm_id: SplashVmId,
    #[rust]
    area: Area,
    /// FOCUS RULE (see `MpRunView`): a preview never takes the keyboard.
    #[rust(true)]
    takes_key_focus: bool,
    /// The WM's focus is on this tile: keys reach the root.
    #[rust]
    focused: bool,
    /// The root has drawn at least once.
    #[rust]
    drawn: bool,
    /// The popin fade the desk drives; a module tile has no frozen frame to
    /// fade, so the ground follows it and the root draws solid.
    #[rust(1.0f32)]
    fade: f32,
}

impl MpModuleView {
    /// Seat an instance's root here. The WM's view of the client is set
    /// with it so a press can name the tile to focus.
    pub fn set_root(&mut self, cx: &mut Cx, client: ClientId, vm_id: SplashVmId, root: WidgetRef) {
        cx.widget_tree_insert_child(self.uid, live_id!(root), root.clone());
        self.root = Some(root);
        self.client = Some(client);
        self.vm_id = vm_id;
        self.drawn = false;
        self.crashed = None;
        self.draw_bg.redraw(cx);
    }

    /// What the root's panic said, once it has.
    pub fn crashed(&self) -> Option<&str> {
        self.crashed.as_deref()
    }

    /// The root panicked in `what` (an event, its draw): let go of it —
    /// nothing draws or dispatches to it again — and tell the WM, which
    /// tears the instance down in the host's order.
    fn crash(&mut self, cx: &mut Cx, what: &str, payload: Box<dyn std::any::Any + Send>) {
        let message = panic_message(&*payload);
        error!("wm: module instance for client {:?} panicked in {what}: {message}", self.client);
        self.root = None;
        self.focused = false;
        self.crashed = Some(message.clone());
        if let Some(client) = self.client {
            cx.widget_action(self.uid, MpRunViewAction::Crashed { client, message });
        }
        self.draw_bg.redraw(cx);
    }

    /// Drop the root — called by the host right before the instance's
    /// isolate is freed. The tile keeps drawing its ground through the
    /// close animation, nothing else.
    pub fn clear_root(&mut self, cx: &mut Cx) {
        self.root = None;
        self.focused = false;
        self.draw_bg.redraw(cx);
    }

    pub fn root(&self) -> Option<WidgetRef> {
        self.root.clone()
    }

    /// "crashed", centred in the tile, in whichever ink reads on the
    /// instance's ground (its theme's window colour, light or dark).
    fn draw_crashed(&mut self, cx: &mut Cx2d, rect: Rect) {
        if self.crashed.is_none() {
            return;
        }
        let ground = self.draw_bg.color;
        let luminance = 0.299 * ground.x + 0.587 * ground.y + 0.114 * ground.z;
        self.draw_crash.color = if luminance > 0.5 { vec4(0.11, 0.11, 0.12, 1.0) } else { vec4(0.94, 0.94, 0.96, 1.0) };
        cx.begin_turtle(Walk::abs_rect(rect), Layout { align: Align { x: 0.5, y: 0.5 }, ..Layout::flow_down() });
        self.draw_crash.draw_walk(cx, Walk::fit(), Align::default(), "crashed");
        cx.end_turtle();
    }
}

impl TileHost for MpModuleView {
    fn client(&self) -> Option<ClientId> {
        self.client
    }

    fn set_status_line(&mut self, _cx: &mut Cx, _line: &str) {
        // A module has no build, no exec scan, no stdout: nothing to show.
    }

    /// The WM's focus lands here: keys may reach the root from now on.
    /// The widget INSIDE that holds the keyboard is the root's own affair
    /// — a cell the person clicked, a text field — so this never moves
    /// the key focus itself (a process tile must, to forward keys; a
    /// module's widgets are in this very tree and claim it themselves).
    fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        if !self.takes_key_focus {
            return true;
        }
        if !self.area.is_valid(cx) {
            return false;
        }
        self.focused = true;
        true
    }

    fn release_keyboard(&mut self, cx: &mut Cx) {
        self.focused = false;
        // Whatever inside held the keyboard must let go too, or a field in
        // a tile behind the pane would keep eating keys.
        cx.set_key_focus(Area::Empty);
    }

    fn set_takes_key_focus(&mut self, on: bool) {
        self.takes_key_focus = on;
    }

    fn set_remote_cursor(&mut self, _cx: &mut Cx, _cursor: MouseCursor) {}

    fn has_frame(&self) -> bool {
        self.drawn
    }

    fn arrival_fade(&self) -> f32 {
        1.0
    }

    fn set_target_size(&mut self, _size: Option<Vec2d>) {}

    fn set_close_crop(&mut self, _crop: Option<(Vec2d, Vec2d)>) {}

    fn set_fade(&mut self, fade: f32) {
        self.fade = fade;
    }
}

impl Widget for MpModuleView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // Keys only while the WM focus is here: a text field inside a tile
        // in the background must not eat what the person types elsewhere.
        if matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_)) && !self.focused {
            return;
        }
        if let Event::MouseDown(e) = event {
            if self.area.is_valid(cx) && self.area.rect(cx).contains(e.abs) {
                if let Some(client) = self.client {
                    // The WM moves focus here (and back to us through
                    // `focus_keyboard`), exactly as for a process tile.
                    cx.widget_action(self.uid, MpRunViewAction::Clicked { client });
                }
            }
        }
        let vm_id = self.vm_id;
        let handled = catch_unwind(AssertUnwindSafe(|| {
            with_isolate(cx, vm_id, |cx| root.handle_event(cx, event, scope))
        }));
        if let Err(payload) = handled {
            self.crash(cx, "an event", payload);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        // The ground is painted first, at z 0, into a pass that has a depth
        // buffer — and an instance that orders its own ink in depth draws
        // BELOW z 0 too (the map's ground at -50, its tilted tiles around
        // -24). A ground that wrote depth would win the LessEqual test
        // against all of that and show nothing but itself: paint order is
        // its whole claim, so it never writes depth.
        self.draw_bg.draw_vars.options.depth_write = false;
        let Some(root) = self.root.clone() else {
            self.draw_bg.draw_abs(cx, rect);
            self.draw_crashed(cx, rect);
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };
        // The ground is the instance's OWN background — its theme's
        // `color_bg_app`, what its own window would clear to — never the
        // desk's colour: a light-theme app that leaves its ground showing
        // put its dark ink on the desk's dark ground (the clock's title).
        if let Some(ground) = cx.cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let theme = vm.module(id!(theme));
            vm.bx.heap.value(theme, id!(color_bg_app).into(), NoTrap).as_color()
        }) {
            self.draw_bg.color = vec4(((ground >> 24) & 0xff) as f32 / 255.0, ((ground >> 16) & 0xff) as f32 / 255.0, ((ground >> 8) & 0xff) as f32 / 255.0, 1.0);
        }
        // Inside a host capture the texture being recorded IS the
        // instance's; otherwise the tile opens the instance's own.
        let own = !CaptureGauss::inside_capture(cx) && rect.size.x >= 1.0 && rect.size.y >= 1.0;
        if own {
            let frame = self.frame.get_or_insert_with(|| WindowFrame::new_with_name(cx, "wm_module_tile"));
            frame.begin(cx, rect);
        }
        self.draw_bg.draw_abs(cx, rect);
        // The root draws under a catch: what its panic leaves open on the
        // draw context — turtles, lists, passes, an overlay, a capture —
        // is cut back to here, so the frame the tile began still ends.
        let mark = cx.unwind_mark();
        let captures = CaptureGauss::scope_depth(cx);
        let vm_id = self.vm_id;
        let drawn = catch_unwind(AssertUnwindSafe(|| {
            with_isolate(cx, vm_id, |cx| {
                root.draw_walk_all(cx, scope, if own { Walk::abs_rect(rect) } else { Walk::fill() })
            })
        }));
        if let Err(payload) = drawn {
            cx.unwind_to(mark);
            CaptureGauss::unwind_scope_to(cx, captures);
            self.crash(cx, "its draw", payload);
            self.draw_crashed(cx, rect);
        }
        self.drawn = true;
        if own {
            let frame = self.frame.as_mut().unwrap();
            frame.end(cx);
            self.draw_capture.draw_vars.set_texture(0, frame.texture());
            self.draw_capture.opacity = self.fade;
            self.draw_capture.y_flip = if matches!(cx.os_type(), OsType::Android(_)) { 1.0 } else { 0.0 };
            self.draw_capture.draw_abs(cx, rect);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
