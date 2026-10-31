//! Original Stage console controls, extracted from donors/stage/src/main.rs.
//! ChromeButton: 421; parameter/knob/fader/tick/panel family: 579–843;
//! crossfader: 1317. Drawing math and dimensions are the authored originals.
//! Rotary is a Slider template, so all controls retain the Slider binding API.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.StageButton = Button{
        draw_bg +: {
            color: stage.control
            color_focus: stage.control
            color_hover: stage.control_hover
            color_down: stage.control_down
            border_color: stage.border_hi
            // ONE RADIUS: match the dropdown chrome (theme 2.5).
            border_radius: 2.5
            border_size: 1.0
        }
        draw_text +: {
            color: stage.label
            color_focus: stage.label
            color_hover: stage.text_hi
            text_style: theme.font_regular{font_size: 10}
        }
    }

    mod.widgets.StageHSlider = Slider{
        width: Fill
        height: 22
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            // The quad is the box: a style sheet grows its slider family's quad for shadows, and this pixel draws to the box, so under a sheet it would scale up.
            vertex: fn() {self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)}
            body_color: uniform(stage.groove)
            track_color: uniform(stage.track)
            fill_color: uniform(stage.accent)
            cap_color: uniform(stage.text)
            inert: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let track_h = 6.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(1., track_y, self.rect_size.x - 2., track_h, 3.)
                sdf.fill(self.track_color)
                // Inset travel: the cap never clips at 0% / 100%.
                let cap_w = 10.
                let left = 1. + cap_w * 0.5
                let w = self.rect_size.x - 2. - cap_w
                let fill_w = max(1., w * self.slide_pos + cap_w * 0.5)
                sdf.box(1., track_y, fill_w, track_h, 3.)
                sdf.fill(self.fill_color)
                let cap_x = left + w * self.slide_pos - cap_w * 0.5
                sdf.box(cap_x, 3., cap_w, self.rect_size.y - 6., 3.)
                sdf.fill(self.cap_color)
                return sdf.result * (1.0 - self.inert * 0.72)
            }
        }
    }

    mod.widgets.StageBipolarSlider = Slider{
        width: Fill
        height: 22
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            // The quad is the box: a style sheet grows its slider family's quad for shadows, and this pixel draws to the box, so under a sheet it would scale up.
            vertex: fn() {self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)}
            body_color: uniform(stage.groove)
            track_color: uniform(stage.track)
            fill_color: uniform(stage.accent)
            cap_color: uniform(stage.text)
            centre_color: uniform(stage.border_mid)
            inert: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let track_h = 6.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(1., track_y, self.rect_size.x - 2., track_h, 3.)
                sdf.fill(self.track_color)
                let cap_w = 10.
                let left = 1. + cap_w * 0.5
                let w = self.rect_size.x - 2. - cap_w
                let lo = min(self.slide_pos, 0.5)
                let hi = max(self.slide_pos, 0.5)
                sdf.box(left + w * lo, track_y, max(1., w * (hi - lo)), track_h, 3.)
                sdf.fill(self.fill_color)
                sdf.rect(left + w * 0.5 - 0.5, track_y - 2., 1., track_h + 4.)
                sdf.fill(self.centre_color)
                let cap_x = left + w * self.slide_pos - cap_w * 0.5
                sdf.box(cap_x, 3., cap_w, self.rect_size.y - 6., 3.)
                sdf.fill(self.cap_color)
                return sdf.result * (1.0 - self.inert * 0.72)
            }
        }
    }

    mod.widgets.StageKnob = Rotary{
        width: 44
        height: 44
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
        draw_bg +: {
            // The quad is the box: a style sheet grows its slider family's quad for shadows, and this pixel draws to the box, so under a sheet it would scale up.
            vertex: fn() {self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)}
            body_color: uniform(stage.groove)
            body_color_hover: uniform(stage.track)
            rim_color: uniform(stage.border_strong)
            ring_color: uniform(stage.control_top)
            val_color: uniform(stage.accent)
            pointer_color: uniform(stage.text_bright)
            // Slot dials the loaded effect does not declare dim to inert
            // (fixed dial count keeps MIDI maps stable; the dimming keeps
            // the strip honest about which ones do anything).
            inert: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                // A point inside the quad's edge: the ring's outer edge
                // used to sit ON the edge, and the quad's fractional
                // pixel position then cut the ring's antialiased rim off
                // flat on one side -- the right, on the rack's mix knob.
                let r = min(self.rect_size.x, self.rect_size.y) * 0.5 - 1.0
                // Sdf2d arcs: angle 0 points DOWN, increasing = clockwise.
                let start = PI * 0.25
                let sweep = PI * 1.5
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep, 2.5)
                sdf.fill(self.ring_color)
                let lit = max(self.slide_pos, 0.01)
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep * lit, 2.5)
                sdf.fill(self.val_color)
                sdf.circle(c.x, c.y, r - 7.5)
                sdf.fill_keep(self.body_color.mix(self.body_color_hover, max(self.hover, self.drag)))
                sdf.stroke(self.rim_color, 1.0)
                let a = start + sweep * self.slide_pos
                let d = vec2(-sin(a), cos(a))
                let p0 = c + d * (r - 13.5)
                let p1 = c + d * (r - 8.5)
                sdf.move_to(p0.x, p0.y)
                sdf.line_to(p1.x, p1.y)
                sdf.stroke(self.pointer_color, 2.0)
                return sdf.result * (1.0 - self.inert * 0.72)
            }
        }
    }

    mod.widgets.StageIconButton = ButtonIcon{
        width: 24
        height: 22
        padding: 0
        // ONE BUTTON FAMILY app-wide (the pager's well + glyph ratio):
        // 24x22 wells, 9-wide glyphs, comfortable padding everywhere.
        icon_walk: Walk{width: 9 height: Fit}
        draw_bg +: {
            color: stage.control
            color_focus: stage.control
            color_hover: stage.control_hover
            color_down: stage.control_down
            border_color: stage.border
            // ONE RADIUS: match the dropdown chrome (theme 2.5).
            border_radius: 2.5
            border_size: 1.0
        }
        draw_icon +: {
            color: stage.label
        }
    }

    mod.widgets.StageFader = Slider{
        // The wheel over a fader moves it, the ladder the clip decks'
        // sliders use.
        scroll_step: 0.025
        axis: DragAxis.Vertical
        width: 44
        height: 108
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
        draw_bg +: {
            // The quad is the box: a style sheet grows its slider family's quad for shadows, and this pixel draws to the box, so under a sheet it would scale up.
            vertex: fn() {self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)}
            body_color: uniform(stage.groove)
            track_color: uniform(stage.track)
            fill_color: uniform(stage.accent)
            cap_color: uniform(stage.text)
            cap_shadow: uniform(stage.muted)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(3., 2., self.rect_size.x - 6., self.rect_size.y - 4., 6.)
                sdf.fill(self.body_color)
                let top = 10.
                let bottom_pad = 10.
                let h = self.rect_size.y - top - bottom_pad
                let track_w = 8.
                let track_x = (self.rect_size.x - track_w) * 0.5
                sdf.box(track_x, top, track_w, h, 3.)
                sdf.fill(self.track_color)
                let fill_h = max(1., h * self.slide_pos)
                sdf.box(track_x + 1.5, top + (h - fill_h) + 1.5, track_w - 3., max(1., fill_h - 3.), 2.)
                sdf.fill(self.fill_color)
                let cap_h = 14.
                let cap_y = top + (h - fill_h) - cap_h * 0.5
                sdf.box(6., cap_y + 1.5, self.rect_size.x - 12., cap_h, 4.)
                sdf.fill(self.cap_shadow)
                sdf.box(5., cap_y, self.rect_size.x - 10., cap_h, 4.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    // A chip that drops a list of ticks (the library's FILTER and COLUMNS),
    // in the palette's colours. The two boxes are redrawn so the corner
    // radius is a uniform a style reload reaches. Donor views.rs VjDropToggles.
    mod.widgets.StageDropToggles = DropToggles{
        draw_bg +: {
            color: uniform(stage.control)
            color_hover: uniform(stage.control_top)
            border_color: uniform(stage.border_hi)
            border_color_lit: uniform(stage.accent)
            radius: uniform(2.5)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color.mix(self.color_hover, max(self.hover, self.open)))
                sdf.stroke(self.border_color.mix(self.border_color_lit, self.lit), 1.0)
                return sdf.result
            }
        }
        draw_icon +: { color: stage.label }
        draw_text +: { color: stage.label text_style: theme.font_regular{font_size: 10} }
        icon_color_lit: stage.accent
        draw_panel +: {
            color: stage.panel
            border_color: stage.border_hi
            radius: uniform(2.5)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_row +: { color_hover: uniform(stage.separator) }
        draw_mark +: {
            color_active: uniform(stage.accent)
            border_color: uniform(stage.border)
            border_color_hover: uniform(stage.veil_hi)
            border_color_active: uniform(stage.accent)
            tick_color: uniform(stage.panel)
        }
        draw_label +: {
            color: stage.muted
            color_hover: uniform(stage.label)
            color_active: uniform(stage.text_bright)
        }
    }

    mod.widgets.StageTick = Label{
        width: Fill
        draw_text.color: stage.secondary
        draw_text.text_style: theme.font_bold{font_size: 8}
    }

    mod.widgets.StagePanel = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 3
        padding: 7
        draw_bg +: {
            color: stage.panel_score
            border_color: stage.line
            border_size: 1.0
            border_radius: 3.0
        }
    }

    mod.widgets.StageXfader = Slider{
        width: Fill
        height: 44
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            // The quad is the box: a style sheet grows its slider family's quad for shadows, and this pixel draws to the box, so under a sheet it would scale up.
            vertex: fn() {self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)}
            body_color: uniform(stage.groove)
            track_color: uniform(stage.track)
            fill_color: uniform(stage.accent)
            cap_color: uniform(stage.text)
            cap_shadow: uniform(stage.muted)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(2., 6., self.rect_size.x - 4., self.rect_size.y - 12., 8.)
                sdf.fill(self.body_color)
                // The cap's CENTER travels an inset span, so the cap body
                // never clips at the extremes (cap 22 wide → 15px inset
                // each end keeps it fully inside the chrome).
                let left = 15.
                let right_pad = 15.
                let w = self.rect_size.x - left - right_pad
                let track_h = 10.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(left, track_y, w, track_h, 4.)
                sdf.fill(self.track_color)
                let fill_w = max(1., w * self.slide_pos)
                sdf.box(left + 1.5, track_y + 1.5, max(1., fill_w - 3.), track_h - 3., 3.)
                sdf.fill(self.fill_color)
                let cap_w = 22.
                let cap_x = left + fill_w - cap_w * 0.5
                sdf.box(cap_x + 1.5, 8., cap_w, self.rect_size.y - 16., 6.)
                sdf.fill(self.cap_shadow)
                sdf.box(cap_x, 6., cap_w, self.rect_size.y - 14., 6.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }
    true
}
