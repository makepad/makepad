//! The pointer glass page under Containers > Material: a modifier any view
//! can carry, a sheet of glass over the view that rings like water where the
//! pointer moves across it and bends what lies under it; shown on a panel,
//! a picture, a plain view, a screen and a meter, and on the stock screen
//! and meter as the active sheet dresses them.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let ScreenCaption = Label{
        draw_text +: {color: theme.color_screen_ink text_style: theme.font_regular{font_size: theme.font_size_p * 0.8}}
    }
    let PanelTitle = Label{
        draw_text +: {text_style: theme.font_bold{font_size: theme.font_size_p * 1.2}}
    }
    let PanelText = Label{
        width: Fill
        draw_text +: {text_style: theme.font_regular{font_size: theme.font_size_p}}
    }

    // The page's own water, so it shows whatever sheet is on: `Ripple.sum`
    // for the water, `Ripple.lay` for clear glass over it. The inputs each
    // glass below declares are the ones the pointer glass fills: the trail
    // of drops and its life, and the window's scene of the frame while the
    // glass is lifted over what it covers.

    // Any view: the glass covers the view, its corner `face_radius`.
    let WaterPanel = RoundedView{
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_high border_radius: 10.}
        field_reach: 2.0
        trail_life: 1.6
        glass_backdrop: true
        draw_glass +: {
            trail_0: instance(vec3(0.0, 0.0, -1.0))
            trail_1: instance(vec3(0.0, 0.0, -1.0))
            trail_2: instance(vec3(0.0, 0.0, -1.0))
            trail_3: instance(vec3(0.0, 0.0, -1.0))
            trail_4: instance(vec3(0.0, 0.0, -1.0))
            trail_5: instance(vec3(0.0, 0.0, -1.0))
            trail_6: instance(vec3(0.0, 0.0, -1.0))
            trail_7: instance(vec3(0.0, 0.0, -1.0))
            trail_life: uniform(1.6)
            face_radius: uniform(10.0)
            scene_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let edge_d = Material.sd_box(p, c, c, min(self.face_radius, min(c.x, c.y)))
                let w = Ripple.sum(p, edge_d, self.trail_life, self.trail_0, self.trail_1, self.trail_2, self.trail_3, self.trail_4, self.trail_5, self.trail_6, self.trail_7)
                var under = vec3(0.0, 0.0, 0.0)
                if self.has_gauss > 0.5 {
                    // What the view shows under the glass, seen through the
                    // water: displaced, never from past its edge.
                    let ps = clamp(p + w.xy, vec2(0.0, 0.0), self.rect_size)
                    let sq = (self.rect_pos + ps) / max(self.source_size, vec2(1.0, 1.0))
                    under = self.scene_texture.sample(vec2(sq.x, mix(sq.y, 1.0 - sq.y, self.source_y_flip))).rgb
                }
                return Ripple.lay(under, self.has_gauss, w.zw, 0.0, Finish.cover(edge_d + px, px))
            }
        }
    }

    // A screen: the glass is the screen's own, over its face inside the
    // bezel, with the theme's quiet sheen.
    let WaterScreen = ScreenView{
        field_reach: 2.0
        trail_life: 1.6
        glass_backdrop: true
        draw_glass +: {
            trail_0: instance(vec3(0.0, 0.0, -1.0))
            trail_1: instance(vec3(0.0, 0.0, -1.0))
            trail_2: instance(vec3(0.0, 0.0, -1.0))
            trail_3: instance(vec3(0.0, 0.0, -1.0))
            trail_4: instance(vec3(0.0, 0.0, -1.0))
            trail_5: instance(vec3(0.0, 0.0, -1.0))
            trail_6: instance(vec3(0.0, 0.0, -1.0))
            trail_7: instance(vec3(0.0, 0.0, -1.0))
            trail_life: uniform(1.6)
            scene_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let lo = vec2(Finish.snap(self.rect_pos.x, px), Finish.snap(self.rect_pos.y, px)) - self.rect_pos
                let hi = vec2(Finish.snap(self.rect_pos.x + self.rect_size.x, px), Finish.snap(self.rect_pos.y + self.rect_size.y, px) - px) - self.rect_pos
                let c = (lo + hi) * 0.5
                let ho = (hi - lo) * 0.5
                let r_out = min(self.face_radius, min(ho.x, ho.y))
                let b = Finish.snap(max(self.bezel, 0.0), px)
                let hf = max(ho - vec2(b, b), vec2(0.5, 0.5))
                let face_d = Material.sd_box(p, c, hf, max(r_out - b, 0.0))
                // The theme's quiet glass: the diagonal band and the line
                // under the top edge.
                let u = (p - (c - hf)) / max(hf * 2.0, vec2(1.0, 1.0))
                let diag = u.x + u.y * 0.6
                let band = smoothstep(0.0, 0.12, diag) * (1.0 - smoothstep(0.28, 0.5, diag))
                let top = Finish.cover(abs(p.y - (c.y - hf.y) - px * 1.5) - px * 0.5, px) * 0.6
                let sheen = clamp(self.sheen * (band + top), 0.0, 1.0)
                let w = Ripple.sum(p, face_d, self.trail_life, self.trail_0, self.trail_1, self.trail_2, self.trail_3, self.trail_4, self.trail_5, self.trail_6, self.trail_7)
                var under = vec3(0.0, 0.0, 0.0)
                if self.has_gauss > 0.5 {
                    // The content under the glass, seen through the water:
                    // displaced, never from past the face.
                    let ps = clamp(p + w.xy, c - hf, c + hf)
                    let sq = (self.rect_pos + ps) / max(self.source_size, vec2(1.0, 1.0))
                    under = self.scene_texture.sample(vec2(sq.x, mix(sq.y, 1.0 - sq.y, self.source_y_flip))).rgb
                }
                return Ripple.lay(under, self.has_gauss, w.zw, sheen, Finish.cover(face_d + px, px))
            }
        }
    }

    // A meter: the glass over its face, two points inside it, so its rim
    // stays still whatever sheet drew it.
    let WaterMeter = NeedleMeter{
        field_reach: 2.0
        trail_life: 1.6
        glass_backdrop: true
        draw_glass +: {
            trail_0: instance(vec3(0.0, 0.0, -1.0))
            trail_1: instance(vec3(0.0, 0.0, -1.0))
            trail_2: instance(vec3(0.0, 0.0, -1.0))
            trail_3: instance(vec3(0.0, 0.0, -1.0))
            trail_4: instance(vec3(0.0, 0.0, -1.0))
            trail_5: instance(vec3(0.0, 0.0, -1.0))
            trail_6: instance(vec3(0.0, 0.0, -1.0))
            trail_7: instance(vec3(0.0, 0.0, -1.0))
            trail_life: uniform(1.6)
            scene_texture: texture_2d(float)
            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let hf = max(c - vec2(2.0, 2.0), vec2(0.5, 0.5))
                let face_d = Material.sd_box(p, c, hf, max(min(self.face_radius, min(c.x, c.y)) - 2.0, 0.0))
                let w = Ripple.sum(p, face_d, self.trail_life, self.trail_0, self.trail_1, self.trail_2, self.trail_3, self.trail_4, self.trail_5, self.trail_6, self.trail_7)
                var under = vec3(0.0, 0.0, 0.0)
                if self.has_gauss > 0.5 {
                    let ps = clamp(p + w.xy, c - hf, c + hf)
                    let sq = (self.rect_pos + ps) / max(self.source_size, vec2(1.0, 1.0))
                    under = self.scene_texture.sample(vec2(sq.x, mix(sq.y, 1.0 - sq.y, self.source_y_flip))).rgb
                }
                return Ripple.lay(under, self.has_gauss, w.zw, 0.0, Finish.cover(face_d + px, px))
            }
        }
    }

    mod.stories.MaterialPointerGlass = StoryPage{
        StoryNote{text: "Glass that answers the pointer the way water does, as a modifier any view can carry. Move the pointer across any of these: wherever it passes it leaves drops, each drop sends a ring out across the glass, and what lies under the glass, text, a picture, digits or a needle, bends through the rings and settles again. Stop moving and the water stills; nothing is drawn or asked for until the pointer moves again."}
        StoryHeading{text: "On a panel, a picture and a plain view"}
        StoryRow{
            subject := WaterPanel{
                width: 240.
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_3
                PanelTitle{text: "Tide table"}
                PanelText{text: "High water 06:12 and 18:40, low water 00:05 and 12:27. Move the pointer across this card."}
            }
            WaterPanel{
                width: 200.
                height: 130.
                padding: 0.
                draw_bg +: {color: #0000}
                Image{width: Fill height: Fill fit: ImageFit.CropToFill src: crate_resource("self:resources/photo_landscape.jpg")}
            }
            WaterPanel{
                width: 150.
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_2
                draw_bg +: {color: #0000}
                PanelText{text: "INPUT"}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.72}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.58}
                LevelMeter{width: Fill height: 6. lamp: false level: 0.31}
            }
        }

        StoryHeading{text: "On a screen and a meter"}
        StoryNote{text: "A screen's glass is its view's pointer glass, drawn by the screen inside its bezel; a meter carries the same glass over its face. Each keeps its own water: a ring stays inside the glass it was dropped in and dies out before the edge, which stays still."}
        StoryRow{
            WaterScreen{
                ScreenCaption{text: "OUTPUT"}
                Readout{text: "-6.0" cells: 5 digit_height: 30.}
                ScreenCaption{text: "dB"}
            }
            WaterMeter{width: 200. height: 120. value: 0.62 label: "VU"}
            WaterScreen{flow: Right spacing: theme.space_2 align: Align{y: 1.} Readout{text: "440.0" cells: 6 digit_height: 24.} ScreenCaption{text: "Hz"}}
        }

        StoryHeading{text: "In the sheet's own glass"}
        StoryNote{text: "The stock screen and meter, as the active sheet dresses them. Whether their glass rings is the sheet's choice: a sheet that turns the pointer glass on for its screens, readouts and meters gives it a glass of its own; the stock look leaves it off, and these stay still."}
        StoryRow{
            ScreenView{
                ScreenCaption{text: "OUTPUT"}
                Readout{text: "-6.0" cells: 5 digit_height: 30.}
                ScreenCaption{text: "dB"}
            }
            NeedleMeter{width: 200. height: 120. value: 0.62 label: "VU"}
            ScreenView{flow: Right spacing: theme.space_2 align: Align{y: 1.} Readout{text: "12:45" digit_height: 24.} ScreenCaption{text: "PM"}}
        }

        StoryHeading{text: "Rings without the backdrop"}
        StoryNote{text: "With glass_backdrop off the glass cannot read what lies under it, so the rings are light and shade on the glass alone and what is under it stays put. Nothing is captured for it."}
        StoryRow{
            WaterScreen{glass_backdrop: false Readout{text: "-12.0" cells: 5 digit_height: 30.}}
            WaterPanel{
                glass_backdrop: false
                width: 200.
                height: Fit
                padding: theme.mspace_3
                PanelText{text: "Light and shade only."}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/material/pointer-glass",
    category: "Containers",
    component: "Material",
    also: &["View", "ScreenView", "NeedleMeter"],
    name: "Pointer glass",
    dsl: "MaterialPointerGlass",
    added: "2026-10-14",
    tags: &["pointer", "water", "glass", "modifier", "new"],
    doc: "# Pointer glass

Glass over a view that rings like water where the pointer moves across it, and bends what lies under it: a modifier every view carries, off until it is given a reach.

- `field_reach` above zero gives the view a sheet of glass, `draw_glass`, drawn over the view and everything in it, answering the pointer within that many points of the view's edge. Zero, the default, draws no glass and costs nothing.
- The glass's shader gets the pointer's field and the trail of drops the pointer leaves as it moves, as inputs it declares: `pointer_field` (vec4 instance), `trail_0` to `trail_7` (vec3 instances, each a drop's place in points from the glass's top left and its age in seconds, newest first, -1 for none) and `trail_life` (uniform). A pointer that does not move drops nothing.
- `trail_spacing` is how far the pointer moves between drops, `trail_life` how long a drop lives.
- `glass_backdrop` lifts the glass over the view while the trail is alive, with the window's scene of the frame bound (`scene_texture`, `source_size`, `source_y_flip`, `has_gauss`), so its shader can read what is under it and draw it displaced. A view drawn on an overlay of its own (a `backdrop` view) cannot be read: its rings are light and shade only.
- At rest nothing runs: no frames, no capture.
- `Ripple.sum` turns the drops into water (the displacement and the slope at a point), `Ripple.lay` draws clear glass over it; a sheet with a look of its own lays its own light and shade from the slope.

A `ScreenView`'s glass is its view's pointer glass, drawn by the screen inside its bezel; a `NeedleMeter` carries the same glass over its face. A sheet turns it on with a few lines, for example for its screens (with `trail_1` to `trail_7` declared as `trail_0` is, and `has_gauss`, `source_size` and `source_y_flip` as uniforms):

```
mod.widgets.ScreenView.field_reach = 2.0
mod.widgets.ScreenView.glass_backdrop = true
mod.widgets.ScreenView.draw_glass.trail_0 = instance(vec3(0.0, 0.0, -1.0))
mod.widgets.ScreenView.draw_glass.trail_life = uniform(1.3)
mod.widgets.ScreenView.draw_glass.scene_texture = texture_2d(float)
mod.widgets.ScreenView.draw_glass.pixel = fn() {
    let w = Ripple.sum(p, face_d, self.trail_life, self.trail_0, ...)
    ...
}
```

The shaders on this page are the page's own, so the water shows under every sheet.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Reach", target: "subject", kind: ControlKind::Number { prop: "field_reach", min: 0., max: 24., step: 0.5, default: 2. } },
        Control { label: "Drop spacing", target: "subject", kind: ControlKind::Number { prop: "trail_spacing", min: 1., max: 40., step: 0.5, default: 6. } },
        Control { label: "Drop life (s)", target: "subject", kind: ControlKind::Number { prop: "trail_life", min: 0.2, max: 4., step: 0.05, default: 1.6 } },
    ],
    on_actions: None,
}];
