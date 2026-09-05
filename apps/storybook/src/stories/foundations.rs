//! The foundations stories: the theme's tokens shown as what they are.
//!
//! The tables read `mod.theme` at draw time through the library's
//! reflection surface, so they list exactly what the running theme defines
//! and cannot drift from the theme files. The visual rows (the radius boxes,
//! the elevation cards, the type presets, the easing buttons) are DSL, since
//! text styles and easings are objects reflection does not reach.
use crate::makepad_widgets::reflect::{theme_values, ThemeVal};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.widgets.TokenTableBase = #(TokenTable::register_widget(vm))
    /** Every theme token whose name starts with one of `prefixes`, in that order: a swatch or a bar, the name, the value and where the theme defines it. */
    mod.widgets.TokenTable = set_type_default() do mod.widgets.TokenTableBase{
        width: Fill
        height: Fit
        draw_swatch +: {
            color: #888
        }
        draw_text +: {
            text_style: theme.font_body_m
            color: theme.color_text
        }
        draw_meta +: {
            text_style: theme.font_body_s
            color: theme.color_text_meta
        }
    }

    mod.stories.FoundationsColorRoles = StoryPage{
        StoryNote{text: "The accent families and the four intents. Each has a base, the colour that reads on it, a container and the colour that reads on the container."}
        mod.widgets.TokenTable{prefixes: [
            "color_primary" "color_on_primary" "color_secondary" "color_on_secondary"
            "color_tertiary" "color_on_tertiary" "color_error" "color_on_error"
            "color_warning" "color_on_warning" "color_success" "color_on_success"
            "color_info" "color_on_info" "color_inverse_primary"
        ]}
    }

    mod.stories.FoundationsColorSurfaces = StoryPage{
        StoryNote{text: "The surface ladder, what reads on it, the outlines, the inverse pair and the scrim."}
        mod.widgets.TokenTable{prefixes: [
            "color_surface" "color_on_surface" "color_outline" "color_inverse_surface"
            "color_inverse_on_surface" "color_scrim" "color_elevation"
        ]}
    }

    mod.stories.FoundationsColorStatus = StoryPage{
        StoryNote{text: "Presence and placeholder colours: a status is never colour alone, but these are the colours it uses."}
        mod.widgets.TokenTable{prefixes: ["color_presence_" "color_placeholder"]}
    }

    mod.stories.FoundationsColorPalette = StoryPage{
        StoryNote{text: "Every colour the theme defines, roles and legacy tokens alike, in the order the theme file declares them."}
        mod.widgets.TokenTable{prefixes: ["color_"]}
    }

    mod.stories.FoundationsShapeRadius = StoryPage{
        StoryNote{text: "The corner ladder, from none to a full pill."}
        StoryRow{
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_none}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_xs}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_s}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_m}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_l}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_xl}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_full}}
        }
        mod.widgets.TokenTable{prefixes: ["radius_"]}
    }

    mod.stories.FoundationsElevationLevels = StoryPage{
        StoryNote{text: "Five levels of lift. Each preset takes its blur, drop and shadow colour from the elevation tokens, so everything built on them agrees on how high it sits."}
        StoryRow{
            spacing: 28.
            padding: theme.mspace_3
            ElevatedView1{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "1"}}
            ElevatedView2{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "2"}}
            ElevatedView3{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "3"}}
            ElevatedView4{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "4"}}
            ElevatedView5{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "5"}}
        }
        mod.widgets.TokenTable{prefixes: ["elevation_" "color_elevation"]}
    }

    mod.stories.FoundationsMotionOverview = StoryPage{
        StoryNote{text: "Durations in seconds, short to extra long, and the seven easings. Hover a button: its fade takes the long duration and follows the easing it is named after."}
        StoryHeading{text: "Easings"}
        StoryRow{
            Button{text: "standard" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            Button{text: "standard decelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard_decelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            Button{text: "standard accelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard_accelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            Button{text: "emphasized decelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_emphasized_decelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
        }
        StoryRow{
            Button{text: "emphasized accelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_emphasized_accelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            Button{text: "linear" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_linear apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            Button{text: "spring" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_spring apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
        }
        StoryHeading{text: "Durations"}
        mod.widgets.TokenTable{prefixes: ["motion_"]}
    }

    mod.stories.FoundationsStateLayers = StoryPage{
        StoryNote{text: "The opacity a state layer adds over a surface: hover, focus, press, drag, the two disabled strengths and the scrim. The chip shows the text colour at that opacity."}
        mod.widgets.TokenTable{prefixes: ["state_"]}
    }

    mod.stories.FoundationsSpacingScale = StoryPage{
        StoryNote{text: "The spacing ladder and the factor it is built from. Every bar is the token's length."}
        mod.widgets.TokenTable{prefixes: ["space_"]}
    }

    mod.stories.FoundationsSizeScale = StoryPage{
        StoryNote{text: "Control heights, icon sizes, the touch target and the hairlines. Every bar is the token's length."}
        mod.widgets.TokenTable{prefixes: ["size_"]}
    }

    mod.stories.FoundationsTypeScale = StoryPage{
        StoryNote{text: "The nine presets built on the font size knob, so the whole scale moves with it."}
        StoryRow{flow: Down spacing: theme.space_1
            Label{text: "Title L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_l}}
            Label{text: "Title M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_m}}
            Label{text: "Title S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_s}}
            Label{text: "Body L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_l}}
            Label{text: "Body M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_m}}
            Label{text: "Body S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_s}}
            Label{text: "Label L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_l}}
            Label{text: "Label M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_m}}
            Label{text: "Label S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_s}}
        }
        mod.widgets.TokenTable{prefixes: ["type_" "font_size_"]}
    }
}

/// One line of a table: what to show at the left, then the three texts.
struct Row {
    name: String,
    value: String,
    at: String,
    demo: Demo,
}

enum Demo {
    /// A filled swatch.
    Color(u32),
    /// A bar this many points long.
    Bar(f64),
    /// The text colour at this opacity.
    Opacity(f64),
}

#[derive(Script, ScriptHook, Widget)]
pub struct TokenTable {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[live]
    draw_swatch: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_meta: DrawText,
    /// Only tokens whose name starts with one of these are listed, grouped in this order.
    #[live]
    prefixes: Vec<String>,
    /// Height of one line.
    #[live(30.0)]
    row_height: f64,
    /// Width of the swatch column.
    #[live(72.0)]
    swatch_width: f64,
    /// Width of the name column.
    #[live(260.0)]
    name_width: f64,
    /// Width of the value column.
    #[live(110.0)]
    value_width: f64,
    #[rust]
    rows: Vec<Row>,
}

fn read_rows(cx: &mut Cx, prefixes: &[String]) -> Vec<Row> {
    let mut rows: Vec<(usize, Row)> = Vec::new();
    for (name, _key, val, at) in theme_values(cx) {
        let Some(group) = prefixes.iter().position(|p| name.starts_with(p.as_str())) else {
            continue;
        };
        let (value, demo) = match val {
            ThemeVal::Color(c) => (format!("#{c:08x}"), Demo::Color(c)),
            ThemeVal::Num(v) => {
                let demo = if name.starts_with("state_") {
                    Demo::Opacity(v)
                } else if name.starts_with("motion_") {
                    Demo::Bar(v * 60.0)
                } else {
                    Demo::Bar(v)
                };
                (format!("{v}"), demo)
            }
        };
        rows.push((group, Row { name, value, at, demo }));
    }
    // Grouped by the prefix list, then by name, so a family reads base,
    // container, and the ladders count up.
    rows.sort_by(|(ga, a), (gb, b)| ga.cmp(gb).then_with(|| a.name.cmp(&b.name)));
    rows.into_iter().map(|(_, row)| row).collect()
}

fn color_vec(c: u32) -> Vec4f {
    Vec4f {
        x: ((c >> 24) & 0xFF) as f32 / 255.0,
        y: ((c >> 16) & 0xFF) as f32 / 255.0,
        z: ((c >> 8) & 0xFF) as f32 / 255.0,
        w: (c & 0xFF) as f32 / 255.0,
    }
}

impl Widget for TokenTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.rows.is_empty() {
            self.rows = read_rows(cx, &self.prefixes);
        }
        cx.begin_turtle(walk, Layout::flow_down());
        let h = self.row_height;
        let rows = std::mem::take(&mut self.rows);
        for row in &rows {
            let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(h)));
            let demo_top = rect.pos.y + 5.0;
            let demo_height = h - 10.0;
            match row.demo {
                Demo::Color(c) => {
                    self.draw_swatch.color = color_vec(c);
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top), size: dvec2(self.swatch_width, demo_height) },
                    );
                }
                Demo::Bar(len) => {
                    let mut c = self.draw_text.color;
                    c.w = 0.8;
                    self.draw_swatch.color = c;
                    let len = len.clamp(2.0, self.swatch_width);
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top + 4.0), size: dvec2(len, demo_height - 8.0) },
                    );
                }
                Demo::Opacity(a) => {
                    let mut c = self.draw_text.color;
                    c.w = a as f32;
                    self.draw_swatch.color = c;
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top), size: dvec2(self.swatch_width, demo_height) },
                    );
                }
            }
            let x = rect.pos.x + self.swatch_width + 12.0;
            let y = rect.pos.y + 6.0;
            self.draw_text.draw_abs(cx, dvec2(x, y), &row.name);
            self.draw_text.draw_abs(cx, dvec2(x + self.name_width, y), &row.value);
            self.draw_meta.draw_abs(cx, dvec2(x + self.name_width + self.value_width, y + 1.0), &row.at);
        }
        self.rows = rows;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // A theme switch arrives as a reload: read the tokens again.
        if let Event::LiveEdit = event {
            self.rows.clear();
        }
    }
}

