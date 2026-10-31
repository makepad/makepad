//! XR in a makepad app, end to end: the app streams its XR view to a headset.
//!
//! - The caption's `XrCaptionButton` (makepad-xr-stream) starts the app's XR server and pairs a
//!   headset by its PIN. A linked headset gives the process an XR display.
//! - `XrDemo` (below) draws the XR pass: a table in the room with a few blocks on it (mixed
//!   reality, passthrough around them). The shared navigation (makepad-xr-nav) moves, turns and
//!   scales the table with both grips in empty space.
//! - The headset UI (makepad-xr-ui): the wrist menu (Y, or the left palm up) with the app's rows,
//!   a card over the table about the selected block, grab-to-edit (a grip on a block carries it;
//!   letting go is one edit and one undo step in the app's document) and the standard haptic
//!   ticks (hover, press, grab, a snap every 5 cm, a limit at the table's edge).
//! - `XrMirrorView` shows the headset's left eye on the desktop.

pub use makepad_widgets;

use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_widgets::*;
use makepad_xr_nav::{ClipBox, XrNav, XrNavConfig, XrNavMode};
use makepad_xr_ui::input::{grip_pose, Buttons};
use makepad_xr_ui::{haptic, Card, EditSink, GrabEdits, Haptics, Height, MenuStep, Tone, Width, XrPanel, XrWristMenu};

app_main!(App);

/// The table top's centre in the room (m), in front of the first head pose.
const TABLE: [f32; 3] = [0.0, 0.9, -0.6];
/// The table top's half size (m).
const TABLE_HALF: [f32; 2] = [0.3, 0.2];
/// A block's half size (m).
const BLOCK: f32 = 0.03;
/// How near a hand must be to a block to grab it (m, in the world).
const REACH: f32 = 0.05;
/// A carried block ticks every this far (m).
const SNAP: f32 = 0.05;

const CONFIG: XrNavConfig = XrNavConfig {
    mode: XrNavMode::Free,
    box_start: ClipBox { center: Vec3f { x: TABLE[0], y: TABLE[1], z: TABLE[2] }, ..XrNavConfig::FOLLOW.box_start },
    scale_start: 1.0,
    anchor: Vec3f { x: 0.0, y: 0.0, z: 0.0 },
    ..XrNavConfig::FOLLOW
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrDemoBase = #(XrDemo::register_widget(vm))
    mod.widgets.XrDemo = set_type_default() do mod.widgets.XrDemoBase{
        width: 1 height: 1
        draw_block +: {depth_write: true light_dir: vec3(0.35, 0.8, 0.45)}
        menu: mod.widgets.XrWristMenu{}
        info: mod.widgets.XrPanel{height: 128.0 meters: 0.14}
        draw_tick +: {color: #0000}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 640)
                caption_bar +: {
                    caption_label +: {
                        flow: Right align: Align{y: 0.5}
                        View{width: Fill height: 1}
                        label +: {text: "Makepad XR"}
                        View{width: Fill height: 1}
                        // the headset: its state, PIN pairing, the stream's numbers and settings
                        xr_caption := XrCaptionButton{app: "xr-example" margin: Inset{right: 8}}
                        // the headset's view (never seen on the desktop)
                        xr_demo := XrDemo{}
                    }
                }
                body +: {
                    flow: Down spacing: 8 padding: 12
                    doc_label := Label{text: "" draw_text +: {text_style +: {font_size: 11}}}
                    Label{
                        width: Fill
                        text: "Pair a headset from the caption's headset icon. In the headset: grip a block to carry it (one undo step), both grips in empty space move the table, Y or the left palm up opens the wrist menu."
                        draw_text +: {color: theme.color_text_meta}
                    }
                    mirror := XrMirrorView{}
                }
            }
        }
    }
}

/// The document the headset edits: the blocks on the table (world metres, the table top's
/// centre at the origin), with an undo history.
pub struct Doc {
    blocks: Vec<Vec3f>,
    undo: Vec<Move>,
    redo: Vec<Move>,
    /// Bumped by every change (the job that carries an edit).
    version: u64,
}

impl Default for Doc {
    fn default() -> Self {
        let at = |x: f32, z: f32| vec3f(x, BLOCK, z);
        Doc { blocks: vec![at(-0.12, 0.0), at(0.0, 0.05), at(0.12, -0.04)], undo: Vec::new(), redo: Vec::new(), version: 0 }
    }
}

/// One edit: a block moved.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    block: usize,
    from: Vec3f,
    to: Vec3f,
}

impl Doc {
    fn apply(&mut self, m: Move, at: Vec3f) {
        if let Some(b) = self.blocks.get_mut(m.block) {
            *b = at;
        }
        self.version += 1;
    }

    fn undo(&mut self) {
        if let Some(m) = self.undo.pop() {
            self.apply(m, m.from);
            self.redo.push(m);
        }
    }

