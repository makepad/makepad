//! The svg story: a vector drawing as a widget, a document that animates
//! itself with a shader per shape, and the frame loop it runs unless you say
//! otherwise.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.SvgOverview = StoryPage{
        StoryNote{text: "A vector drawing sized like any other widget. Seven places in this repository use one. It is not the Icon: an Icon wraps the same drawing in a background and its own icon_walk so it can sit inside a control, and this is the drawing on its own."}
        StoryNote{text: "Which one to use, against Vector and Icon, is set out on the Docs tab."}

        StoryHeading{text: "A drawing that moves, under the controls"}
        StoryNote{text: "The file animates itself. Sixty animate and animateTransform elements drive its paths, positions and opacity, and three symbols are placed forty-nine times through use, each instance tinted through currentColor. This is the one case where animating: true is right: the frame loop is what advances the document's clock, and switching it off freezes the scene where it is."}
        StoryRow{
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                ocean := Svg{
                    width: 600. height: 450.
                    animating: true
                    draw_svg +: {
                        svg: crate_resource("self:resources/ocean_dream.svg")

                        // Hash function for pseudo-random
                        hash21: fn(p: vec2) -> float {
                            let p3x = fract(p.x * 0.1031f);
                            let p3y = fract(p.y * 0.1031f);
                            let p3z = fract(p.x * 0.1031f);
                            let d = dot(vec3(p3x, p3y, p3z), vec3(p3y + 33.33f, p3z + 33.33f, p3x + 33.33f));
                            return fract((p3x + d + p3y + d) * (p3z + d))
                        }

                        // Voronoi distance for caustics
                        voronoi: fn(uv: vec2) -> float {
                            let g = floor(uv);
                            let f = fract(uv);
                            var md = 8.0f;
                            var i = -1.0f;
                            loop {
                                if i > 1.5f { break }
                                var j = -1.0f;
                                loop {
                                    if j > 1.5f { break }
                                    let o = vec2(i, j);
                                    let h1 = self.hash21(g + o);
                                    let h2 = self.hash21(g + o + vec2(17.0f, 31.0f));
                                    let r = o + vec2(h1, h2) - f;
                                    let d = dot(r, r);
                                    if d < md { md = d }
                                    j = j + 1.0f;
                                }
                                i = i + 1.0f;
                            }
                            return sqrt(md)
                        }

                        get_color: fn() {
                            let base = self.eval_gradient();
                            let id = self.v_shape_id;
                            let t = self.svg_time;

                            if id < 0.5f { return base }

                            // Unpremultiply base color so effects can work in linear RGB
                            let ba = max(base.a, 0.001f);
                            let bc = base.rgb / ba;

                            // Compute UV depending on gradient type:
                            // - Solid (type 0): bbox is in param1-4
                            // - Radial (type 2): center=param1,2 radius=param3,4
                            // - Linear (type 1): use v_world scaled to SVG viewport
                            let grad_type = self.v_param0;
                            var uv = vec2(0.0f, 0.0f);
                            if grad_type < 0.5f {
                                // Solid paint: bbox in params
                                let bmin = vec2(self.v_param1, self.v_param2);
                                let bmax = vec2(self.v_param3, self.v_param4);
                                let bsz = max(bmax - bmin, vec2(0.001f, 0.001f));
                                uv = (self.v_world - (bmin + bmax) * 0.5f) / (max(bsz.x, bsz.y) * 0.5f);
                            } else if grad_type > 1.5f {
                                // Radial gradient: center + radii in params
                                let center = vec2(self.v_param1, self.v_param2);
                                let radii = vec2(max(self.v_param3, 0.001f), max(self.v_param4, 0.001f));
                                uv = (self.v_world - center) / radii;
                            } else {
                                // Linear gradient: use world pos relative to gradient midpoint
                                let p0 = vec2(self.v_param1, self.v_param2);
                                let p1 = vec2(self.v_param3, self.v_param4);
                                let mid = (p0 + p1) * 0.5f;
                                let span = max(length(p1 - p0), 0.001f);
                                uv = (self.v_world - mid) / (span * 0.5f);
                            }

                            // ID 1: Jellyfish glow - soft ambient halo
                            if id < 1.5f {
                                let d = length(uv);
                                let glow = exp(-d * d * 2.0f);
                                let pulse = 0.8f + 0.2f * sin(t * 1.5f);
                                let out_rgb = bc * glow * pulse;
                                let out_a = ba * clamp(glow * pulse, 0.0f, 1.0f);
                                return vec4(out_rgb * out_a, out_a)
                            }

                            // ID 2: Jellyfish bell - dome with downward-flowing plasma
                            if id < 2.5f {
                                let d = length(uv);
                                // Dome-like implied normal from UV
                                let dome = max(1.0f - d * d, 0.0f);
                                let nz = sqrt(max(dome, 0.01f));
                                let normal = normalize(vec3(-uv.x, -uv.y, nz));
                                let view = vec3(0.0f, 0.0f, 1.0f);

                                // Animated light source
                                let light = normalize(vec3(sin(t * 1.1f) * 0.5f, cos(t * 0.7f) * 0.3f - 0.4f, 1.0f));
                                let diff = max(dot(normal, light), 0.0f) * 0.6f + 0.25f;

                                // Specular highlight
                                let half_v = normalize(light + view);
                                let spec = pow(max(dot(normal, half_v), 0.0f), 48.0f);

                                // Fresnel iridescence (rainbow shift at edges)
                                let ndv = max(dot(normal, view), 0.0f);
                                let fresnel = pow(1.0f - ndv, 2.0f);
                                let phase = ndv * 12.0f + t * 0.5f;
                                let iri = vec3(
                                    0.5f + 0.5f * sin(phase),
                                    0.5f + 0.5f * sin(phase + 2.09f),
                                    0.5f + 0.5f * sin(phase + 4.19f)
                                );

                                // Downward-flowing plasma effect
                                let flow_y = uv.y - t * 0.8f;
                                let plasma1 = sin(uv.x * 6.0f + flow_y * 8.0f) * 0.5f + 0.5f;
                                let plasma2 = sin(uv.x * 3.0f - flow_y * 5.0f + 1.5f) * 0.5f + 0.5f;
                                let plasma3 = sin((uv.x + uv.y) * 4.0f + flow_y * 6.0f + 2.8f) * 0.5f + 0.5f;
                                let plasma = plasma1 * 0.5f + plasma2 * 0.3f + plasma3 * 0.2f;

                                // Plasma is stronger toward edges (fresnel-like) and fades at center
                                let plasma_mask = fresnel * 0.6f + 0.15f;
                                let plasma_color = vec3(1.0f, 0.7f, 1.0f) * plasma * plasma_mask;

                                let out_rgb = bc * diff + plasma_color + iri * fresnel * 0.3f + vec3(1.0f, 1.0f, 1.0f) * spec * 0.8f;
                                return vec4(out_rgb * ba, ba)
                            }

                            // ID 3: Bubbles - glass sphere with thin-film rainbow
                            if id < 3.5f {
                                let d = length(uv);
                                let sphere = max(1.0f - d * d, 0.0f);
                                if sphere < 0.01f { return base }
                                let nz = sqrt(sphere);
                                let normal = normalize(vec3(-uv.x, -uv.y, nz));
                                let view = vec3(0.0f, 0.0f, 1.0f);
                                let ndv = max(dot(normal, view), 0.0f);

                                // Fresnel reflectance
                                let fresnel = 0.06f + 0.94f * pow(1.0f - ndv, 4.0f);

                                // Thin-film color bands
                                let thickness = ndv * 16.0f + t * 1.2f + d * 4.0f;
                                let film = vec3(
                                    0.5f + 0.5f * sin(thickness),
                                    0.5f + 0.5f * sin(thickness + 2.09f),
                                    0.5f + 0.5f * sin(thickness + 4.19f)
                                );

                                // Specular highlight (offset upward-left for 3D)
                                let light = normalize(vec3(0.3f, -0.5f, 1.0f));
                                let half_v = normalize(light + view);
                                let spec = pow(max(dot(normal, half_v), 0.0f), 80.0f);

                                // Ocean-tinted environment
                                let env = vec3(0.08f, 0.15f, 0.25f) + vec3(0.05f, 0.1f, 0.2f) * normal.y;

                                let out_rgb = env * fresnel + film * fresnel * 0.5f + vec3(1.0f, 1.0f, 1.0f) * spec * 1.5f;
                                let out_a = clamp(fresnel * 0.7f + spec + 0.05f, 0.0f, 1.0f) * ba;
                                return vec4(out_rgb * out_a, out_a)
                            }

                            // ID 4: Bioluminescent particles - pulsing glow
                            if id < 4.5f {
                                let d = length(uv);
                                let glow = exp(-d * d * 1.8f);
                                let pulse = 0.6f + 0.4f * sin(t * 4.0f + d * 3.0f);
                                // Bioluminescent green-cyan color
                                let bio_color = vec3(
                                    0.2f + 0.3f * sin(t * 1.5f),
                                    0.8f + 0.2f * sin(t * 0.7f + 1.0f),
                                    0.3f + 0.4f * sin(t * 1.1f + 2.5f)
                                );
                                let out_rgb = (bc + bio_color * 2.0f) * glow * pulse;
                                let out_a = clamp(glow * pulse * 1.5f, 0.0f, 1.0f) * ba;
                                return vec4(out_rgb * out_a, out_a)
                            }

                            // ID 5: Light rays - underwater caustic shimmer
                            if id < 5.5f {
                                // Use world-scaled UV for large-scale caustic pattern
                                let cuv = uv * 1.5f;
                                let v1 = self.voronoi(cuv + vec2(t * 0.12f, t * 0.08f));
                                let v2 = self.voronoi(cuv * 1.7f + vec2(-t * 0.15f, t * 0.1f));
                                let caustic = v1 * v2;
                                // Gentle brightness boost along the rays
                                let bright = pow(1.0f - caustic, 1.5f) * 0.5f;
                                let out_rgb = bc * (1.0f + bright);
                                return vec4(out_rgb * ba, ba)
                            }

                            // ID 6: Ocean background - 1D vertical light rays with y-fade
                            if id < 6.5f {
                                let wx = self.v_world.x * 0.012f;

                                // 1D voronoi on X axis only - creates vertical ray columns
                                // Use hash21 with y=0 to get 1D cell pattern
                                let v1 = self.voronoi(vec2(wx * 1.0f + t * 0.06f, 0.0f));
                                let v2 = self.voronoi(vec2(wx * 2.5f - t * 0.04f, 0.0f));
                                let v3 = self.voronoi(vec2(wx * 0.5f + t * 0.03f, 0.0f));

                                // Sharp bright lines where rays are
                                let c1 = pow(1.0f - v1, 4.0f);
                                let c2 = pow(1.0f - v2, 3.0f);
                                let c3 = pow(1.0f - v3, 2.5f);
                                let caustic = c1 * 0.5f + c2 * 0.3f + c3 * 0.2f;

                                // Vertical fade: strong at top, fading toward bottom
                                // uv.y goes from -1 (top) to +1 (bottom) for vertical gradient
                                let depth_fade = clamp(1.0f - (uv.y + 1.0f) * 0.5f, 0.0f, 1.0f);
                                let depth_fade2 = depth_fade * depth_fade * depth_fade;

                                // Add subtle vertical shimmer/movement
                                let wy = self.v_world.y * 0.008f;
                                let shimmer = sin(wy * 3.0f + t * 0.5f) * 0.15f + 0.85f;

                                let ray_color = vec3(0.4f, 0.7f, 1.0f);
                                let ray_strength = caustic * depth_fade2 * shimmer * 0.4f;

                                let out_rgb = bc + ray_color * ray_strength;
                                return vec4(out_rgb * ba, ba)
                            }

                            // ID 7: Tentacle light rays - animated pulses moving down the stroke
                            if id < 7.5f {
                                let sd = self.v_stroke_dist;

                                // Multiple light pulses traveling downward along the tentacle
                                // sd increases along the path length
                                let pulse_speed = 80.0f;
                                let pulse_spacing = 40.0f;
                                let pulse_width = 12.0f;

                                // Create repeating pulses moving down (increasing sd)
                                let phase1 = modf(sd - t * pulse_speed, pulse_spacing);
                                let pulse1 = exp(-phase1 * phase1 / (pulse_width * pulse_width));

                                let phase2 = modf(sd - t * pulse_speed * 0.7f + pulse_spacing * 0.5f, pulse_spacing * 1.3f);
                                let pulse2 = exp(-phase2 * phase2 / (pulse_width * 1.5f * pulse_width * 1.5f));

                                let phase3 = modf(sd - t * pulse_speed * 1.2f + pulse_spacing * 0.3f, pulse_spacing * 0.8f);
                                let pulse3 = exp(-phase3 * phase3 / (pulse_width * 0.8f * pulse_width * 0.8f));

                                let pulse = pulse1 * 0.6f + pulse2 * 0.3f + pulse3 * 0.2f;

                                // Light ray color - bright white-blue glow
                                let ray_color = vec3(0.5f, 0.8f, 1.0f);
                                let brightness = pulse * 1.5f;

                                let out_rgb = bc + ray_color * brightness;
                                let out_a = ba;
                                return vec4(out_rgb * out_a, out_a)
                            }

                            return base
                        }
                    }
                }
            }
        }

        StoryHeading{text: "It takes the room you give it"}
        StoryNote{text: "Fit by default, so it takes the drawing's own size. Given a width and a height it scales into them."}
        StoryRow{
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 64. height: 64.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 96. height: 32.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
        }

        StoryHeading{text: "The document's colours, unless you say otherwise"}
        StoryNote{text: "draw_svg.color carries a sentinel meaning leave the drawing alone, so by default you get the colours it was authored with. Give it a colour and that colour replaces them, keeping the per-vertex alpha — which is how the same file serves as a picture in one place and a tinted mark in another."}
        StoryRow{
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                tinted := Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                        color: theme.color_text_meta
                    }
                }
            }
            Label{text: "as authored, then tinted" draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "animating is on by default, and it is not free"}
        StoryNote{text: "An animating Svg asks for the next frame, every frame, for as long as it exists — that is how a drawing with time in it moves. A drawing with no time in it does exactly the same thing and shows exactly the same picture, so the only way to tell is a machine that never idles. The one caller in this repository that thought about it writes animating: false. Both of these look identical; the left one is spinning the frame loop."}
        StoryRow{
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 48. height: 48.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}
                }
            }
            Label{text: "left: animating (default). right: animating: false." draw_text +: {color: theme.color_text_meta}}
        }

        StoryHeading{text: "A document of real size"}
        StoryNote{text: "One icon says little about the parser and the tessellator. This is two hundred and forty stroked and filled paths, each in a group that sets its own paint, all under one scaling transform, fitted into three hundred points. animating is false because nothing in it moves."}
        StoryRow{
            SolidView{
                width: Fit height: Fit
                padding: theme.mspace_2
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                Svg{
                    width: 300. height: 300.
                    animating: false
                    draw_svg +: {svg: crate_resource("self:resources/tiger.svg")}
                }
            }
        }

        StoryHeading{text: "A shader per shape"}
        StoryNote{text: "A shape carrying a data-shader-id attribute arrives in the pixel shader as v_shape_id, so a get_color override can treat each tagged shape differently. It has svg_time, the paint's parameters in v_param0 to v_param4, the position in v_world, the distance along a stroke in v_stroke_dist, and eval_gradient() for the colour the file asked for. The drawing at the top of this page overrides it once, and seven ids become a pulsing halo, an iridescent dome, thin-film bubbles, plankton, caustic rays, the water column, and light running down the tentacles."}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "media/svg/overview",
    category: "Media",
    component: "Svg",
    also: &[],
    name: "Overview",
    dsl: "SvgOverview",
    added: "2026-02-12",
    tags: &["ported", "animation", "shader", "shader id"],
    doc: "# Svg