const fn story(key: &'static str, component: &'static str, name: &'static str, dsl: &'static str, doc: &'static str) -> Story {
    Story {
        key,
        category: "Foundations",
        component,
        name,
        dsl,
        added: "2026-09-05",
        tags: &["tokens", "new"],
        doc,
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    }
}

pub const STORIES: &[Story] = &[
    story(
        "foundations/colour/roles",
        "Colour",
        "Roles",
        "FoundationsColorRoles",
        "# Colour roles\n\nSeven accent families: primary, secondary, tertiary and the four intents. Each family is four tokens: the base, `color_on_<family>` for what reads on it, a container, and what reads on the container. The values are generated from the house seed by the rule in the token registry; a unit test regenerates them and diffs against the theme files.",
    ),
    story(
        "foundations/colour/surfaces",
        "Colour",
        "Surfaces",
        "FoundationsColorSurfaces",
        "# Surfaces\n\nThe surface ladder sits on the opaque ladder the legacy tokens already define: `color_surface` is the app background, the containers step up or down from it. Outlines are translucent tints, surfaces never are. The scrim and the inverse pair are here too.",
    ),
    story(
        "foundations/colour/status",
        "Colour",
        "Status",
        "FoundationsColorStatus",
        "# Status colours\n\nPresence dots and the placeholder shimmer. A status is shown with a shape as well as a colour, so it is never colour alone.",
    ),
    story(
        "foundations/colour/palette",
        "Colour",
        "Palette",
        "FoundationsColorPalette",
        "# Palette\n\nEvery colour token in the running theme, roles and legacy alike, in the order the theme file declares them. This is what the design overlay's palette strip reads.",
    ),
    story(
        "foundations/shape/radius",
        "Shape",
        "Radius",
        "FoundationsShapeRadius",
        "# Radius\n\nSeven corner sizes from none to a full pill. Widgets take one of these rather than a number of their own.",
    ),
    story(
        "foundations/elevation/levels",
        "Elevation",
        "Levels",
        "FoundationsElevationLevels",
        "# Elevation\n\nFive levels, each a blur radius, a vertical drop and a shadow colour. `ElevatedView1` to `ElevatedView5` apply them to a rounded shadow view.",
    ),
    story(
        "foundations/motion/overview",
        "Motion",
        "Overview",
        "FoundationsMotionOverview",
        "# Motion\n\nSixteen durations in four bands and seven easings. An easing token is an `Ease` object, so an animator state says `ease: theme.motion_ease_standard`; the buttons above use exactly that.",
    ),
    story(
        "foundations/state/layers",
        "State",
        "Layers",
        "FoundationsStateLayers",
        "# State layers\n\nA state is shown by laying the content colour over the surface at a fixed opacity. These are the opacities.",
    ),
    story(
        "foundations/spacing/scale",
        "Spacing",
        "Scale",
        "FoundationsSpacingScale",
        "# Spacing\n\nSix steps built on `space_factor`. The insets (`mspace_*`) are objects and are not listed here.",
    ),
    story(
        "foundations/size/scale",
        "Size",
        "Scale",
        "FoundationsSizeScale",
        "# Sizes\n\nThree control heights, three icon sizes, the touch target and the hairline widths.",
    ),
    story(
        "foundations/type/scale",
        "Type",
        "Scale",
        "FoundationsTypeScale",
        "# Type scale\n\nTitle, body and label in three sizes each. The sizes are expressed on `font_size_base` and `font_size_contrast`, so the scale follows the theme's knob; `font_size_5` and `font_size_6` fill the gap between the headings and the paragraph size.",
    ),
];
