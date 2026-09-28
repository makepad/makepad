//! The Material component's second page: the Material Bench's knob presets.
//!
//! Eighteen of the bench's knob styles, live, in the material the controls
//! pick, on that material's ground; the picked style large and in 3D beside
//! them, in the shape the geometry controls give it. The knobs are the
//! storybook's port of the bench's knob engine (`crate::knob`); this page is
//! its host: it holds the material and the shape the controls write, hands
//! the material to every knob and the shape to the large knob and the 3D
//! view, and picks the style a knob in the gallery is tapped on.
use crate::controls::{ControlValue, StoryControlAction};
use crate::knob::look::{color_of, ink_of, KnobLook, DEFAULT_STYLE, STYLE_INDEX, STYLE_PRESET, VALUE};
use crate::knob::presets::{style_shapes, Anchor, KnobMaterial, KnobShape, KnobStyle, STYLES};
use crate::knob::widgets::{set_material_uniforms, KnobView3dWidgetExt, TurnedKnobAction, TurnedKnobWidgetExt};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::knob::script_mod(vm);
    page::script_mod(vm)
}

mod page {
    use super::KnobPresets;
    use crate::makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets.*
        use mod.widgets.*
        use mod.storybook.*

        mod.storybook.KnobPresetsBase = #(KnobPresets::register_widget(vm))
        mod.storybook.KnobPresets = set_type_default() do mod.storybook.KnobPresetsBase{
            width: Fill
            height: Fill
        }

        // One gallery cell: a knob and its style's name under it.
        // A knob's light and shadow reach past its cell; no view between it
        // and the page clips them.
        let KnobCell = View{
            width: 136.
            height: Fit
            flow: Down
            clip_x: false
            clip_y: false
            align: Align{x: 0.5 y: 0.0}
            knob := TurnedKnob{width: 136. height: 128. fill: 0.5}
            name := Label{
                text: ""
                draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
            }
        }

        let PageNote = P{
            width: Fill
            text: ""
        }

        mod.stories.MaterialKnobPresets = mod.storybook.KnobPresets{
            flow: Down
            spacing: 14.
            padding: Inset{left: 24. right: 24. top: 20. bottom: 24.}
            scroll_bars: ScrollBars{
                show_scroll_x: false
                show_scroll_y: true
                scroll_bar_y.drag_scrolling: true
            }
            // The page is the material's ground, through the same exposure
            // and roll-off as the knobs, so their quads meet it seamlessly.
            show_bg: true
            draw_bg +: {..mod.storybook.KnobGroundFill}

            intro := PageNote{text: "The Material Bench's knob engine, ported: eighteen knob styles in thirteen materials. Every knob here is live -- drag one to turn them all, tap one to pick its style. The large knob and the 3D view show the picked style in the shape the geometry controls give it; drag the 3D view to orbit it, ctrl-scroll to zoom, double-tap to put the camera back."}
            stage := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                clip_x: false
                clip_y: false
                spacing: 16.
                align: Align{x: 0.0 y: 0.5}
                view3d := KnobView3d{width: 403. height: 290.}
                side := View{
                    width: 270.
                    height: Fit
                    flow: Down
                    clip_x: false
                    clip_y: false
                    spacing: 4.
                    align: Align{x: 0.5 y: 0.0}
                    big := TurnedKnob{width: 270. height: 270. fill: 0.62}
                    caption := Label{
                        text: ""
                        draw_text +: {text_style: theme.font_bold{font_size: 11}}
                    }
                }
            }
            gallery := View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                clip_x: false
                clip_y: false
                c0 := KnobCell{}
                c1 := KnobCell{}
                c2 := KnobCell{}
                c3 := KnobCell{}
                c4 := KnobCell{}
                c5 := KnobCell{}
                c6 := KnobCell{}
                c7 := KnobCell{}
                c8 := KnobCell{}
                c9 := KnobCell{}
                c10 := KnobCell{}
                c11 := KnobCell{}
                c12 := KnobCell{}
                c13 := KnobCell{}
                c14 := KnobCell{}
                c15 := KnobCell{}
                c16 := KnobCell{}
                c17 := KnobCell{}
            }
        }
    }
}

/// The page: the look its controls write (`crate::knob::look`), handed to
/// every knob and to the 3D view, and the shape they write, one live
/// property per control, handed to the large knob and the 3D view. A
/// material preset writes all the material's, a style preset all the
/// shape's.
#[derive(Script, Widget)]
pub struct KnobPresets {
    #[deref]
    view: View,
    #[live]
    look: KnobLook,
    // The shape: every number of a style, under the style's own names.
    #[live]
    capr: f64,
    #[live]
    cap_ink: Vec4f,
    #[live]
    spun: f64,
    #[live]
    flat: f64,
    #[live]
    wr0: f64,
    #[live]
    wr1: f64,
    #[live]
    wwmax: f64,
    #[live]
    barfil: f64,
    #[live]
    wendr: f64,
    #[live]
    wmode: f64,
    #[live]
    wbase: f64,
    #[live]
    arcr: f64,
    #[live]
    arcw: f64,
    #[live]
    awell: f64,
    #[live]
    well: f64,
    #[live]
    ticks: f64,
    #[live]
    tickr: f64,
    #[live]
    tickl: f64,
    #[live]
    tickw: f64,
    #[live]
    flutes: f64,
    #[live]
    fd: f64,
    #[live]
    fs: f64,
    #[live]
    gtaper: f64,
    #[live]
    ffrom: f64,
    #[live]
    fto: f64,
    #[live]
    ptype: f64,
    #[live]
    pr0: f64,
    #[live]
    pr1: f64,
    #[live]
    pw: f64,
    #[live]
    cut: f64,
    #[live]
    cn: f64,
    #[live]
    cr: f64,
    #[live]
    cs: f64,
    #[live]
    cl: f64,
    #[live]
    cf: f64,
    #[live]
    cw: f64,
    #[live]
    cfil: f64,
    #[live]
    csph: f64,
    #[live]
    cz: f64,
    /// The curve controls' writes, `[[x, y, kind], ...]` each, under the
    /// names in [`CURVES`]. Each is read into `curves` as it arrives and
    /// cleared (see `on_after_apply`).
    #[live(NIL)]
    prof: ScriptValue,
    #[live(NIL)]
    flute: ScriptValue,
    #[live(NIL)]
    wwid: ScriptValue,
    #[live(NIL)]
    whgt: ScriptValue,
    #[live(NIL)]
    wprof: ScriptValue,
    /// The shape's curves, in the order [`CURVES`] names them: the style's,
    /// and whatever a curve control or [`KnobPresets::set_curve`] writes
    /// over them.
    #[rust]
    curves: [Vec<Anchor>; 5],
    /// The style whose numbers and curves were last loaded whole. The style
    /// index moving away from it loads the new style whole.
    #[rust]
    loaded_style: usize,
    /// Whether the style index loaded a style the controls have not been
    /// told of yet: the next event pass tells them (see `handle_event`).
    #[rust]
    announce: bool,
    /// What the children were last handed, so a draw that changes nothing
    /// hands them nothing.
    #[rust]
    pushed: Option<(KnobMaterial, KnobShape, usize, f64, bool)>,
}

