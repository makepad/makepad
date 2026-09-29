//! The grounds page under View: a panel's face replaced by a texture the
//! way a sheet replaces it, anchored to the panel so it moves with it.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // Each panel here carries the pixel function a sheet would write for
    // its template: the same body, on an instance so the page can show
    // several side by side. Every texture is laid in the panel's own
    // coordinates, `self.pos * self.rect_size`, so it is anchored to the
    // panel; the grain is hashed on the panel's own device pixels.
    //
    // A panel's colour can be translucent: the dark theme's container is a
    // shade of black laid over the ground, the light theme's a veil of
    // white. A factor on that colour scales nothing the eye sees, so the
    // texture is an offset in luminance laid on what shows
    // (`Finish.shade`, which needs the ground the panel stands on, here the
    // theme's), and the rim a factor on what shows (`Finish.scale`).

    let GroundCaption = Label{
        draw_text +: {text_style: theme.font_bold{} color: theme.color_on_surface_variant}
    }

    let GroundPanel = PanelView{
        width: 200.
        height: 110.
        flow: Down
        padding: theme.mspace_3
        spacing: theme.space_1
        draw_bg +: {
            /** the ground the panel stands on, for a translucent panel's texture */
            ground: uniform(theme.color_bg_app)
        }
    }

    mod.stories.GroundsOverview = StoryPage{
        StoryNote{text: "A sheet replaces the pixel function of PanelView, InsetPanelView and RoundedView like any stock face, and the window's own ground as well. These panels wear the recipes the reference briefs measured, each written the way a sheet writes it. The texture is laid in the panel's own coordinates, so it moves with the panel: scroll the page and the grain rides along instead of swimming under it. It is an offset laid on what shows, so it reads on the dark theme's translucent panels as well as on opaque ones."}
        StoryHeading{text: "Panel recipes"}
        StoryRow{
            subject := GroundPanel{
                draw_bg +: {
                    /** grain strength: the largest step either way, per device pixel; the spread is a 2.45th of it 0..0.05 step 0.001 */
                    grain: uniform(0.02)
                    pixel: fn() {
                        let p = self.pos * self.rect_size
                        let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                        let c = self.rect_size * 0.5
                        let d = Material.sd_box(p, c, c, 6.0)
                        var out = vec4(self.color.rgb * self.color.a, self.color.a)
                        // A long fall of two percent top to bottom, and
                        // monochrome grain, one value per device pixel.
                        let s = 0.01 - 0.02 * self.pos.y + Finish.grain(p / px, self.grain)
                        out = Finish.shade(out, s, self.ground.rgb)
                        // A device pixel of rim, 40 percent darker.
                        out = Finish.scale(out, 1.0 - 0.4 * Finish.band(d, 0.0, px, px))
                        return out * Finish.cover(d, px)
                    }
                }
                GroundCaption{text: "Grain"}
                Label{text: "A fall and grain."}
            }
            GroundPanel{
                draw_bg +: {
                    pixel: fn() {
                        let p = self.pos * self.rect_size
                        let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                        let c = self.rect_size * 0.5
                        let d = Material.sd_box(p, c, c, 6.0)
                        var out = vec4(self.color.rgb * self.color.a, self.color.a)
                        let s = (Finish.brushed(p, vec2(1.0, 0.0), 0.7) - 0.5) * 0.025 + Finish.grain(p / px, 0.008)
                        out = Finish.shade(out, s, self.ground.rgb)
                        out = Finish.scale(out, 1.0 - 0.4 * Finish.band(d, 0.0, px, px))
                        return out * Finish.cover(d, px)
                    }
                }
                GroundCaption{text: "Brushed"}
                Label{text: "Streaks along x."}
            }
            GroundPanel{
                draw_bg +: {
                    pixel: fn() {
                        let p = self.pos * self.rect_size
                        let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                        let c = self.rect_size * 0.5
                        let d = Material.sd_box(p, c, c, 6.0)
                        var out = vec4(self.color.rgb * self.color.a, self.color.a)
                        // A twill, and a vignette of a point and a half.
                        let v = length((self.pos - vec2(0.5, 0.4)) * vec2(1.0, 0.8))
                        let s = (Finish.weave(p, 3.0) - 0.5) * 0.02 + 0.0075 - 0.015 * smoothstep(0.1, 0.75, v)
                        out = Finish.shade(out, s, self.ground.rgb)
                        out = Finish.scale(out, 1.0 - 0.4 * Finish.band(d, 0.0, px, px))
                        return out * Finish.cover(d, px)
                    }
                }
                GroundCaption{text: "Weave"}
                Label{text: "A twill and a vignette."}
            }
        }

        StoryHeading{text: "Moving"}
        StoryNote{text: "The same grain panel in a row that scrolls sideways: the texture travels with the panel."}
        ScrollXView{
            width: Fill
            height: 130.
            flow: Right
            spacing: theme.space_3
            scroll_bars +: {scroll_bar_x +: {auto_hide: false}}
            View{width: 120. height: 10.}
            GroundPanel{
                draw_bg +: {
                    pixel: fn() {
                        let p = self.pos * self.rect_size
                        let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                        let c = self.rect_size * 0.5
                        let d = Material.sd_box(p, c, c, 6.0)
                        let out = vec4(self.color.rgb * self.color.a, self.color.a)
                        return Finish.shade(out, Finish.grain(p / px, 0.03), self.ground.rgb) * Finish.cover(d, px)
                    }
                }
                GroundCaption{text: "Coarse grain"}
            }
            View{width: 600. height: 10.}
        }

        StoryHeading{text: "The window's ground"}
        StoryNote{text: "A window clears to its pass colour and draws nothing else, so a sheet that lays a ground turns the window's own background on and gives it a pixel function: mod.widgets.Window.show_bg = true, then mod.widgets.Window.draw_bg.pixel = fn() { ... }. The window's draw_bg carries the theme's ground as color, and with show_bg left off, which every shipped sheet does, nothing changes."}
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "containers/view/grounds",
        category: "Containers",
        component: "View",
        also: &["PanelView", "InsetPanelView", "RoundedView"],
        name: "Grounds",
        dsl: "GroundsOverview",
        added: "2026-09-28",
        tags: &["texture", "grain", "brushed", "weave", "style sheet review", "new"],
        doc: "# Grounds\n\nWhat a sheet writes to texture a panel or the window.\n\n- A panel: `mod.widgets.PanelView.draw_bg.pixel = fn() { ... }` (and `RoundedView`, `InsetPanelView` inherits PanelView's). Lay the texture in `self.pos * self.rect_size`, the panel's own points, and hash grain on `p / px`, the panel's own device pixels: the texture then moves with the panel and never swims under it.\n- The window: `mod.widgets.Window.show_bg = true` and `mod.widgets.Window.draw_bg.pixel = fn() { ... }`. The window's `draw_bg.color` is the theme's `color_bg_app`, and with `show_bg` off, as every shipped sheet leaves it, the window only clears as it always did.\n\nThe texture is an offset in luminance laid on what shows, `Finish.shade(colour, offset, ground)`, and the rim a factor on what shows, `Finish.scale(colour, factor)`: a factor on the panel colour itself does nothing where the colour is a translucent shade, as the dark theme's containers are. The recipes here follow the measured briefs: grain of spread 0.006 to 0.012 per device pixel (`Finish.grain` with 2.45 times that), streaks and weaves at half that, a long fall of a couple of percent at most, and no spotlight.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Grain", target: "subject", kind: ControlKind::Number { prop: "draw_bg.grain", min: 0., max: 0.05, step: 0.001, default: 0.02 } },
        ],
        on_actions: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds_and_every_recipe_compiles() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
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
        assert_eq!(makepad_platform::shader_error::take(), None, "a ground recipe failed to compile");
        assert!(!page.widget(&cx, ids!(subject)).is_empty(), "no subject");
    }
}