A vector document drawn as a widget. Seven places in this repository use one.

It is not `Icon`, but the difference is not tinting — both draw through the same `DrawSvg`. An `Icon` wraps that drawing in a background quad and gives it an `icon_walk` and a `size`, so it can sit inside a button and be measured like a glyph. `Svg` is the drawing on its own, taking the widget's own walk.

## Which one to use

| Want | Use |
|---|---|
| artwork that already exists as a file | `Svg` |
| a small drawing composed in the same DSL as the page, or one with a value that comes from somewhere else | `Vector`, on the Vector page |
| a drawing inside a control, measured like a glyph | `Icon` |

## Size and colour

`draw_svg.svg` takes the resource. The widget is `Fit` by default, so it takes the drawing's own size; give it a width and a height and it scales into them, ignoring the drawing's aspect if you ask it to.

`draw_svg.color` holds a sentinel that means *leave the drawing alone*, so the default is the colours the file was authored with. Setting a colour replaces them while keeping the per-vertex alpha, which is how one file serves as a picture in one place and a tinted mark in another.

## Animating

**`animating` is `true` by default and it costs a frame loop.** An animating `Svg` asks for the next frame on every frame, for as long as it exists, which is what makes a drawing with time in it move. A drawing with no time in it does the same thing and shows the same still picture, so nothing on screen tells you it is happening — the only symptom is a process that never goes idle. Of the callers in this repository, one sets `animating: false` deliberately; the rest take the default. Set it false unless the drawing actually moves.