/// The shape's curves in the order [`KnobPresets::set_curve`] takes them:
/// the revolve profile, the grip's tooth, the wing's width along it, its
/// crest height along it, and its section across it.
pub const CURVES: [&str; 5] = ["prof", "flute", "wwid", "whgt", "wprof"];

impl KnobPresets {
    /// Load a style whole, its numbers and its curves, and make it the
    /// picked one.
    fn load_style(&mut self, index: usize) {
        let index = index.min(STYLES.len() - 1);
        let s = &STYLES[index];
        self.look.style = index as f64;
        self.loaded_style = index;
        self.capr = s.capr;
        self.cap_ink = color_of(s.cap_ink);
        self.spun = s.spun;
        self.flat = s.flat;
        self.wr0 = s.wr0;
        self.wr1 = s.wr1;
        self.wwmax = s.wwmax;
        self.barfil = s.barfil;
        self.wendr = s.wendr;
        self.wmode = s.wmode;
        self.wbase = s.wbase;
        self.arcr = s.arcr;
        self.arcw = s.arcw;
        self.awell = s.awell;
        self.well = s.well;
        self.ticks = s.ticks;
        self.tickr = s.tickr;
        self.tickl = s.tickl;
        self.tickw = s.tickw;
        self.flutes = s.flutes;
        self.fd = s.fd;
        self.fs = s.fs;
        self.gtaper = s.gtaper;
        self.ffrom = s.ffrom as f64;
        self.fto = s.fto as f64;
        self.ptype = s.ptype;
        self.pr0 = s.pr0;
        self.pr1 = s.pr1;
        self.pw = s.pw;
        self.cut = s.cut;
        self.cn = s.cn;
        self.cr = s.cr;
        self.cs = s.cs;
        self.cl = s.cl;
        self.cf = s.cf;
        self.cw = s.cw;
        self.cfil = s.cfil;
        self.csph = s.csph;
        self.cz = s.cz;
        self.curves = [s.prof.to_vec(), s.flute.to_vec(), s.wwid.to_vec(), s.whgt.to_vec(), s.wprof.to_vec()];
    }

    /// The shape the controls describe now, named after the picked style.
    pub fn shape(&self) -> KnobShape {
        let anchor_index = |v: f64| v.round().max(0.0) as usize;
        let [prof, flute, wwid, whgt, wprof] = self.curves.clone();
        KnobShape {
            name: STYLES[self.look.style_index()].name.to_string(),
            capr: self.capr,
            cap_ink: ink_of(self.cap_ink),
            spun: self.spun,
            flat: self.flat,
            wr0: self.wr0,
            wr1: self.wr1,
            wwmax: self.wwmax,
            barfil: self.barfil,
            wendr: self.wendr,
            wmode: self.wmode,
            wbase: self.wbase,
            wwid,
            whgt,
            wprof,
            arcr: self.arcr,
            arcw: self.arcw,
            awell: self.awell,
            well: self.well,
            ticks: self.ticks,
            tickr: self.tickr,
            tickl: self.tickl,
            tickw: self.tickw,
            prof,
            flutes: self.flutes,
            fd: self.fd,
            fs: self.fs,
            gtaper: self.gtaper,
            ffrom: anchor_index(self.ffrom),
            fto: anchor_index(self.fto),
            flute,
            ptype: self.ptype,
            pr0: self.pr0,
            pr1: self.pr1,
            pw: self.pw,
            cut: self.cut,
            cn: self.cn,
            cr: self.cr,
            cs: self.cs,
            cl: self.cl,
            cf: self.cf,
            cw: self.cw,
            cfil: self.cfil,
            csph: self.csph,
            cz: self.cz,
        }
    }

    /// Set curve `which` of the shape, in the order [`CURVES`] names them.
    /// A curve needs two anchors at least; fewer, or an index past the
    /// last curve, change nothing.
    pub fn set_curve(&mut self, cx: &mut Cx, which: usize, anchors: Vec<Anchor>) {
        if self.put_curve(which, anchors) {
            self.view.redraw(cx);
        }
    }

    /// [`KnobPresets::set_curve`] without the redraw: whether the curve
    /// changed.
    fn put_curve(&mut self, which: usize, anchors: Vec<Anchor>) -> bool {
        if anchors.len() < 2 {
            return false;
        }
        match self.curves.get_mut(which) {
            Some(curve) if *curve != anchors => {
                *curve = anchors;
                true
            }
            _ => false,
        }
    }

    fn cell(i: usize) -> LiveId {
        LiveId::from_str(&format!("c{i}"))
    }

    /// Tell the controls what the page shows: the style preset on the
    /// picked style, and every shape control on the shape's value, edits
    /// and all. The panel passes over a value it holds already, so what it
    /// is told again writes nothing back. The index is not told: a tap
    /// tells it itself, and any other move of it came from the panel, which
    /// holds it already; told back, a slider still being dragged would be
    /// pulled back a step.
    fn tell_controls(&self, cx: &mut Cx) {
        let uid = self.widget_uid();
        let preset = ControlValue::Choice(self.look.style_index());
        cx.widget_action(uid, StoryControlAction::Set { label: STYLE_PRESET, value: preset });
        for (label, value) in shape_values(&self.shape()) {
            cx.widget_action(uid, StoryControlAction::Set { label, value });
        }
    }

    /// Whether a shape is its style's as the preset has it.
    fn edited(shape: &KnobShape, style: usize) -> bool {
        shape.fingerprint() != style_shapes()[style].fingerprint()
    }

