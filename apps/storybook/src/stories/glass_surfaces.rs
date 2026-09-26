//! The glass surfaces gallery: the lensing backing and every preset of it,
//! over something worth bending; a second page of four sheets that open over
//! the page itself on the same material, one of which can be pressed; and a
//! third, the floating surface, which is that material with a frame you can
//! take hold of.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Cap = Label{draw_text +: {color: #ffffffcc}}
    // A slider on a glass sheet. The sheet is dark in every theme, so the
    // label and the value are white in every state, as the sheet titles
    // are; the theme's text colour is dark in light and would not read.
    let SheetSlider = Slider{
        width: Fill
        draw_text +: {
            color: #fff
            color_hover: #fff
            color_drag: #fff
            color_focus: #fff
            color_disabled: #fff
            color_empty: #fff
        }
        text_input +: {
            draw_text +: {
                color: #fff
                color_hover: #fff
                color_focus: #fff
                color_down: #fff
                color_disabled: #fff
                color_empty: #fff
                color_empty_hover: #fff
                color_empty_focus: #fff
            }
        }
    }

    mod.stories.GlassSurfacesOverview = StoryPage{
        StoryNote{text: "The lensing surface the glass family is built on, and the presets of it that each control uses. They all bend what is behind them, so they are shown over the same colourful ground — on a flat one they collapse to an outline."}

        StoryHeading{text: "The surfaces"}
        StoryNote{text: "LensSurface is the base every control's backing derives from. The rest are the same surface tuned for what sits on it: a button, a prominent button, a chip, an icon, an input, a radio."}
        // Each band is taller than the one it replaces by the 42 points
        // the shared stage's clearance costs over the old 9-point inset.
        GlassStage{
            height: 250.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.LensSurface{width: 110. height: 44.}
                    Cap{text: "LensSurface"}
                    mod.widgets.glass.ButtonSurface{width: 110. height: 44.}
                    Cap{text: "ButtonSurface"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.ProminentButtonSurface{width: 110. height: 44.}
                    Cap{text: "ProminentButtonSurface"}
                    mod.widgets.glass.ChipSurface{width: 90. height: 32.}
                    Cap{text: "ChipSurface"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.IconSurface{width: 44. height: 44.}
                    Cap{text: "IconSurface"}
                    mod.widgets.glass.InputSurface{width: 140. height: 36.}
                    Cap{text: "InputSurface"}
                    mod.widgets.glass.RadioSurface{width: 44. height: 26.}
                    Cap{text: "RadioSurface"}
                }
            }
        }

        StoryHeading{text: "The controls made from them"}
        StoryNote{text: "A lens button and a lens chip are those same surfaces with a padding and a centring — despite the names they are not buttons. They carry no text and no press: the label goes inside them, and if you want something that answers a click, that is GlassButton on Glass > Controls."}
        GlassStage{
            height: 240.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    // No text on any of these: they are surfaces with a
                    // padding and a centring, so the label goes inside.
                    mod.widgets.glass.LensButton{Cap{text: "LensButton"}}
                    mod.widgets.glass.LensButtonProminent{Cap{text: "LensButtonProminent"}}
                    mod.widgets.glass.LensChip{Cap{text: "LensChip"}}
                }
                mod.widgets.glass.ClearPanel{
                    width: Fill height: Fit
                    padding: theme.mspace_2
                    Cap{text: "ClearPanel"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.CutButton{text: "CutButton"}
                    mod.widgets.glass.ProminentButton{text: "ProminentButton"}
                    mod.widgets.glass.IconButton{text: "Icon"}
                    mod.widgets.glass.Body{text: "glass.Body"}
                    mod.widgets.glass.ButtonLabel{text: "glass.ButtonLabel"}
                }
                mod.widgets.glass.List{
                    width: Fill height: Fit
                    mod.widgets.glass.ListRow{Cap{text: "a glass list row"}}
                    mod.widgets.glass.ListRow{Cap{text: "and another"}}
                }
            }
        }

        StoryHeading{text: "The two rounded surfaces underneath"}
        StoryNote{text: "LensedRoundedView is the tuned preset of GaussRoundedView the whole family sits on, and GaussGradientRoundedView is the version that also lays a gradient over the blur."}
        rounded := GlassStage{
            height: 150.
            // The stage's slot flows Down, which is what a column of demos
            // wants; this band is one row, so it says so. The body already
            // centres what it holds on the cross axis.
            body +: {
                flow: Right
                LensedRoundedView{width: 150. height: 70.}
                Cap{text: "LensedRoundedView"}
                GaussGradientRoundedView{width: 150. height: 70.}
                Cap{text: "GaussGradientRoundedView"}
            }
        }
    }

    mod.storybook.StoryPressLensBase = #(StoryPressLens::register_widget(vm))

    /** A water lens that answers a press: a flat button with every face
     * state erased lies over the glass, and this widget forwards the press
     * to the library's press model, which flattens the lens under the finger
     * and lets it rebound. */
    mod.storybook.StoryPressLens = set_type_default() do mod.storybook.StoryPressLensBase{
        width: Fill
        height: Fit

        press_lens_popup := PopupNotification{
            align: Align{x: 0.5 y: 0.5}
            content +: {
                width: 300
                height: 92
                flow: Overlay
                clip_x: false
                clip_y: false

                press_lens := RippleLensRoundedView{
                    width: Fill
                    height: Fill
                    draw_bg +: {
                        blur_level: 0.25
                        lensing_effect: 1.0
                        lensing_strength: 38.0
                        lensing_width: 13.0
                        corner_radius: 23.0
                        tint_color: #b8b8b8
                        tint_alpha: 0.025
                        border_alpha: 0.82
                        specular_strength: 0.24
                        noise_strength: 0.004
                        shadow_color: #0009
                        shadow_radius: 34.0
                        shadow_offset: vec2(0.0, 14.0)
                        diffraction_strength: 5.2
                    }
                }
                /** the click target over the lens: label only, face erased */
                press_lens_face := ButtonFlat{
                    width: Fill
                    height: Fill
                    text: "Focus"
                    draw_text +: {
                        /** lens label ink: white, dimming to 65% under the
                         * pointer and to 25% while held, as the lens
                         * button's label does */
                        color: #fff
                        color_hover: #ffffffa6
                        color_down: #ffffff40
                        color_focus: #fff
                        /** lens label size in points 8..48 step 1 */
                        text_style +: {font_size: 22}
                    }
                    /** every face state erased so the glass behind shows through */
                    draw_bg +: {
                        /** no bevel: the glass draws its own edge 0..4 step 0.5 */
                        border_size: 0.0
                        /** face transparent in every state */
                        color: #0000
                        color_hover: #0000
                        color_down: #0000
                        color_focus: #0000
                        color_disabled: #0000
                        /** bevel transparent in every state */
                        border_color: #0000
                        border_color_hover: #0000
                        border_color_down: #0000
                        border_color_focus: #0000
                        border_color_disabled: #0000
                    }
                }
            }
        }
    }

    mod.stories.GlassSurfacesPopups = StoryPage{
        StoryNote{text: "Four sheets that open over the page, each backed by one member of the glass family: a plain frosted one, one with a lens at its rim, one you can press, and one whose blur is sharper at its edge. They are window-centred popups, and a glass inside an overlay still samples what is under it, so each bends the page rather than a flat backdrop."}

        StoryHeading{text: "Four sheets over the page"}
        StoryNote{text: "Open one and the other three close. Each is a PopupNotification with the glass as its whole background and the content laid over it; the stage below is there to give the sheets something worth bending."}
        StoryRow{
            open_blur_sheet := Button{text: "Open blur sheet"}
            open_lens_sheet := Button{text: "Open lens sheet"}
            open_press_lens := Button{text: "Open pressable lens"}
            open_gradient_sheet := Button{text: "Open gradient sheet"}
        }
        // A fixed height, never Fit: the sheets are window-centred and the
        // ground has to reach under them from where the row leaves off.
        GlassStage{
            height: 440.
            body +: {Cap{text: "The sheets open over this ground"}}
        }

        StoryHeading{text: "A blur sheet, no lens"}
        StoryNote{text: "The raw GaussRoundedView, frosted with the lens off. The two sliders on it retune the surface that is already on screen through set_blurriness and set_lensing_effect; slide the second one up and it turns into the lens sheet next door."}

        StoryHeading{text: "A lens sheet"}
        StoryNote{text: "The same popup on LensedRoundedView with the lens at 0.75 instead of 0: the rim bends the ground under it where the blur sheet's does not. Its sliders drive the same two setters. Every other knob of the material — tint, seal, rim, shadow — is the family's, and is explained on Glass > Overview."}

        StoryHeading{text: "A lens you can press"}
        StoryNote{text: "A water lens made pressable by a flat button laid over it with every face state erased, so only its label shows. Hold it and the lens lies flat behind a ring that crosses it; let go and it springs back behind a softer ring, a little past rest before it settles. The clock ticks only while the lens moves, so the sheet costs no frames while it sits there or while it is held. A click closes the sheet once the rebound has played."}

        StoryHeading{text: "A sheet that is sharper at its edge"}
        StoryNote{text: "GaussGradientRoundedView: the blur level ramps from gradient_blur_edge at the rim to blur_level in the middle, over gradient_blur_edge_width of the sheet's size and shaped by gradient_blur_power, with the lens off. The four knobs are in the controls panel. They write to the sheet whether or not it is open, so open it first."}

        // Declared at the end deliberately: a PopupNotification takes a Fill
        // slot in the column it is written in, so anywhere earlier it would
        // push the sections after it down the page.
        blur_popup := PopupNotification{
            align: Align{x: 0.5 y: 0.5}
            content +: {
                width: 430
                height: 300
                flow: Overlay
                clip_x: false
                clip_y: false

                blur_sheet := GaussRoundedView{
                    width: Fill
                    height: Fill
                    draw_bg +: {
                        blur_level: 5.0
                        lensing_effect: 0.0
                        corner_radius: 18.0
                        tint_color: #b8b8b8
                        tint_alpha: 0.07
                        surface_alpha: 0.82
                        border_alpha: 0.36
                        specular_strength: 0.10
                        shadow_color: #000b
                        shadow_radius: 44.0
                        shadow_offset: vec2(0.0, 18.0)
                    }
                }

                blur_sheet_content := View{
                    width: Fill
                    height: Fill
                    padding: 22
                    flow: Down
                    spacing: 12

                    Label{
                        text: "Blur sheet"
                        draw_text +: {color: #fff text_style +: {font_size: 18}}
                    }
                    blur_sheet_blur := SheetSlider{text: "Blurriness" min: 0.0 max: 6.0 default: 5.0}
                    blur_sheet_lensing := SheetSlider{text: "Lensing Effect" min: 0.0 max: 100.0 default: 0.0}
                    View{
                        width: Fill
                        height: Fill
                    }
                    View{
                        width: Fill
                        height: 72
                        flow: Right
                        align: Align{x: 1.0 y: 0.5}
                        close_blur_popup := ButtonFlat{text: "Close"}
                    }
                }
            }
        }

        lens_popup := PopupNotification{
            align: Align{x: 0.5 y: 0.5}
            content +: {
                width: 430
                height: 300
                flow: Overlay
                clip_x: false
                clip_y: false

                lens_sheet := LensedRoundedView{
                    width: Fill
                    height: Fill
                    draw_bg +: {
                        blur_level: 5.2
                        lensing_effect: 0.75
                        corner_radius: 18.0
                        tint_color: #b8b8b8
                        tint_alpha: 0.08
                        surface_alpha: 0.76
                        border_alpha: 0.56
                        specular_strength: 0.14
                        shadow_color: #000c
                        shadow_radius: 46.0
                        shadow_offset: vec2(0.0, 20.0)
                        diffraction_strength: 2.4
                    }
                }

                lens_sheet_content := View{
                    width: Fill
                    height: Fill
                    padding: 22
                    flow: Down
                    spacing: 12

                    Label{
                        text: "Lens glass"
                        draw_text +: {color: #fff text_style +: {font_size: 18}}
                    }
                    lens_sheet_blur := SheetSlider{text: "Blurriness" min: 0.0 max: 6.0 default: 5.2}
                    lens_sheet_lensing := SheetSlider{text: "Lensing Effect" min: 0.0 max: 100.0 default: 75.0}
                    View{
                        width: Fill
                        height: Fill
                    }
                    View{
                        width: Fill
                        height: 72
                        flow: Right
                        align: Align{x: 1.0 y: 0.5}
                        close_lens_popup := ButtonFlat{text: "Close"}
                    }
                }
            }
        }

        gradient_popup := PopupNotification{
            align: Align{x: 0.5 y: 0.5}
            content +: {
                width: 520
                height: 280
                flow: Overlay
                clip_x: false
                clip_y: false

                // Written out in full although the template's defaults are
                // the same, so the knobs the controls panel drives can be
                // read off the page.
                gradient_sheet := GaussGradientRoundedView{
                    width: Fill
                    height: Fill
                    draw_bg +: {
                        blur_level: 4.35
                        gradient_blur_edge: 1.45
                        gradient_blur_edge_width: 0.20
                        gradient_blur_power: 0.75
                        corner_radius: 18.0
                        tint_color: #b8b8b8
                        tint_alpha: 0.045
                        border_alpha: 0.44
                        specular_strength: 0.08
                        shadow_color: #000b
                        shadow_radius: 44.0
                        shadow_offset: vec2(0.0, 18.0)
                    }
                }

                gradient_sheet_content := View{
                    width: Fill
                    height: Fill
                    padding: 22
                    flow: Down
                    spacing: 12

                    Label{
                        text: "Gradient blur"
                        draw_text +: {color: #fff text_style +: {font_size: 18}}
                    }
                    View{
                        width: Fill
                        height: Fill
                    }
                    View{
                        width: Fill
                        height: 72
                        flow: Right
                        align: Align{x: 1.0 y: 0.5}
                        close_gradient_popup := ButtonFlat{text: "Close"}
                    }
                }
            }
        }

        press_demo := mod.storybook.StoryPressLens{}
    }

    mod.stories.GlassFloatingSurfaceOverview = StoryPage{
        StoryNote{text: "The panel's material with a frame you can take hold of. Drag the body to move it; drag any edge or any corner to size it. It floats over the window in window points, so it is already up when this page opens."}

        StoryHeading{text: "Take it down and put it back"}
        StoryNote{text: "The page declares shown: true, so the surface is here on arrival; these two buttons are open() and close(). A rebuild - a theme switch, a reload - brings back what the page declared, not what you last pressed."}
        StoryRow{
            show_surface := Button{text: "Show it"}
            hide_surface := Button{text: "Hide it"}
            surface_state := Label{text: "shown"}
        }

        StoryHeading{text: "Something worth bending"}
        StoryNote{text: "Drag the surface across this band and back onto the plain page. The lens reads the scene BEHIND the surface, and it is re-read on every frame of a move or a resize - which is the whole difficulty of making a glass surface resizable, and the reason a naive one carries a picture of where the drag started."}
        GlassStage{height: 200.}

        StoryHeading{text: "Whose press it is"}
        StoryNote{text: "The surface can be dragged over the navigator and the splitter bars, but a drag cannot be STARTED on the part of it that lies over them: those panes are asked about a press before the pane this page lives in, and a press belongs to whoever answered it first. Park the sheet over the file tree and the cost is plain - every attempt to pick it back up opens a different story instead, and takes this page down with it. Start the drag over the page and it carries on anywhere."}

        StoryHeading{text: "What it reports"}
        StoryNote{text: "Every drag reports the frame it is passing through and the frame it settles on. The size is held between min_size and max_size, and the surface is pulled back inside the window every draw - one whose frame had gone past an edge could never be dragged back."}
        StoryRow{
            surface_frame := Label{text: "not moved yet"}
        }

        floater := mod.widgets.glass.FloatingSurface{
            shown: true
            pos: vec2(470., 300.)
            // Wide and tall enough for the heading, both paragraphs and the
            // button below them at this body size: a sheet that hides the
            // control its prose points at is demonstrating nothing.
            size: vec2(360., 340.)
            min_size: vec2(180., 120.)
            max_size: vec2(560., 460.)
            content +: {
                body +: {
                    mod.widgets.glass.H2{text: "A pane of glass"}
                    mod.widgets.glass.Body{text: "The body moves it. The edges and the corners size it, and the corner mark says where the surest grip is."}
                    mod.widgets.glass.Body{text: "The button below still takes its own press: the move is only claimed by what nothing inside wanted."}
                    inside := Button{text: "A control on the glass"}
                }
            }
        }
    }
}

/// The pressable lens: a popup holding a water lens with a face-erased
/// button over it. The button reports the press; this widget forwards it to
/// the library's `LensPress`, which is the same press `GlassButton` has built
/// in, and pushes what comes back through the surface's press response. It
/// ticks NextFrame only while the lens is flattening or rebounding, so an
/// idle sheet, or a long hold, costs no frames. What it adds is the close: a
/// click puts the sheet away once the rebound has played.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryPressLens {
    #[deref]
    view: View,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    press: LensPress,
    /// A click closes the sheet, but only once the rebound has played.
    #[rust]
    pending_close: bool,
}

impl StoryPressLens {
    fn set_response(&self, cx: &mut Cx, response: PressResponse) {
        // A RippleLensRoundedView is a template over the one Rust widget, so
        // it borrows as that widget.
        if let Some(mut glass) = self
            .view
            .widget(cx, ids!(press_lens))
            .borrow_mut::<GaussRoundedView>()
        {
            glass.set_press_response(cx, response.flatten, response.ripple_age, response.ripple_strength);
        }
    }

    fn step(&mut self, cx: &mut Cx, step: LensPressStep) {
        if let Some(response) = step.response {
            self.set_response(cx, response);
        }
        if step.tick {
            self.next_frame = cx.new_next_frame();
        }
    }

    /// The lens at rest, with nothing of a previous press left on it.
    fn rest(&mut self, cx: &mut Cx) {
        self.pending_close = false;
        let response = self.press.rest();
        self.set_response(cx, response);
    }

    pub fn open_fresh(&mut self, cx: &mut Cx) {
        self.rest(cx);
        // Arm the window's capture before the open: the first sheet a fresh
        // process opens otherwise misses the frame it is painted in and
        // draws nothing until something else asks for a frame.
        arm_gauss_capture(cx);
        self.view.popup_notification(cx, ids!(press_lens_popup)).open(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.rest(cx);
        self.view.popup_notification(cx, ids!(press_lens_popup)).close(cx);
    }

    pub fn press(&mut self, cx: &mut Cx) {
        self.pending_close = false;
        let step = self.press.down(false);
        self.step(cx, step);
    }

    /// The face was released, and `close_after` says whether it was a click.
    /// The face reports a click as a release as well, and the second report
    /// changes nothing but the close.
    pub fn release(&mut self, cx: &mut Cx, close_after: bool) {
        self.pending_close |= close_after;
        let step = self.press.up(false);
        self.step(cx, step);
        if self.pending_close && self.press.is_at_rest() {
            // Nothing is left to play: a press this sheet never saw.
            self.close(cx);
        }
    }

    /// One frame of the press or the rebound.
    fn tick(&mut self, cx: &mut Cx, time: f64) {
        let Some(step) = self.press.tick(time, &LensPressCurve::default()) else {
            return;
        };
        self.step(cx, step);
        if !step.tick && self.pending_close && self.press.is_at_rest() {
            self.close(cx);
        }
    }
}

impl Widget for StoryPressLens {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Some(frame) = self.next_frame.is_event(event) {
            self.tick(cx, frame.time);
        }
    }
}

/// The three plain sheets: the button that opens each, the one that closes
/// it, and the popup. The fourth, the pressable lens, lives inside
/// `press_demo` and opens and closes through it, so that a close puts the
/// press state to rest along with the popup.
const SHEETS: &[(LiveId, LiveId, LiveId)] = &[
    (live_id!(open_blur_sheet), live_id!(close_blur_popup), live_id!(blur_popup)),
    (live_id!(open_lens_sheet), live_id!(close_lens_popup), live_id!(lens_popup)),
    (live_id!(open_gradient_sheet), live_id!(close_gradient_popup), live_id!(gradient_popup)),
];

fn glass_popups_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    // Every lookup first. A `borrow_mut` on the press demo is held while it
    // works, and a lookup from the root walks through the demo to reach
    // anything declared after it, which would ask for the same cell twice.
    let sheets: Vec<(ButtonRef, ButtonRef, PopupNotificationRef)> = SHEETS
        .iter()
        .map(|(open, close, popup)| {
            (
                root.button(cx, &[*open]),
                root.button(cx, &[*close]),
                root.popup_notification(cx, &[*popup]),
            )
        })
        .collect();
    let open_press = root.button(cx, ids!(open_press_lens));
    let face = root.button(cx, ids!(press_lens_face));
    let blur_blur = root.slider(cx, ids!(blur_sheet_blur));
    let blur_lensing = root.slider(cx, ids!(blur_sheet_lensing));
    let lens_blur = root.slider(cx, ids!(lens_sheet_blur));
    let lens_lensing = root.slider(cx, ids!(lens_sheet_lensing));
    let blur_sheet = root.widget(cx, ids!(blur_sheet));
    let lens_sheet = root.widget(cx, ids!(lens_sheet));
    let demo = root.story_press_lens(cx, ids!(press_demo));

    // One sheet at a time: an open closes the other three first.
    for (i, (open, _, popup)) in sheets.iter().enumerate() {
        if open.clicked(actions) {
            for (j, (_, _, other)) in sheets.iter().enumerate() {
                if j != i {
                    other.close(cx);
                }
            }
            if let Some(mut demo) = demo.borrow_mut() {
                demo.close(cx);
            }
            // See open_fresh: the capture has to be armed before the open.
            arm_gauss_capture(cx);
            popup.open(cx);
        }
    }
    if open_press.clicked(actions) {
        for (_, _, popup) in &sheets {
            popup.close(cx);
        }
        if let Some(mut demo) = demo.borrow_mut() {
            demo.open_fresh(cx);
        }
    }
    for (_, close, popup) in &sheets {
        if close.clicked(actions) {
            popup.close(cx);
        }
    }

    // The face reports the press; the demo owns the clock. A click is a
    // release as well, and it is the one that closes the sheet afterwards.
    if face.pressed(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.press(cx);
        }
    }
    if face.released(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.release(cx, false);
        }
    }
    if face.clicked(actions) {
        if let Some(mut demo) = demo.borrow_mut() {
            demo.release(cx, true);
        }
    }

    // The sliders retune the surface that is already drawn. The lensing
    // sliders run 0..100 for the finer steps; the setter takes 0..1.
    if let Some(value) = blur_blur.slided(actions) {
        if let Some(mut glass) = blur_sheet.borrow_mut::<GaussRoundedView>() {
            glass.set_blurriness(cx, value as f32);
        }
    }
    if let Some(value) = blur_lensing.slided(actions) {
        if let Some(mut glass) = blur_sheet.borrow_mut::<GaussRoundedView>() {
            glass.set_lensing_effect(cx, value as f32 / 100.0);
        }
    }
    if let Some(value) = lens_blur.slided(actions) {
        if let Some(mut glass) = lens_sheet.borrow_mut::<GaussRoundedView>() {
            glass.set_blurriness(cx, value as f32);
        }
    }
    if let Some(value) = lens_lensing.slided(actions) {
        if let Some(mut glass) = lens_sheet.borrow_mut::<GaussRoundedView>() {
            glass.set_lensing_effect(cx, value as f32 / 100.0);
        }
    }
}