**The document can carry its own time.** `animate` and `animateTransform` elements in the file drive its paths, positions, transforms and opacity, and a `symbol` placed through `use` is instanced as many times as the file asks, each instance taking its own tint through `currentColor`. Nothing in the DSL says any of this; the file does. What the DSL says is `animating: true`, and for the drawing at the top of the page that is the right setting: the widget asks for the next frame on every frame and hands the elapsed time to the document, and that is what moves it. The control switches it off, and the scene freezes where it is — the file's clock only runs while the widget's does.

A document of real size costs nothing more at draw time than an icon: the file is parsed and tessellated once into cached geometry, and every frame after that is one draw of that geometry, however many paths it holds.

## A shader per shape

A shape carrying a `data-shader-id` attribute keeps that number through tessellation and arrives in the pixel shader as `v_shape_id`, with its paint's parameters in `v_param0` to `v_param4` (the bounding box for a solid paint, the end points for a linear gradient, the centre and radii for a radial one), its position in `v_world`, its distance along the stroke in `v_stroke_dist`, the time in `svg_time`, and `eval_gradient()` for the colour the file asked for. A `get_color` override on `draw_svg` reads the id and does something different for each. The drawing at the top of the page overrides it once and handles seven ids: a pulsing halo, a lit and iridescent dome, glass bubbles with a thin-film rainbow, plankton that glow, caustic light rays, the water column, and pulses of light running down the tentacles. Untagged shapes fall through to the file's own colour.