    /// Hand the material, the value and the picked style to every knob, the
    /// shape to the large knob and the 3D view, and colour the text to read
    /// on the ground.
    fn push(&mut self, cx: &mut Cx) {
        let m = self.look.material();
        let shape = self.shape();
        let style = self.look.style_index();
        let state = (m, shape.clone(), style, self.look.value, self.look.lit);
        if self.pushed.as_ref() == Some(&state) {
            return;
        }
        let first = self.pushed.is_none();
        let old = self.pushed.replace(state);
        let edited = Self::edited(&shape, style);
        let ground = crate::knob::bake::ink(m.ground);
        let luma = ground[0] * 0.2126 + ground[1] * 0.7152 + ground[2] * 0.0722;
        let text: Vec4f = if luma > 0.45 { vec4(0.14, 0.15, 0.18, 1.0) } else { vec4(0.86, 0.88, 0.91, 1.0) };
        let meta: Vec4f = if luma > 0.45 { vec4(0.30, 0.32, 0.37, 1.0) } else { vec4(0.62, 0.65, 0.70, 1.0) };
        // The picked style's name in the glow ink, where that reads on the
        // ground; a white glow on porcelain does not, and takes the text's.
        let glow = crate::knob::bake::ink(m.glow_ink);
        let glow_luma = glow[0] * 0.2126 + glow[1] * 0.7152 + glow[2] * 0.0722;
        let accent = if (glow_luma - luma).abs() > 0.3 { color_of(m.glow_ink) } else { text };
        let recolour =
            first || old.as_ref().map(|o| o.0.ground != m.ground || o.0.glow_ink != m.glow_ink).unwrap_or(true);
        let restyle =
            first || old.as_ref().map(|o| o.2 != style || Self::edited(&o.1, o.2) != edited).unwrap_or(true);
        // The caption names the material by its lights as well as its inks.
        let relit = old.as_ref().map(|o| o.0.lights != m.lights).unwrap_or(true);
        for i in 0..STYLES.len() {
            let knob = self.view.turned_knob(cx, &[Self::cell(i), live_id!(knob)]);
            knob.set_style(cx, i);
            knob.set_material(cx, &m);
            knob.set_value(cx, self.look.value);
            knob.set_lit(cx, self.look.lit);
            if recolour || restyle {
                let mut name = self.view.widget(cx, &[Self::cell(i), live_id!(name)]);
                let label = STYLES[i].label();
                let color = if i == style { accent } else { meta };
                name.set_text(cx, &label);
                script_apply_eval!(cx, name, { draw_text +: {color: #(color)} });
            }
        }
        let big = self.view.turned_knob(cx, &[live_id!(big)]);
        big.set_style(cx, style);
        big.set_shape(cx, &shape);
        big.set_material(cx, &m);
        big.set_value(cx, self.look.value);
        big.set_lit(cx, self.look.lit);
        let view3d = self.view.knob_view3d(cx, &[live_id!(view3d)]);
        view3d.set_style(cx, style);
        view3d.set_shape(cx, &shape);
        view3d.set_material(cx, &m);
        view3d.set_value(cx, self.look.value);
        if recolour || restyle || relit {
            // The material by its inks and, of the chrome and the showroom
            // chrome, its lights.
            let material = self.look.material_name();
            let mut caption = self.view.widget(cx, &[live_id!(caption)]);
            let style_name = STYLES[style].label();
            let words = if edited {
                format!("{style_name} (edited) in {material}")
            } else {
                format!("{style_name} in {material}")
            };
            caption.set_text(cx, &words);
            script_apply_eval!(cx, caption, { draw_text +: {color: #(text)} });
            let mut intro = self.view.widget(cx, &[live_id!(intro)]);
            script_apply_eval!(cx, intro, { draw_text +: {color: #(meta)} });
        }
        self.view.redraw(cx);
    }
}

impl ScriptHook for KnobPresets {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.look.reset();
        self.load_style(DEFAULT_STYLE);
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        // The style index moved on its own (its slider, or a rebuild putting
        // the edits back in the order they were made): the style comes whole,
        // curves and all, as it always did. A style preset writes the index
        // first, so the numbers it writes after it land on top of this.
        let style = self.look.style_index();
        if style != self.loaded_style {
            self.load_style(style);
            // The controls still show the style that was. They are told on
            // the page's next event pass, once every write of this one has
            // landed (a preset's numbers, a rebuild's later edits), and this
            // action brings that pass on at once. Left to the next input
            // event, the telling could reach the panel in the same pass as
            // the index slider's next step, and land this style's numbers on
            // the style that step loads.
            self.announce = true;
            let uid = self.widget_uid();
            vm.with_cx_mut(|cx| cx.widget_action(uid, KnobPresetsAction::StyleLoaded(style)));
        }
        // A curve control's write, after the style so a curve written with
        // it lands on top. Each is taken as it arrives and its field cleared:
        // left there, the array would be read again by the next apply (the
        // style index moving) and written back over the style it loads.
        let written = [self.prof, self.flute, self.wwid, self.whgt, self.wprof];
        for (which, value) in written.into_iter().enumerate() {
            if let Some(anchors) = read_curve(vm, value) {
                self.put_curve(which, anchors);
            }
        }
        self.prof = ScriptValue::NIL;
        self.flute = ScriptValue::NIL;
        self.wwid = ScriptValue::NIL;
        self.whgt = ScriptValue::NIL;
        self.wprof = ScriptValue::NIL;
        vm.with_cx_mut(|cx| self.view.redraw(cx));
    }
}

/// A curve control's write as anchors: an array of `[x, y, kind]` arrays,
/// `[x, y]` being smooth as the curve editor reads it. Anything else, or any
/// entry that is not two or three numbers, is no curve: a curve with a row
/// left out is not the curve that was written.
fn read_curve(vm: &ScriptVm, value: ScriptValue) -> Option<Vec<Anchor>> {
    let heap = &vm.bx.heap;
    let rows = value.as_array()?;
    (0..heap.array_len(rows))
        .map(|i| {
            let row = heap.array_index_unchecked(rows, i).as_array()?;
            let len = heap.array_len(row);
            if !(2..=3).contains(&len) {
                return None;
            }
            let at = |k: usize| heap.array_index_unchecked(row, k).as_number();
            Some([at(0)?, at(1)?, if len == 3 { at(2)? } else { 1.0 }])
        })
        .collect()
}

impl Widget for KnobPresets {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.push(cx);
        let m = self.look.material();
        set_material_uniforms(cx, &mut self.view.draw_bg.draw_vars, &m);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The style the index loaded (see `on_after_apply`), told once every
        // write of its pass has landed, as the page now shows it. Only a
        // style loading sets this, and the values told back move no index,
        // so the telling never tells itself again, and a curve editor's
        // drag, which loads no style, is never answered.
        if std::mem::take(&mut self.announce) {
            self.tell_controls(cx);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let mut picked = None;
        let mut turned = None;
        for action in actions.iter() {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            match wa.cast::<TurnedKnobAction>() {
                TurnedKnobAction::Changed(v) => turned = Some(v),
                TurnedKnobAction::Tapped => {
                    for i in 0..STYLES.len() {
                        let knob = self.view.turned_knob(cx, &[Self::cell(i), live_id!(knob)]);
                        if knob.widget_uid() == wa.widget_uid {
                            picked = Some(i);
                        }
                    }
                }
                TurnedKnobAction::None => {}
            }
        }
        // What a knob does on the page, the controls show: the value
        // slider follows a turn, and a pick moves the style preset and
        // every control it writes. Setting a preset's choice by label only
        // shows the choice, so its values are set one by one.
        let uid = self.widget_uid();
        if let Some(v) = turned {
            self.look.value = v;
            self.push(cx);
            cx.widget_action(uid, StoryControlAction::Set { label: VALUE, value: ControlValue::Number(v) });
        }
        if let Some(i) = picked {
            self.load_style(i);
            self.push(cx);
            cx.widget_action(uid, KnobPresetsAction::StylePicked(i));
            cx.widget_action(uid, StoryControlAction::Set { label: STYLE_INDEX, value: ControlValue::Number(i as f64) });
            self.tell_controls(cx);
        }
        cx.extend_actions(actions);
    }
}

/// What the page raised.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum KnobPresetsAction {
    /// A knob in the gallery was tapped: its style is the picked one now.
    StylePicked(usize),
    /// The style index moved and the page loaded that style whole. Raised
    /// from the apply that moved it, so that the pass it brings is the one
    /// the page tells the controls on.
    StyleLoaded(usize),
    #[default]
    None,
}

// ---- the controls ----

// The shape, in the bench's words.
const CAP_RADIUS: &str = "Cap radius (0 = none)";
const SPUN: &str = "Spun metal";
const FLAT: &str = "Flat cut (0 = none)";
const CAP_INK: &str = "Cap ink";
const REVOLVE_PROFILE: &str = "Revolve profile";
const TOOTH_PROFILE: &str = "Tooth profile";
const WING_WIDTH_CURVE: &str = "Wing width";
const WING_HEIGHT: &str = "Wing height";
const WING_SECTION: &str = "Wing section";
const COLUMNS: &str = "Columns (0 = smooth)";
const TOOTH_DEPTH: &str = "Tooth depth (pt)";
const TOOTH_SHARPNESS: &str = "Tooth sharpness";
const GRIP_TAPER: &str = "Taper across band";
const FLUTES_FROM: &str = "Flutes from point";
const FLUTES_TO: &str = "Flutes to point";
const WING_ROOT: &str = "Wing root (x R, - = tail)";
const WING_TIP: &str = "Wing tip (x R, 0 = none)";
const WING_WIDTH: &str = "Wing width (x R)";
const WING_BLEND: &str = "Wing blend (fillet, x R)";
const WING_END: &str = "Wing end rounding (x half-width)";
const WING_MODE: &str = "Wing mode (0 ridge added / 1 cutters taken away)";
const WING_BASE: &str = "Wing base (x cap height, the level the section stands on)";
const POINTER: &str = "Pointer (0 none / line / tapered / dot)";
const POINTER_INNER: &str = "Pointer inner (x R)";
const POINTER_OUTER: &str = "Pointer outer (x R)";
const POINTER_WIDTH: &str = "Pointer width (pt)";
const DIAL_TICKS: &str = "Dial ticks";
const TICK_RADIUS: &str = "Tick radius (x R)";
const TICK_LENGTH: &str = "Tick length (x R, 0 = dot)";
const TICK_STROKE: &str = "Tick stroke (pt at 56pt)";
const ARC_WIDTH: &str = "Value arc width (0 = none)";
const ARC_RADIUS: &str = "Value arc radius (x R)";
const ARC_WELL: &str = "Value arc well (x arc width, 0 = none)";
const WELL: &str = "Well (x radius, 0 = none)";
const CUT: &str = "Cut (0 none / dimple / slot / scallops / ring)";
const CUT_COUNT: &str = "Cut count";
const CUT_POSITION: &str = "Cut position (x R)";
const CUT_SIZE: &str = "Cut size (x R)";
const DIMPLE_SHAPE: &str = "Dimple shape (0 flat floor / 1 sphere, the cut size its radius)";
const SPHERE_CENTRE: &str = "Sphere centre above the surface (x R)";
const CUT_LENGTH: &str = "Cut half length (x R)";
const CUT_FLOOR: &str = "Cut floor (x cap height, 0 = through)";
const CUT_WALL: &str = "Cut wall (x R per cap height)";
const CUT_BLEND: &str = "Cut blend (fillet, x R)";

/// Every shape control's value in one style, the style index first: the
/// whole geometry, its curves too, so the curve editors show the style's.
fn style_preset(option: usize) -> Vec<(&'static str, ControlValue)> {
    let Some(shape) = style_shapes().get(option) else {
        return Vec::new();
    };
    let mut values = vec![(STYLE_INDEX, ControlValue::Number(option as f64))];
    values.extend(shape_values(shape));
    values
}

/// Every shape control's value in a shape, by label: what a style preset
/// writes after the index, and what the page tells the controls its shape
/// is.
fn shape_values(s: &KnobShape) -> Vec<(&'static str, ControlValue)> {
    use ControlValue::{Color, Curve, Number};
    vec![
        (CAP_RADIUS, Number(s.capr)),
        (SPUN, Number(s.spun)),
        (FLAT, Number(s.flat)),
        (CAP_INK, Color(s.cap_ink)),
        (REVOLVE_PROFILE, Curve(s.prof.clone())),
        (FLUTES_FROM, Number(s.ffrom as f64)),
        (FLUTES_TO, Number(s.fto as f64)),
        (TOOTH_PROFILE, Curve(s.flute.clone())),
        (COLUMNS, Number(s.flutes)),
        (TOOTH_DEPTH, Number(s.fd)),
        (TOOTH_SHARPNESS, Number(s.fs)),
        (GRIP_TAPER, Number(s.gtaper)),
        (WING_WIDTH_CURVE, Curve(s.wwid.clone())),
        (WING_HEIGHT, Curve(s.whgt.clone())),
        (WING_SECTION, Curve(s.wprof.clone())),
        (WING_ROOT, Number(s.wr0)),
        (WING_TIP, Number(s.wr1)),
        (WING_WIDTH, Number(s.wwmax)),
        (WING_BLEND, Number(s.barfil)),
        (WING_END, Number(s.wendr)),
        (WING_MODE, Number(s.wmode)),
        (WING_BASE, Number(s.wbase)),
        (POINTER, Number(s.ptype)),
        (POINTER_INNER, Number(s.pr0)),
        (POINTER_OUTER, Number(s.pr1)),
        (POINTER_WIDTH, Number(s.pw)),
        (DIAL_TICKS, Number(s.ticks)),
        (TICK_RADIUS, Number(s.tickr)),
        (TICK_LENGTH, Number(s.tickl)),
        (TICK_STROKE, Number(s.tickw)),
        (ARC_WIDTH, Number(s.arcw)),
        (ARC_RADIUS, Number(s.arcr)),
        (ARC_WELL, Number(s.awell)),
        (WELL, Number(s.well)),
        (CUT, Number(s.cut)),
        (CUT_COUNT, Number(s.cn)),
        (CUT_POSITION, Number(s.cr)),
        (CUT_SIZE, Number(s.cs)),
        (DIMPLE_SHAPE, Number(s.csph)),
        (SPHERE_CENTRE, Number(s.cz)),
        (CUT_LENGTH, Number(s.cl)),
        (CUT_FLOOR, Number(s.cf)),
        (CUT_WALL, Number(s.cw)),
        (CUT_BLEND, Number(s.cfil)),
    ]
}

/// A curve editor on one of the page's curves, captioned at its two ends.
const fn curve(
    label: &'static str,
    prop: &'static str,
    default: &'static [Anchor],
    left: &'static str,
    right: &'static str,
) -> Control {
    Control {
        label,
        target: "",
        kind: ControlKind::Curve { prop, default, left, right, guide: None, guide_label: "", mirror: false },
    }
}

/// The style the shape's controls open on.
const S: KnobStyle = STYLES[DEFAULT_STYLE];

pub const STORIES: &[Story] = &[Story {
    key: "containers/material/knobs",
    category: "Containers",
    component: "Material",
    also: &["TurnedKnob", "KnobView3d"],
    name: "Knob presets",
    dsl: "MaterialKnobPresets",
    added: "2026-09-26",
    tags: &["material", "knob", "presets", "3d", "skeuomorph", "bench"],
    doc: "# Knob presets

The Material Bench's knob engine, ported into the storybook: eighteen of its knob styles, live, in any of its eleven materials and two of ours (a dark neumorphic one and a showroom chrome), on that material's ground. The picked style is shown large and in 3D beside the gallery, in the shape the geometry controls give it.

## The engine

`TurnedKnob` draws one knob in one quad, grown past its layout rect as far as its light and shadow reach and composed over the page (transparent where it paints nothing), so no quad edge ever shows: the ground under it with the knob's cast shadow, ground lip and contact ring, the wells some styles stand in, the knob's face and its marks -- the pointer, the dial ticks and the value arc. `KnobView3d` ray marches the same solid under an orbiting camera. Both live in the storybook (`apps/storybook/src/knob/`); nothing in the widget library depends on them.

A **style** is geometry: a revolve profile drawn as a Bezier curve, a grip (flutes, knurls, lobes), a wing -- a ridge added along the pointer or two cutters taken away -- a cut (a dimple, a slot, scallops, a ring), a flat, a cap, and the marks. A **material** is light and finish: the key light, the tier, the specular and roughness (GGX), metal, a clear coat, the studio it reflects (a softbox studio, a chrome studio hung with strip lights, outdoors), exposure and highlight roll-off, the shadow, and seven inks. The two multiply: any style in any material. The showroom chrome is the chrome in a studio hung with seven strip lights where the bench has three, so its face shows more of the room.

What the bench works out in JavaScript is worked out here in Rust, once per shape and light: the curves resampled to 256 taps, the revolve's outline by height for the analytic cast-shadow sweep, the wing as a few knots, and the self-shadow over the disc as a 64 x 64 table. The shader reads them from two small textures. An edited shape is baked afresh as it changes; every knob of one shape shares its bakes.

## Reading a knob

- The **cast shadow** is swept analytically along the light from the solid's outline at each height, so a tall wing throws a long, soft shadow and a low skirt a short, crisp one.
- The **face** is lit from the solid's own height field: five taps give the normal, the curvature widens the highlight where the surface turns inside a pixel, and a crease is reflected from both of its sides with the darker kept, so thin bright lines do not sparkle.
- Reflective materials show the studio in the face; metals show it in their own colour.

## Using it

Drag any knob, the large one included, to turn them all; tap a knob in the gallery to pick its style. On the 3D view a drag orbits the camera, ctrl and the wheel zoom, and a double tap puts the camera back.

The Controls tab has the style and value; the shape's own numbers and curves in folding groups -- Cap, Profile, Grip, Wing, Pointer, Dial and Cut -- which the style preset, the style index or a tap on a knob in the gallery sets all at once, the curves drawn in curve editors (the revolve profile, half a grip tooth, and the wing's width, height and section); a material preset that sets every material control at once; and the bench's material controls in folding groups: Light, Surface, Environment, Relief, Wells, Shadow and Colours. The gallery always shows the styles as they come; the large knob and the 3D view show the shape as edited.",
    subject: "",
    feature: None,
    controls: crate::knob_controls!(
        style: self::style_preset,
        shape: [
            section("Cap", false),
            number(CAP_RADIUS, "capr", 0., 1., 0.01, S.capr),
            number(SPUN, "spun", 0., 1., 0.01, S.spun),
            number(FLAT, "flat", 0., 1., 0.01, S.flat),
            color(CAP_INK, "cap_ink", S.cap_ink),
            section("Profile", false),
            curve(REVOLVE_PROFILE, "prof", S.prof, "CENTRE", "SKIRT RIM"),
            // Profile anchors: the bench's slider runs to the profile's last,
            // and the chrome cap's grip runs to its seventh.
            number(FLUTES_FROM, "ffrom", 0., 6., 1., S.ffrom as f64),
            number(FLUTES_TO, "fto", 0., 6., 1., S.fto as f64),
            section("Grip", false),
            // Half a tooth, crest to trough: the mirror shows the whole one it
            // makes round the knob.
            Control {
                label: TOOTH_PROFILE,
                target: "",
                kind: ControlKind::Curve {
                    prop: "flute",
                    default: S.flute,
                    left: "CREST",
                    right: "TROUGH",
                    guide: None,
                    guide_label: "",
                    mirror: true,
                },
            },
            number(COLUMNS, "flutes", 0., 96., 1., S.flutes),
            number(TOOTH_DEPTH, "fd", 0., 12., 0.25, S.fd),
            number(TOOTH_SHARPNESS, "fs", 0., 1., 0.01, S.fs),
            number(GRIP_TAPER, "gtaper", 0., 1., 0.01, S.gtaper),
            section("Wing", false),
            curve(WING_WIDTH_CURVE, "wwid", S.wwid, "ROOT", "TIP"),
            // The crest's height: the box is twice the cap's, so the cap's top
            // is the guide at half.
            Control {
                label: WING_HEIGHT,
                target: "",
                kind: ControlKind::Curve {
                    prop: "whgt",
                    default: S.whgt,
                    left: "ROOT",
                    right: "TIP",
                    guide: Some(0.5),
                    guide_label: "CAP TOP",
                    mirror: false,
                },
            },
            curve(WING_SECTION, "wprof", S.wprof, "CENTRE", "EDGE"),
            // The bench's slider stops at -1; the winged knob's root is at -1.3.
            number(WING_ROOT, "wr0", -1.5, 1., 0.05, S.wr0),
            number(WING_TIP, "wr1", 0., 2.5, 0.05, S.wr1),
            number(WING_WIDTH, "wwmax", 0.05, 2.4, 0.01, S.wwmax),
            number(WING_BLEND, "barfil", 0., 0.5, 0.01, S.barfil),
            number(WING_END, "wendr", 0., 1., 0.01, S.wendr),
            number(WING_MODE, "wmode", 0., 1., 1., S.wmode),
            number(WING_BASE, "wbase", 0., 1., 0.01, S.wbase),
            section("Pointer", false),
            number(POINTER, "ptype", 0., 3., 1., S.ptype),
            number(POINTER_INNER, "pr0", 0., 1., 0.01, S.pr0),
            number(POINTER_OUTER, "pr1", 0., 1.2, 0.01, S.pr1),
            number(POINTER_WIDTH, "pw", 0.5, 14., 0.25, S.pw),
            section("Dial", false),
            number(DIAL_TICKS, "ticks", 0., 24., 1., S.ticks),
            number(TICK_RADIUS, "tickr", 0.3, 1.6, 0.01, S.tickr),
            number(TICK_LENGTH, "tickl", 0., 0.4, 0.01, S.tickl),
            number(TICK_STROKE, "tickw", 0.5, 8., 0.1, S.tickw),
            number(ARC_WIDTH, "arcw", 0., 10., 0.25, S.arcw),
            number(ARC_RADIUS, "arcr", 0.5, 1.6, 0.01, S.arcr),
            number(ARC_WELL, "awell", 0., 6., 0.1, S.awell),
            number(WELL, "well", 0., 1.6, 0.01, S.well),
            section("Cut", false),
            number(CUT, "cut", 0., 4., 1., S.cut),
            number(CUT_COUNT, "cn", 2., 24., 1., S.cn),
            number(CUT_POSITION, "cr", 0., 1.3, 0.01, S.cr),
            number(CUT_SIZE, "cs", 0.02, 1., 0.005, S.cs),
            number(DIMPLE_SHAPE, "csph", 0., 1., 1., S.csph),
            number(SPHERE_CENTRE, "cz", -0.6, 1., 0.005, S.cz),
            number(CUT_LENGTH, "cl", 0.05, 1.3, 0.01, S.cl),
            number(CUT_FLOOR, "cf", 0., 1., 0.01, S.cf),
            number(CUT_WALL, "cw", 0.01, 0.8, 0.01, S.cw),
            number(CUT_BLEND, "cfil", 0., 0.4, 0.005, S.cfil),
        ],
    ),
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::apply_chunk;
    use crate::controls::{chunk_for, default_of};
    use crate::knob::look::{material_preset, DEFAULT_MATERIAL, MATERIAL_NAMES, STUDIO_LIGHTS, STYLE_NAMES};
    use crate::knob::presets::{material_index, style_index, MATERIALS};
    use crate::makepad_widgets::makepad_script::trap::NoTrap;

    /// The control a preset's value goes to: the one of that label that
    /// writes a property.
    fn control(label: &str) -> &'static Control {
        STORIES[0]
            .controls
            .iter()
            .find(|c| c.label == label && !matches!(c.kind, ControlKind::Section { .. } | ControlKind::Preset { .. }))
            .unwrap_or_else(|| panic!("no control {label}"))
    }

    /// The shape's own controls: those from its first section to the
    /// material's.
    fn shape_labels() -> Vec<&'static str> {
        let controls = STORIES[0].controls;
        let section = |label: &str| {
            controls
                .iter()
                .position(|c| c.label == label && matches!(c.kind, ControlKind::Section { .. }))
                .unwrap_or_else(|| panic!("no section {label}"))
        };
        controls[section("Cap")..section("Material")]
            .iter()
            .filter(|c| !matches!(c.kind, ControlKind::Section { .. }))
            .map(|c| c.label)
            .collect()
    }

    /// Build the page from its template the way the canvas does. The page
    /// and its shaders are DSL, which the compiler never reads, and a shader
    /// that fails to compile is no error anywhere -- the knob just paints
    /// nothing.
    fn build_page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            super::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        page
    }

    /// Write preset values to the page the way the controls panel does:
    /// each through its control's chunk.
    fn write(cx: &mut Cx, page: &WidgetRef, values: Vec<(&'static str, ControlValue)>) {
        for (label, value) in values {
            let chunk = chunk_for(control(label), &value).unwrap_or_else(|| panic!("{label} writes no chunk"));
            apply_chunk(cx, page, &chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
        }
    }

    fn shape_of(page: &WidgetRef) -> KnobShape {
        page.borrow::<KnobPresets>().expect("the page is a KnobPresets").shape()
    }

    #[test]
    fn the_page_builds_and_every_control_writes_a_property_it_has() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        for i in 0..STYLES.len() {
            let cell = [KnobPresets::cell(i), live_id!(knob)];
            assert!(!page.widget(&cx, &cell).is_empty(), "no knob in cell {i}");
        }
        assert!(page.widget(&cx, &[KnobPresets::cell(STYLES.len())]).is_empty(), "a cell with no style in it");
        // Every control writes a property the page really has, with the
        // value the panel opens on, and those values are the default style
        // in the default material.
        for control in STORIES[0].controls {
            let Some(chunk) = chunk_for(control, &default_of(control)) else {
                continue;
            };
            apply_chunk(&mut cx, &page, &chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
        }
        let p = page.borrow::<KnobPresets>().expect("the page is a KnobPresets");
        assert_eq!(p.shape(), KnobShape::from(&STYLES[DEFAULT_STYLE]));
        assert_eq!(p.look.material(), KnobMaterial { name: "custom", ..MATERIALS[DEFAULT_MATERIAL] });
    }

    #[test]
    fn a_style_preset_writes_every_shape_control_once() {
        let labels = shape_labels();
        // Thirty-eight numbers, the cap's ink and five curves.
        assert_eq!(labels.len(), 44);
        for i in 0..STYLES.len() {
            let values = style_preset(i);
            assert_eq!(values[0].0, STYLE_INDEX, "the index goes first, so the numbers land on the style it loads");
            for label in &labels {
                let n = values.iter().filter(|(l, _)| l == label).count();
                assert_eq!(n, 1, "style {i} writes {label} {n} times");
            }
            assert_eq!(values.len(), labels.len() + 1, "style {i} writes something that is no shape control");
        }
        assert!(style_preset(STYLES.len()).is_empty());
    }

    /// A value a slider cannot show is a slider showing the wrong value.
    #[test]
    fn every_preset_value_fits_its_control() {
        let presets = (0..STYLES.len()).map(style_preset).chain((0..MATERIALS.len()).map(material_preset));
        for values in presets {
            for (label, value) in values {
                match (&control(label).kind, value) {
                    (ControlKind::Number { min, max, .. }, ControlValue::Number(v)) => {
                        assert!(v >= *min && v <= *max, "{label}: {v} is outside {min}..{max}")
                    }
                    (ControlKind::Color { .. }, ControlValue::Color(_)) => {}
                    // The editor would tidy anything else into another
                    // curve than the one the style has.
                    (ControlKind::Curve { .. }, ControlValue::Curve(anchors)) => {
                        assert!(anchors.len() >= 2, "{label}: {anchors:?} is no curve");
                        for [x, y, kind] in &anchors {
                            assert!((0.0..=1.0).contains(x) && (0.0..=1.0).contains(y), "{label}: {anchors:?} leaves the box");
                            assert!([0.0, 1.0, 2.0, 3.0].contains(kind), "{label}: {anchors:?} has no such kind");
                        }
                        assert!(anchors.windows(2).all(|w| w[0][0] <= w[1][0]), "{label}: {anchors:?} is out of order");
                    }
                    _ => panic!("{label} is given a value of the wrong kind"),
                }
            }
        }
    }

    #[test]
    fn the_preset_names_are_the_tables() {
        assert_eq!(STYLE_NAMES.len(), STYLES.len());
        for (name, s) in STYLE_NAMES.iter().zip(STYLES.iter()) {
            assert_eq!(*name, s.label());
        }
        assert_eq!(MATERIAL_NAMES, &MATERIALS.map(|m| m.name)[..]);
        assert_eq!(STYLE_NAMES[DEFAULT_STYLE], "Winged");
        assert_eq!(MATERIAL_NAMES[DEFAULT_MATERIAL], "Charcoal");
    }

    #[test]
    fn a_style_preset_through_the_controls_makes_the_page_that_style() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        for (i, s) in STYLES.iter().enumerate() {
            write(&mut cx, &page, style_preset(i));
            assert_eq!(shape_of(&page), KnobShape::from(s), "{}", s.name);
        }
        // Each value lands on its own field, not only by the index loading
        // the style: the winged knob's numbers and curves written over the
        // classic knob, the index left alone, are the winged knob under the
        // classic knob's name.
        write(&mut cx, &page, style_preset(0));
        let values = style_preset(DEFAULT_STYLE).into_iter().filter(|(l, _)| *l != STYLE_INDEX).collect();
        write(&mut cx, &page, values);
        let expected = KnobShape { name: STYLES[0].name.to_string(), ..KnobShape::from(&STYLES[DEFAULT_STYLE]) };
        assert_eq!(shape_of(&page), expected);
    }

    /// A curve control's chunk reaches the page's shape, each curve its
    /// own, and stays until the style moves; a write that is no curve
    /// changes nothing.
    #[test]
    fn a_curve_chunk_reaches_the_page_shape() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let labels = [REVOLVE_PROFILE, TOOTH_PROFILE, WING_WIDTH_CURVE, WING_HEIGHT, WING_SECTION];
        let styled = style_shapes()[DEFAULT_STYLE].clone();
        let curve_of = |shape: &KnobShape, which: usize| {
            [&shape.prof, &shape.flute, &shape.wwid, &shape.whgt, &shape.wprof][which].clone()
        };
        for (which, label) in labels.into_iter().enumerate() {
            let ControlKind::Curve { prop, .. } = control(label).kind else {
                panic!("{label} is no curve control");
            };
            assert_eq!(prop, CURVES[which], "{label} writes another curve");
            let anchors = vec![[0.0, 0.9, 2.0], [0.25 + 0.1 * which as f64, 0.6, 1.0], [1.0, 0.1, 0.0]];
            write(&mut cx, &page, vec![(label, ControlValue::Curve(anchors.clone()))]);
            let shape = shape_of(&page);
            assert_eq!(curve_of(&shape, which), anchors, "{label}");
            assert_ne!(shape.fingerprint(), styled.fingerprint(), "{label}: an edited curve is a new bake");
            // Every curve written so far is the page's, and the rest are the
            // style's still.
            for other in which + 1..labels.len() {
                assert_eq!(curve_of(&shape, other), curve_of(&styled, other), "{label} moved {}", CURVES[other]);
            }
        }
        let edited = shape_of(&page);
        // An anchor that is no pair or triple of numbers makes the whole
        // write no curve, and so does a lone anchor.
        for chunk in ["{prof: [[0.0, 1.0, 1.0], [0.5], [1.0, 0.0, 1.0]]}", "{prof: [[0.0, 1.0, 1.0]]}", "{prof: 3.0}"] {
            apply_chunk(&mut cx, &page, chunk).unwrap_or_else(|e| panic!("{chunk}: {e}"));
            assert_eq!(shape_of(&page), edited, "{chunk} changed the shape");
        }
        // A kind left out is smooth, as the editor reads it.
        apply_chunk(&mut cx, &page, "{prof: [[0.0, 1.0], [1.0, 0.0, 2.0]]}").unwrap();
        assert_eq!(shape_of(&page).prof, vec![[0.0, 1.0, 1.0], [1.0, 0.0, 2.0]]);
        // The style moving brings its own curves, not the last ones written.
        write(&mut cx, &page, vec![(STYLE_INDEX, ControlValue::Number(0.0))]);
        assert_eq!(shape_of(&page), KnobShape::from(&STYLES[0]));
        write(&mut cx, &page, vec![(VALUE, ControlValue::Number(0.5))]);
        assert_eq!(shape_of(&page), KnobShape::from(&STYLES[0]), "an apply with no curve wrote one");
    }

    /// The caption says what the large knob shows: the style, whether its
    /// shape is edited, and the material, told from its twin by its lights.
    #[test]
    fn the_caption_names_the_shape_and_the_material() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let caption = |cx: &mut Cx| {
            page.borrow_mut::<KnobPresets>().expect("the page is a KnobPresets").push(cx);
            let label = page.widget(cx, &[live_id!(caption)]);
            let color = label.borrow::<Label>().expect("the caption is a Label").draw_text.color;
            (label.text(), color)
        };
        let (words, color) = caption(&mut cx);
        assert_eq!(words, "Winged in Charcoal");
        // The charcoal's ground is dark, so the caption takes the light text.
        assert_eq!(color, vec4(0.86, 0.88, 0.91, 1.0));
        // The chrome and the showroom chrome share their inks; the lights
        // tell them apart.
        let chrome = material_index("Chrome").expect("a chrome material");
        write(&mut cx, &page, material_preset(chrome));
        assert_eq!(caption(&mut cx).0, "Winged in Chrome");
        write(&mut cx, &page, vec![(STUDIO_LIGHTS, ControlValue::Number(7.0))]);
        assert_eq!(caption(&mut cx).0, "Winged in Showroom chrome");
        let flat = vec![[0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        page.borrow_mut::<KnobPresets>().expect("the page is a KnobPresets").set_curve(&mut cx, 0, flat);
        assert_eq!(caption(&mut cx).0, "Winged (edited) in Showroom chrome");
    }

    /// One event pass of the page: what it tells the controls, in order.
    fn told(cx: &mut Cx, page: &WidgetRef) -> Vec<(&'static str, ControlValue)> {
        let actions = cx.capture_actions(|cx| page.handle_event(cx, &Event::Actions(Vec::new()), &mut Scope::empty()));
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter_map(|wa| match wa.cast::<StoryControlAction>() {
                StoryControlAction::Set { label, value } => Some((label, value)),
                StoryControlAction::None => None,
            })
            .collect()
    }

    /// The style index moving loads its style whole, and the page's next
    /// event pass tells the controls so: the style preset and every shape
    /// control, with the shape the page shows once that pass's writes have
    /// landed. Told once: the values written back, and a curve dragged,
    /// tell nothing more.
    #[test]
    fn the_style_index_moving_tells_the_controls() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        assert!(told(&mut cx, &page).is_empty(), "told the controls of nothing moving");
        let dial = style_index("dial").expect("a dial style");
        let (s, winged) = (&STYLES[dial], &STYLES[DEFAULT_STYLE]);
        assert!(s.capr != winged.capr && s.cap_ink != winged.cap_ink && s.prof != winged.prof);
        write(&mut cx, &page, vec![(STYLE_INDEX, ControlValue::Number(dial as f64))]);
        let sets = told(&mut cx, &page);
        let value = |label: &str| {
            let mut found = sets.iter().filter(|(l, _)| *l == label).map(|(_, v)| v.clone());
            let value = found.next().unwrap_or_else(|| panic!("{label} was not told"));
            assert!(found.next().is_none(), "{label} was told twice");
            value
        };
        assert_eq!(value(STYLE_PRESET), ControlValue::Choice(dial));
        assert_eq!(value(CAP_RADIUS), ControlValue::Number(s.capr));
        assert_eq!(value(CAP_INK), ControlValue::Color(s.cap_ink));
        assert_eq!(value(REVOLVE_PROFILE), ControlValue::Curve(s.prof.to_vec()));
        // Every shape control and the preset, and not the index: the panel
        // moved that, and holds it.
        let labels = shape_labels();
        for label in &labels {
            value(label);
        }
        assert_eq!(sets.len(), labels.len() + 1);
        assert!(told(&mut cx, &page).is_empty(), "told twice");
        // Written back as the panel writes them, they load no style, so
        // they tell nothing; nor does a curve editor's drag.
        write(&mut cx, &page, sets.into_iter().filter(|(l, _)| *l != STYLE_PRESET).collect());
        assert!(told(&mut cx, &page).is_empty(), "the values told, written back, were told again");
        let dragged = vec![[0.0, 1.0, 1.0], [0.4, 0.8, 1.0], [1.0, 0.0, 1.0]];
        write(&mut cx, &page, vec![(REVOLVE_PROFILE, ControlValue::Curve(dragged))]);
        assert!(told(&mut cx, &page).is_empty(), "a curve's drag was answered");
        // What is told is the shape as the page shows it, not as the table
        // has it: a number written after the index in the same pass, as a
        // rebuild puts a later edit back, is told as written.
        write(&mut cx, &page, vec![(STYLE_INDEX, ControlValue::Number(0.0)), (CAP_RADIUS, ControlValue::Number(0.37))]);
        let sets = told(&mut cx, &page);
        assert!(sets.contains(&(STYLE_PRESET, ControlValue::Choice(0))));
        assert!(sets.contains(&(CAP_RADIUS, ControlValue::Number(0.37))), "{sets:?}");
        assert!(sets.contains(&(REVOLVE_PROFILE, ControlValue::Curve(STYLES[0].prof.to_vec()))));
    }

    #[test]
    fn a_curve_set_on_the_page_reaches_its_shape_until_the_style_moves() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let page = build_page(&mut cx);
        let prof = vec![[0.0, 1.0, 0.0], [0.7, 0.9, 1.0], [1.0, 0.0, 2.0]];
        {
            let mut p = page.borrow_mut::<KnobPresets>().expect("the page is a KnobPresets");
            p.set_curve(&mut cx, 0, prof.clone());
            // One anchor is no curve, and there is no sixth curve.
            p.set_curve(&mut cx, 1, vec![[0.0, 1.0, 0.0]]);
            p.set_curve(&mut cx, CURVES.len(), prof.clone());
        }
        let shape = shape_of(&page);
        assert_eq!(shape.prof, prof);
        assert_eq!(shape.flute, STYLES[DEFAULT_STYLE].flute.to_vec());
        assert_ne!(shape.fingerprint(), style_shapes()[DEFAULT_STYLE].fingerprint(), "an edited curve is a new bake");
        // Another style comes with its own curves.
        write(&mut cx, &page, vec![(STYLE_INDEX, ControlValue::Number(0.0))]);
        assert_eq!(shape_of(&page), KnobShape::from(&STYLES[0]));
    }
}