    fn redo(&mut self) {
        if let Some(m) = self.redo.pop() {
            self.apply(m, m.to);
            self.undo.push(m);
        }
    }
}

/// The headset's edits go straight into the document (an app with a worker would send them
/// there and answer [`GrabEdits::sent`] when it has the job).
impl EditSink<Move> for Doc {
    fn commit(&mut self, m: Move) {
        self.apply(m, m.to);
        self.undo.push(m);
        self.redo.clear();
    }
}

/// The wrist menu's cells.
mod item {
    pub const UNDO: u32 = 0;
    pub const REDO: u32 = 1;
    pub const REAL_SIZE: u32 = 2;
    pub const SNAP: u32 = 3;
}

/// A hand carrying a block.
#[derive(Clone, Copy)]
struct Hold {
    block: usize,
    /// The block's centre minus the hand's (world) at the grab.
    offset: Vec3f,
    from: Vec3f,
    /// The 5 cm cell it was in (the snap tick), and whether it was at the table's edge.
    cell: [i32; 3],
    at_edge: bool,
}

struct XrPass {
    pass: DrawPass,
    root: DrawList,
    _color: Texture,
    _depth: Texture,
    nav: XrNav,
    stepped_at: Option<f64>,
    nav_state: Box<XrState>,
    head: Vec3f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrDemo {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_block: DrawCube,
    #[live]
    menu: XrWristMenu,
    #[live]
    info: XrPanel,
    #[redraw]
    #[live]
    draw_tick: DrawColor,
    #[rust]
    xr: Option<XrPass>,
    #[rust]
    buttons: Buttons,
    #[rust]
    menu_step: MenuStep,
    #[rust]
    holds: [Option<Hold>; 2],
    #[rust]
    grip_down: [bool; 2],
    /// Where a carried block is shown until the document has its move.
    #[rust]
    shown: Vec<Option<Vec3f>>,
    #[rust]
    edits: GrabEdits,
    #[rust]
    selected: Option<usize>,
    #[rust(true)]
    snap_ticks: bool,
}

impl XrDemo {
    fn activate(&mut self, cx: &mut Cx) {
        let color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        let depth = Texture::new_with_format(cx, TextureFormat::DepthD32Sampled { size: TextureSize::Auto, initial: true });
        let pass = DrawPass::new_with_name(cx, "xr example");
        // premultiplied on nothing: the room shows wherever the table is not
        pass.set_color_texture(cx, &color, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        // the XR server takes this pass's attachments at its end
        pass.set_as_xr_pass(cx);
        cx.xr_start_presenting();
        cx.xr_set_see_through(true);
        let nav = XrNav::new(CONFIG);
        self.xr = Some(XrPass { pass, root: DrawList::new(cx), _color: color, _depth: depth, nav, stepped_at: None, nav_state: Box::default(), head: Vec3f::default() });
    }

    /// Where a block is (a carried one where the hand has it).
    fn block_at(&self, doc: &Doc, i: usize) -> Vec3f {
        self.shown.get(i).copied().flatten().unwrap_or(doc.blocks[i])
    }

    fn fill_menu(&mut self, doc: &Doc) {
        let key = doc.version ^ ((doc.undo.len() as u64) << 32) ^ ((doc.redo.len() as u64) << 48) ^ ((self.snap_ticks as u64) << 63);
        self.menu.panel.set(key, |rows| {
            rows.row(Height::Fixed(52.0))
                .button(item::UNDO, "Undo", Width::FILL)
                .button(item::REDO, "Redo", Width::FILL)
                .button(item::REAL_SIZE, "1:1", Width::FILL);
            rows.row(Height::Fixed(52.0)).toggle(item::SNAP, "Snap ticks", self.snap_ticks, Width::FILL);
            rows.row(Height::Fixed(52.0)).heading(format_args!("{} edit(s), {} to redo", doc.undo.len(), doc.redo.len()), Tone::Meta, Width::FILL);
            rows.row(Height::Fill).text("Grip a block to carry it", Tone::Normal, Width::FILL);
            rows.row(Height::Fill).text("Both grips: move the table", Tone::Meta, Width::FILL);
        });
    }

    fn fill_info(&mut self, doc: &Doc) {
        let Some(i) = self.selected else { return };
        let p = self.block_at(doc, i);
        let waiting = self.edits.waiting(i);
        let key = (i as u64) ^ ((p.x.to_bits() as u64) << 8) ^ ((p.z.to_bits() as u64) << 32) ^ ((waiting as u64) << 63);
        self.info.set(key, |rows| {
            rows.row(Height::Fixed(52.0)).text(format_args!("Block {}", i + 1), Tone::Normal, Width::FILL);
            rows.row(Height::Fixed(52.0)).text(format_args!("x {:+.0} mm  z {:+.0} mm", p.x * 1000.0, p.z * 1000.0), if waiting { Tone::Warning } else { Tone::Good }, Width::FILL);
        });
    }

    /// One headset frame: the menu, the grabs, the navigation with what the grabs leave.
    fn step(&mut self, cx: &mut Cx, input: &XrState, doc: &mut Doc) {
        let now = Cx::time_now();
        let xr = self.xr.as_mut().unwrap();
        let dt = xr.stepped_at.map_or(0.0, |t| (now - t) as f32);
        if xr.stepped_at.is_none() {
            // the table in front of the first head pose
            let head = &input.head_pose;
            let f = head.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
            let flat = vec3f(f.x, 0.0, f.z);
            let f = if flat.length() > 1e-4 { flat.normalize() } else { vec3f(0.0, 0.0, -1.0) };
            xr.nav.clip.center = vec3f(head.position.x + f.x * -TABLE[2], TABLE[1], head.position.z + f.z * -TABLE[2]);
            xr.nav.center_on(Vec3f::default());
        }
        xr.stepped_at = Some(now);
        xr.head = input.head_pose.position;
        let view = xr.nav.view();
        let mut haptics = Haptics::default();

        self.buttons.step(input);
        self.fill_menu(doc);
        self.menu_step = self.menu.step(input, &self.buttons, now);
        haptics.merge(self.menu_step.haptics);
        match self.menu_step.pressed {
            Some(item::UNDO) => doc.undo(),
            Some(item::REDO) => doc.redo(),
            Some(item::REAL_SIZE) => self.xr.as_mut().unwrap().nav.set_scale(1.0),
            Some(item::SNAP) => self.snap_ticks = !self.snap_ticks,
            _ => {}
        }
        if self.buttons.hit(1, XrController::CLICK_B) {
            doc.undo();
        }

        // the grips: a block near the hand is carried; letting go commits its move
        self.shown.resize(doc.blocks.len(), None);
        self.edits.resize(doc.blocks.len());
        let ctrl = [&input.left_controller, &input.right_controller];
        let hand = [&input.left_hand, &input.right_hand];
        for i in 0..2 {
            let grip = grip_pose(ctrl[i], hand[i], self.grip_down[i]);
            let (pressed, released) = (grip.is_some() && !self.grip_down[i], grip.is_none() && self.grip_down[i]);
            self.grip_down[i] = grip.is_some();
            if released {
                if let Some(h) = self.holds[i].take() {
                    let to = self.block_at(doc, h.block);
                    if (to - h.from).length() > 1e-4 {
                        // one gesture, one edit, one undo step
                        self.edits.commit(vec![h.block], Move { block: h.block, from: h.from, to }, doc);
                        self.edits.sent(doc.version);
                    } else {
                        self.shown[h.block] = None;
                    }
                }
            }
            if pressed {
                let at = view.world_from_room(grip.unwrap().position);
                let other = self.holds[1 - i].map(|h| h.block);
                let near = (0..doc.blocks.len())
                    .filter(|&b| Some(b) != other && !self.edits.waiting(b))
                    .map(|b| (b, (self.block_at(doc, b) - at).length()))
                    .filter(|(_, d)| *d <= REACH + BLOCK)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((b, _)) = near {
                    let from = self.block_at(doc, b);
                    self.holds[i] = Some(Hold { block: b, offset: from - at, from, cell: cell_of(from), at_edge: false });
                    self.selected = Some(b);
                    haptics.add(i, haptic::GRAB);
                }
            }
            if let (Some(h), Some(p)) = (self.holds[i].as_mut(), grip) {
                let want = view.world_from_room(p.position) + h.offset;
                // on the table: inside its top, never under it
                let lim = [TABLE_HALF[0] - BLOCK, TABLE_HALF[1] - BLOCK];
                let to = vec3f(want.x.clamp(-lim[0], lim[0]), want.y.max(BLOCK), want.z.clamp(-lim[1], lim[1]));
                let at_edge = to.x != want.x || to.z != want.z;
                if at_edge && !h.at_edge {
                    haptics.add(i, haptic::LIMIT);
                }
                h.at_edge = at_edge;
                let cell = cell_of(to);
                if cell != h.cell && self.snap_ticks {
                    haptics.add(i, haptic::SNAP);
                }
                h.cell = cell;
                self.shown[h.block] = Some(to);
            }
        }
        // the document has every move up to its version: settled blocks show where it says
        let (holds, shown) = (&self.holds, &mut self.shown);
        self.edits.settle(doc.version, |b| holds.iter().flatten().any(|h| h.block == b), |b| shown[b] = None);

        // the navigation sees the grips the blocks left
        let xr = self.xr.as_mut().unwrap();
        let nav_in: &XrState = if self.holds.iter().all(|h| h.is_none()) {
            input
        } else {
            *xr.nav_state = input.clone();
            let s = &mut *xr.nav_state;
            for (i, (c, h)) in [(&mut s.left_controller, &mut s.left_hand), (&mut s.right_controller, &mut s.right_hand)].into_iter().enumerate() {
                if self.holds[i].is_some() {
                    c.grip = 0.0;
                    h.flags &= !XrHand::PINCH_MIDDLE;
                }
            }
            &xr.nav_state
        };
        xr.nav.update(nav_in, dt);
        haptics.play(cx);
    }

    fn draw_xr(&mut self, cx: &mut Cx2d, doc: &Doc) {
        self.fill_menu(doc);
        self.fill_info(doc);
        let (view, head) = self.xr.as_ref().map(|xr| (xr.nav.view(), xr.head)).unwrap();
        let transform = view.matrix();
        // the card floats over the selected block, facing the head
        let info_card = self.selected.map(|i| {
            let p = view.room_from_world(self.block_at(doc, i));
            Card::facing(p + vec3f(0.0, 0.09, 0.0), head, self.info.size())
        });
        let xr = self.xr.as_mut().unwrap();
        cx.begin_pass(&xr.pass, None);
        xr.root.begin_always(cx);
        // the cards' textures: child passes, redrawn only when what they show changed
        self.menu.render(cx);
        if info_card.is_some() {
            self.info.render(cx);
        }
        let cx3d = &mut Cx3d::new(cx.cx);
        cx3d.begin_scene_3d(SceneState3D::default());
        self.draw_block.transform = transform;
        self.draw_block.color = vec4(0.22, 0.24, 0.28, 1.0);
        self.draw_block.cube_pos = vec3f(0.0, -0.01, 0.0);
        self.draw_block.cube_size = vec3f(TABLE_HALF[0] * 2.0, 0.02, TABLE_HALF[1] * 2.0);
        self.draw_block.draw(cx3d);
        for i in 0..doc.blocks.len() {
            let held = self.holds.iter().flatten().any(|h| h.block == i);
            self.draw_block.color = match (held, self.selected == Some(i)) {
                (true, _) => vec4(0.95, 0.75, 0.3, 1.0),
                (false, true) => vec4(0.45, 0.65, 0.95, 1.0),
                _ => vec4(0.75, 0.77, 0.8, 1.0),
            };
            self.draw_block.cube_pos = self.block_at(doc, i);
            self.draw_block.cube_size = vec3f(BLOCK * 2.0, BLOCK * 2.0, BLOCK * 2.0);
            self.draw_block.draw(cx3d);
        }
        self.menu.draw(cx3d);
        if let Some(card) = &info_card {
            self.info.draw(cx3d, card);
        }
        cx3d.end_scene_3d();
        let xr = self.xr.as_mut().unwrap();
        xr.root.end(cx);
        cx.end_pass(&xr.pass);
    }
}

fn cell_of(p: Vec3f) -> [i32; 3] {
    [(p.x / SNAP).floor() as i32, (p.y / SNAP).floor() as i32, (p.z / SNAP).floor() as i32]
}

impl Widget for XrDemo {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // a linking headset can deliver its first pose before the next desktop draw
        if self.xr.is_none() && cx.xr_display() {
            self.activate(cx);
        }
        let (Some(_), Event::XrUpdate(update)) = (self.xr.as_ref(), event) else { return };
        let Some(doc) = scope.data.get_mut::<Doc>() else { return };
        let version = doc.version;
        self.step(cx, &update.state, doc);
        if doc.version != version {
            cx.action(XrDemoAction::DocChanged);
        }
        self.draw_tick.redraw(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.xr.is_none() && cx.cx.xr_display() {
            self.activate(cx.cx);
        }
        if self.xr.is_some() {
            if let Some(doc) = scope.data.get::<Doc>() {
                self.draw_xr(cx, doc);
            }
        }
        self.draw_tick.draw_walk(cx, walk);
        DrawStep::done()
    }
}

#[derive(Clone, Debug, Default)]
pub enum XrDemoAction {
    DocChanged,
    #[default]
    None,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    doc: Doc,
}

impl App {
    fn show_doc(&mut self, cx: &mut Cx) {
        let d = &self.doc;
        let text = format!("{} blocks, {} edit(s) to undo, {} to redo", d.blocks.len(), d.undo.len(), d.redo.len());
        self.ui.label(cx, ids!(doc_label)).set_text(cx, &text);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.show_doc(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if actions.iter().any(|a| matches!(a.downcast_ref::<XrDemoAction>(), Some(XrDemoAction::DocChanged))) {
            self.show_doc(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        // the caption control, the server's frame sink and the mirror
        makepad_xr_stream::script_mod(vm);
        makepad_xr_stream::serve_script_mod(vm);
        // the wrist menu, the panels and the card shader
        makepad_xr_ui::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.doc));
    }
}