fn glass_floating_surface_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let surface = root.glass_floating_surface(cx, ids!(floater));
    if root.button(cx, ids!(show_surface)).clicked(actions) {
        surface.open(cx);
    }
    if root.button(cx, ids!(hide_surface)).clicked(actions) {
        surface.close(cx);
    }
    if root.button(cx, ids!(inside)).clicked(actions) {
        let n = crate::stories::bump(live_id!(glass_surface_inside));
        root.button(cx, ids!(inside))
            .set_text(cx, &format!("pressed {n} times"));
    }
    // `framed` answers whichever kind of drag reported this pass, which is
    // what a readout wants: the page cares about the frame, not about which
    // handle is doing it.
    if let Some((pos, size)) = surface.framed(actions) {
        root.label(cx, ids!(surface_frame)).set_text(
            cx,
            &format!("at {:.0},{:.0} sized {:.0}x{:.0}", pos.x, pos.y, size.x, size.y),
        );
    }
    let state = if surface.is_open() { "shown" } else { "hidden" };
    let label = root.label(cx, ids!(surface_state));
    if label.text() != state {
        label.set_text(cx, state);
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/glass/surfaces",
        category: "Containers",
        component: "Glass",
        also: &[
            "LensSurface", "ButtonSurface", "ProminentButtonSurface", "ChipSurface",
            "IconSurface", "InputSurface", "RadioSurface",
            "LensButton", "LensButtonProminent", "LensChip", "ClearPanel",
            "List", "ListRow", "CutButton", "ProminentButton", "IconButton", "Body", "ButtonLabel",
            "GaussRoundedView", "LensedRoundedView", "GaussGradientRoundedView",
        ],
        name: "Surfaces",
        dsl: "GlassSurfacesOverview",
        added: "2026-02-12",
        tags: &[],
        doc: "# Glass surfaces

The lensing backing the glass family is built on, and every preset of it the library ships.

`GaussRoundedView` is the raw surface: it samples the scene behind itself through a chain of mip textures and blurs it. It arranges that capture itself. In normal flow it opens an overlay of its own and asks the window for the blurred scene on every draw, and it announces itself when it is built, so the window captures on the frame the surface first paints rather than the one after; an ordinary window needs nothing set up for it. `LensedRoundedView` is the tuned preset the family actually sits on, and `GaussGradientRoundedView` lays a gradient over the blur as well.

`LensSurface` derives from that, and everything else derives from `LensSurface` — the same surface adjusted for what sits on top of it. `ButtonSurface` and `ProminentButtonSurface` for buttons, `ChipSurface` for a chip, `IconSurface` for a square icon target, `InputSurface` for a field, `RadioSurface` for a toggle. **`LensButton`, `LensButtonProminent` and `LensChip` are not buttons.** They are the same surfaces with a padding and a centring, and they carry neither a label nor a press — writing `text:` on one is rejected at runtime, where only the log can see it. Put the label inside. The thing that answers a click is `GlassButton`, on Glass > Controls.

`ClearPanel` is the plain sheet, and `List` with `ListRow` are the family's own rows — note that these live under `mod.widgets.glass` and are *not* general-purpose list views, which is a mistake worth making only once.

They are shown over a coloured ground because every one of them draws what is behind it. Glass > Overview says when that makes the family the wrong choice.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
    Story {
        key: "containers/glass/sheets",
        category: "Containers",
        component: "Glass",
        also: &[
            "GaussRoundedView", "LensedRoundedView", "RippleLensRoundedView", "GaussGradientRoundedView",
            "PopupNotification", "ButtonFlat", "Slider", "LensPress",
        ],
        name: "Sheets",
        dsl: "GlassSurfacesPopups",
        added: "2026-02-12",
        tags: &["ported"],
        doc: "# Glass sheets

Four sheets that open over the page, each backed by one member of the glass family, and each a window-centred `PopupNotification` with the glass as its whole background and the content laid over it in an `Overlay` flow.

## They bend the page, not a backdrop

A `GaussRoundedView` samples the scene behind itself. Inside an overlay it does not open a second one: a surface that finds itself already drawing in an overlay binds the window's capture and draws inline, so a sheet that opens over the page bends whatever is under it — the coloured stage on this one included — rather than a flat fallback. That is what the stage under the launcher row is for: with nothing worth bending underneath, every one of these reads as a grey rectangle with a shadow.

## The four sheets

- **Blur sheet** — the raw `GaussRoundedView`, frosted with the lens off (`lensing_effect: 0`).
- **Lens sheet** — `LensedRoundedView`, the same material with the lens at 0.75, so the rim bends what is under it.
- **Pressable lens** — a `RippleLensRoundedView`, the water lens, with a face-erased `ButtonFlat` laid over it, so a lens answers a press.
- **Gradient sheet** — `GaussGradientRoundedView`, whose blur level ramps from the rim to the middle.

Every template of the family is a DSL preset over the one Rust widget, so any of them borrows as a `GaussRoundedView`. That is how the sliders on the first two sheets retune a surface that is already drawn: `set_blurriness` (0..6) and `set_lensing_effect` (0..1) write uniforms on the retained draw call, and the change lands on the frame after, with no rebuild.

## The press

The pressable lens is on `RippleLensRoundedView`, the water lens: the material the lens button is drawn with, livelier than the rest of the family, with a sheen toward the rim and the top, a colour split at the rim, and a press it can show. Its numbers are the lens button's: a 300 by 92 pill, a 38 point bend over a 13 point band, a 5.2 point colour split, and a soft shadow 34 points wide.

The shader never reads the frame clock. A press is three uniforms — `press_flatten`, `ripple_age`, `ripple_strength` — pushed through `set_press_response` on a `NextFrame` chain that runs only while the lens is flattening or rebounding, so an idle sheet and a long hold cost no frames. A shader that read `draw_pass.time` for this instead would pin the window at display rate for as long as the glass was visible.

The numbers come from the library's `LensPress`, the same press `GlassButton` has built in. The story widget here, `StoryPressLens`, forwards the face's press and release to it and adds one thing a button has no action for: a click closes the sheet once the rebound has played.

Held, the lens eases flat over 0.78 s behind a ring that crosses it in 0.88 s and fades over 1.05 s, and once the ring has gone it stays flat without asking for frames. Let go, it is sent a negative flatten: the lens springs back from as flat as it got behind a second ring at 62% strength, lifts a little past rest just behind the front, and settles about a second later. The restrained presets clamp a negative flatten to nothing and draw a ring of under a point, which is why this sheet is not on `LensedRoundedView`.

The label is the face's, and it answers the pointer as the lens button's does: white at rest, 65% of that under the pointer and 25% while held. The glass under it does not light up.

## The gradient sheet's knobs

`blur_level` is the blur in the middle of the sheet and `gradient_blur_edge` the blur at its rim. `gradient_blur_edge_width` is how far in from the rim, as a fraction of the sheet's size, the ramp between them runs, and `gradient_blur_power` shapes that ramp — below 1 the sharp band hugs the rim, above 1 it reaches further in. The controls panel writes them to the sheet whether or not it is open, so open it first to watch.

Every other knob of the material — tint, seal, rim, shadow — is the family's, and Glass > Overview explains each of them.",
        subject: "",
        feature: None,
        controls: &[
            // The two blur steps are 0.001 rather than 0.05. The panel's
            // slider floors onto its step grid, and 4.35 / 0.05 comes out a
            // hair under 87 in floating point, so at 0.05 the panel showed
            // 4.30 and would have written it on the first touch. The test
            // below pushes every default through that round trip.
            Control { label: "Blur level", target: "gradient_sheet", kind: ControlKind::Number { prop: "draw_bg.blur_level",               min: 0.,   max: 6.,  step: 0.001, default: 4.35 } },
            Control { label: "Edge blur",  target: "gradient_sheet", kind: ControlKind::Number { prop: "draw_bg.gradient_blur_edge",       min: 0.,   max: 6.,  step: 0.001, default: 1.45 } },
            Control { label: "Edge width", target: "gradient_sheet", kind: ControlKind::Number { prop: "draw_bg.gradient_blur_edge_width", min: 0.01, max: 0.5, step: 0.01, default: 0.20 } },
            Control { label: "Edge curve", target: "gradient_sheet", kind: ControlKind::Number { prop: "draw_bg.gradient_blur_power",      min: 0.1,  max: 3.,  step: 0.05, default: 0.75 } },
        ],
        on_actions: Some(glass_popups_actions),
    },
    Story {
        key: "containers/glass/floating-surface",
        category: "Containers",
        component: "Glass",
        also: &["GlassFloatingSurface", "FloatingSurface"],
        name: "Floating surface",
        dsl: "GlassFloatingSurfaceOverview",
        added: "2026-09-10",
        tags: &["new", "layout"],
        doc: "# GlassFloatingSurface

The panel's material with a frame you can take hold of: a sheet of glass that floats over the page, **moved by its body and sized by its edges and its corners**.

## The two gestures are claimed at opposite ends

The frame claims a press *before* the surface's own contents see it. The grab band is a few points wide and lies over whatever was put against the edge, so a resize that begins by dropping a caret into a field is a resize you then have to undo. `grab_margin` sets its width, and it reaches both ways from the edge — the surface is a rounded rectangle, and a band that stopped at the boundary would ask for a press on glass that is not there.

The move is claimed the other way round, *after* the contents have had their turn. The press's handled mark is read once before the contents run and once after, and the only handler between those two reads is the surface's own subtree — so a press a button on the glass took is told apart from a press on a control sitting **behind** the glass, which had marked the event handled long before the surface was reached at all. The first leaves the surface where it is; the second still moves it. Only what nothing inside wanted moves the surface, which is why there is no title bar: a strip of chrome across the top is exactly what this family exists not to draw.

**A press another widget already answered is not the surface's.** It floats in window points, so it can lie over panes that are asked about a press before the pane it lives in — a navigator, a splitter bar. The press belongs to whoever took it first, so a drag cannot be *started* on the part of the sheet that overlaps one; once a drag has begun it carries on anywhere, because nothing else holds the pointer. On this shell the cost is not just a drag that fails to start: the navigator answers that press by opening a different story, which tears down the page the surface is standing on. A sheet that let a press through would be answering with a widget nobody can see, so it claims a press on itself whether or not `movable` and `resizable` are on, and whichever button made it. Every press it answers also takes the key focus, so a search box elsewhere stops eating keys the moment the sheet is worked. Closing asks where the caret *is*, not who put it there: one anywhere the surface draws goes back to whatever the surface took it from, or is simply dropped where the surface never took it; one that has since moved off the surface is left where it is, which on this page is the button that just asked it to close.

**The pointer and the wheel go with the press.** A hover over the sheet is claimed at the same two places a press is, so a field under the glass does not light up and offer a caret for a click it will never get; and a wheel is stopped once the surface's own body has had it, so the page underneath does not slide out from under a sheet that stays put.

**The grab band costs a ring of page.** `grab_margin` reaches outward as well as inward, so presses that far outside the painted glass belong to the surface, with nothing drawn there to explain it. The outward half is not optional — a rounded rectangle's corners are unpainted, and a band that stopped at the boundary would ask for a press on glass that is not there — but it is a reason to keep the number small. And the gesture is mouse only: it is written against the mouse events rather than `Event::TouchUpdate`, so this widget is desktop only until a touch path is written.

## Why a resizable glass surface is harder than a resizable panel

The lens reads the scene **behind** the surface, from a capture the window takes only when a draw asks for one — and the ask happens inside the surface's own draw. A repaint that reuses the drawn content asks for nothing, the window stops capturing, and the glass goes on showing the page as it was when the drag began. So every frame of a move or a resize redraws the surface's whole subtree. Drag it across the coloured band on this page and the band bends through it as it crosses; that is the capture being re-taken, not a still picture being carried around.

The same reasoning is why coming up is not just a redraw. The window decides whether to capture before any widget draws, so a surface that appears — `open()`, a `place()` while it is up, or a page writing `shown: true` — announces itself first; without that its first painted frame carries the flat fallback face and the window then redraws the whole UI to correct itself.

## The frame

`shown` is a live property, so a page can put one up by declaring it — and a theme switch or a live edit brings back what the page declared, not what you last pressed. `min_size` and `max_size` hold the size; a zero side of `max_size` means the window is the only ceiling. Dragging a near edge past the floor pins **that** edge and leaves the far one where it was — clamping the position instead would shove the far edge along, quietly moving a surface you were only trying to make smaller. Every draw pulls the whole frame back inside the window, because a surface whose frame had gone past an edge could never be dragged back.

`Sizing` and `Moving` arrive on every frame of a drag and `Placed` once, when the hand comes off; `framed` answers whichever of the three came this pass. Position is reported, never stored — a caller that wants it back next run keeps the value itself.",
        subject: "floater",
        feature: None,
        controls: &[
            // `grab_margin` caps at 16, which is also the library's own
            // exposed maximum: the band reaches that far OUTSIDE the glass
            // as well as inside, and a ring of page that wide answering to a
            // surface, with nothing drawn there to say so, is more than a
            // reader will forgive.
            Control { label: "Grab margin", target: "floater", kind: ControlKind::Number { prop: "grab_margin", min: 2.,  max: 16., step: 1., default: 8. } },
            Control { label: "Corner mark", target: "floater", kind: ControlKind::Number { prop: "grip_size",   min: 0.,  max: 48., step: 1., default: 24. } },
            Control { label: "Movable",     target: "floater", kind: ControlKind::Bool   { prop: "movable",   default: true } },
            Control { label: "Resizable",   target: "floater", kind: ControlKind::Bool   { prop: "resizable", default: true } },
            // The material, reached on the surface's own glass panel.
            // `content` is one id segment, found by the same subtree search
            // that already reaches `inside` three levels deeper.
            Control { label: "Blur level",  target: "content", kind: ControlKind::Number { prop: "draw_bg.blur_level",   min: 0., max: 6.,   step: 0.1,   default: 5.2 } },
            Control { label: "Tint amount", target: "content", kind: ControlKind::Number { prop: "draw_bg.tint_alpha",   min: 0., max: 0.30, step: 0.002, default: 0.08 } },
            Control { label: "Shadow",      target: "content", kind: ControlKind::Number { prop: "draw_bg.shadow_alpha", min: 0., max: 1.,   step: 0.05,  default: 1.0 } },
        ],
        on_actions: Some(glass_floating_surface_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Cx` with this crate's theme, the shared page templates and this
    /// file's own stories registered, and nothing else: a failure here is
    /// this page's, not some other story's.
    fn shell() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            // Registering a template compiles nothing; making an instance
            // out of one does. Clearing here keeps any other module's
            // complaint out of these tests' answers.
            let _ = makepad_platform::shader_error::take();
        });
        cx
    }

    fn build(cx: &mut Cx, dsl: &str) -> WidgetRef {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {dsl}");
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// Every page is built from the DSL, which the Rust compiler never
    /// reads, and a shader that fails to compile is not an error anywhere —
    /// the draw is skipped and the widget paints nothing. Building each and
    /// asking for every id the controls panel writes to is what turns either
    /// mistake into a failed build. The floating surface's `content` is the
    /// target worth the trouble: it is three levels inside another widget,
    /// reached by the same single-segment subtree search. A Number control
    /// whose default lies outside its own range would sit on a slider it
    /// cannot reach, and one whose default is not on its step grid in
    /// floating point comes back a step low: the panel's slider floors, and
    /// 4.35 at step 0.05 read 4.30 on the panel.
    #[test]
    fn every_page_builds_and_its_targets_can_be_reached() {
        let mut cx = shell();
        for story in STORIES {
            let page = build(&mut cx, story.dsl);
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(
                makepad_platform::shader_error::take(),
                None,
                "{}: a draw shader failed to compile",
                story.key
            );
            for target in std::iter::once(story.subject)
                .chain(story.controls.iter().map(|c| c.target))
                .filter(|t| !t.is_empty())
            {
                assert!(
                    !page.widget(&cx, &[LiveId::from_str(target)]).is_empty(),
                    "{}: no widget at {}",
                    story.key,
                    target
                );
            }
            for control in story.controls {
                if let ControlKind::Number { min, max, step, default, .. } = control.kind {
                    assert!(
                        (min..=max).contains(&default),
                        "{}: {} defaults to {default}, outside {min}..{max}",
                        story.key,
                        control.label
                    );
                    // The panel hands the default to a Slider, whose Linear
                    // taper floors onto the step grid on the way back out.
                    let travel = taper_to_travel(SliderTaper::Linear, default, min, max, default, step);
                    let shown = taper_to_value(SliderTaper::Linear, travel, min, max, default, step);
                    assert!(
                        (shown - default).abs() < 1e-9,
                        "{}: {} defaults to {default} but the panel's slider would show {shown}",
                        story.key,
                        control.label
                    );
                }
            }
        }
    }

    /// The overview's last band is one row of four. The stage hands out a
    /// slot that flows Down, so a merge that silently failed would stack
    /// them.
    #[test]
    fn the_overview_row_band_still_flows_right() {
        let mut cx = shell();
        let page = build(&mut cx, "GlassSurfacesOverview");
        let stage = page.widget(&cx, &[live_id!(rounded)]);
        assert!(!stage.is_empty(), "the last band is named on the page");
        let slot = stage
            .borrow::<View>()
            .expect("a stage is a View")
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(body))
            .map(|(_, w)| w.clone())
            .expect("the stage has a body");
        assert!(
            matches!(
                slot.borrow::<View>().expect("the slot is a View").layout.flow,
                Flow::Right { .. }
            ),
            "the row band overrode the stage's Down flow"
        );
    }

    /// Every id the popups page's handler reaches, by the same single-segment
    /// subtree search the handler uses. A PopupNotification derefs to a
    /// View, so the search reaches inside a closed sheet as well; and the
    /// press demo has to be its own widget, or the handler's `borrow_mut`
    /// finds nothing and the lens never moves.
    #[test]
    fn the_popups_page_names_every_part_the_handler_reaches() {
        let mut cx = shell();
        let page = build(&mut cx, "GlassSurfacesPopups");
        for id in [
            "open_blur_sheet", "open_lens_sheet", "open_press_lens", "open_gradient_sheet",
            "blur_popup", "blur_sheet", "blur_sheet_blur", "blur_sheet_lensing", "close_blur_popup",
            "lens_popup", "lens_sheet", "lens_sheet_blur", "lens_sheet_lensing", "close_lens_popup",
            "press_demo", "press_lens_popup", "press_lens", "press_lens_face",
            "gradient_popup", "gradient_sheet", "close_gradient_popup",
        ] {
            assert!(
                !page.widget(&cx, &[LiveId::from_str(id)]).is_empty(),
                "no widget at {id}"
            );
        }
        let demo = page.widget(&cx, &[live_id!(press_demo)]);
        assert!(
            demo.borrow::<StoryPressLens>().is_some(),
            "press_demo does not borrow as a StoryPressLens"
        );
    }

    /// The host adds one thing to the library's press: a click puts the sheet
    /// away, but only once the rebound has played. The face reports the click
    /// as a release too, and neither report may close the sheet early or
    /// start the rebound over.
    #[test]
    fn a_click_closes_the_sheet_once_the_rebound_has_played() {
        let mut cx = shell();
        let page = build(&mut cx, "GlassSurfacesPopups");
        let demo_ref = page.widget(&cx, &[live_id!(press_demo)]);
        let mut demo = demo_ref.borrow_mut::<StoryPressLens>().expect("press_demo is the host");
        let open = |demo: &StoryPressLens, cx: &Cx| demo.view.popup_notification(cx, ids!(press_lens_popup)).is_open();

        demo.open_fresh(&mut cx);
        assert!(open(&demo, &cx));
        demo.press(&mut cx);
        demo.tick(&mut cx, 1.0);
        demo.tick(&mut cx, 1.3);
        demo.release(&mut cx, false);
        demo.release(&mut cx, true);
        assert!(open(&demo, &cx), "closed on the click instead of after the rebound");

        let stops = LensPressCurve::default().release_stops_at();
        demo.tick(&mut cx, 1.35);
        demo.tick(&mut cx, 1.35 + stops * 0.5);
        assert!(open(&demo, &cx), "closed half way through the rebound");
        demo.tick(&mut cx, 1.35 + stops);
        assert!(!open(&demo, &cx), "the rebound played out and the sheet stayed");
        assert!(demo.press.is_at_rest());
    }
}