The override is written in the shader language inside the DSL: helper functions (`hash21`, `voronoi`) are declared next to `get_color` and called through `self`. A shader that does not compile is not an error the compiler sees — the draw is skipped and the widget paints nothing — which is why this file's test builds the page and asks the shader compiler what it saw.",
    subject: "ocean",
    feature: None,
    controls: &[Control {
        label: "animating",
        target: "ocean",
        kind: ControlKind::Bool { prop: "animating", default: true },
    }],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is markup the compiler never reads, and it carries a shader
    /// override of two hundred and fifty lines. A draw
    /// shader that fails to compile is not an error anywhere: the draw is
    /// skipped and the widget paints nothing, so the only way to find out
    /// is to build every page and ask the shader compiler what it saw. The
    /// panel then addresses the subject and the control targets by name,
    /// and a name that resolves to nothing is a panel that moves nothing;
    /// and the switch it shows has to open agreeing with the page.
    #[test]
    fn every_page_builds_with_its_shaders_and_its_names_resolve() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        for story in STORIES {
            let page = cx.with_vm(|vm| {
                let stories = vm.module(id!(stories));
                let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
                assert!(value.as_object().is_some(), "no template {}", story.dsl);
                WidgetRef::script_from_value(vm, value)
            });
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
                    "{}: no widget at {target}",
                    story.key
                );
            }
            let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
            let svg = subject.borrow::<Svg>().expect("the subject is an Svg");
            for control in story.controls {
                if let ControlKind::Bool { prop: "animating", default } = control.kind {
                    assert_eq!(
                        svg.animating, default,
                        "{}: the animating switch opens disagreeing with the page",
                        story.key
                    );
                }
            }
        }
    }
}
