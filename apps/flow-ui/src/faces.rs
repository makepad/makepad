//! Faces (DESIGN.md §3): one splash isolate per open instance. The flow file
//! is evaluated in it with the REAL face prelude (`faces.splash`) in scope,
//! each node's `ui` object is mounted with `WidgetRef::script_from_value`
//! inside that isolate — so every inline handler routes back to it — and the
//! canvas draws the mounted roots inside its node frames.
//!
//! The `flow` bridge the handlers see never re-enters the canvas: every call
//! is posted as a [`FaceBridgeCall`] action and the app acts on it on the
//! next event dispatch.

use crate::graph_view::{declared_output_type, PortIcon};
use crate::values::ValueCache;
use makepad_code_editor::code_view::CodeView;
use makepad_flow::{
    Graph, InstanceRow, Literal, ModelsResponse, Node, NodeTypeCatalog, PortType, ValueBytes,
    ValueRef, PRELUDE,
};
use makepad_widgets::fab_controls::*;
use makepad_widgets::makepad_micro_serde::SerJson;
use makepad_widgets::makepad_platform::event::TweakRayEvent;
use makepad_widgets::makepad_script::*;
use makepad_widgets::widget_async::{enter_isolate, leave_isolate, CxSplashVmExt, SplashVmId};
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use makepad_flowgraph::{Camera, NodeFaces};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

const FACES: &str = include_str!("faces.splash");
const RECIPE_PRELUDE: &str = include_str!("../../../libs/flow/recipes/prelude_recipes.splash");
const PRELUDE_FILE: &str = "<makepad-flow-prelude>";
const FACES_FILE: &str = "<flow-ui-faces>";
const RECIPE_FILE: &str = "<makepad-flow-recipe-prelude>";
const FLOW_INSTRUCTION_LIMIT: usize = 5_000_000;
const HANDLER_INSTRUCTION_LIMIT: usize = 200_000;
/// The model picker's first entry: the hub elects the box and the model.
pub const HUB_PICKS: &str = "hub picks";
/// The first entry in every size preset picker.
pub const CUSTOM_FORMAT: &str = "Custom";
/// The caret shown at the end of streaming text.
const STREAM_CARET: &str = " ▌";

/// One width × height choice shown by the face and inspector pickers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatPreset {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl FormatPreset {
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            name: name.into(),
            width,
            height,
        }
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

const IMAGE_FORMAT_PRESETS: &[(&str, u32, u32)] = &[
    ("512×512", 512, 512),
    ("768×768", 768, 768),
    ("1024×1024", 1024, 1024),
    ("1536×1536", 1536, 1536),
    ("2048×2048", 2048, 2048),
    ("1024×768 (4:3)", 1024, 768),
    ("768×1024 (3:4)", 768, 1024),
    ("1280×720 (16:9)", 1280, 720),
    ("720×1280 (9:16)", 720, 1280),
    ("1920×1080 (16:9)", 1920, 1080),
    ("1080×1920 (9:16)", 1080, 1920),
    ("1024×576 (16:9)", 1024, 576),
    ("576×1024 (9:16)", 576, 1024),
    ("1344×768 (7:4)", 1344, 768),
    ("768×1344 (4:7)", 768, 1344),
];

#[derive(Clone, Debug, PartialEq)]
pub struct FormatOptions {
    pub presets: Vec<FormatPreset>,
    pub width_range: (f64, f64, f64),
    pub height_range: (f64, f64, f64),
}

/// The matching preset label, with `Custom` for a hand-entered size.
pub fn format_preset_name(presets: &[FormatPreset], width: u32, height: u32) -> &str {
    presets
        .iter()
        .find(|preset| preset.dimensions() == (width, height))
        .map(|preset| preset.name.as_str())
        .unwrap_or(CUSTOM_FORMAT)
}

fn doc_format_presets(entry: &NodeTypeCatalog) -> Vec<FormatPreset> {
    let mut dimensions = Vec::new();
    for param in &entry.params {
        if !matches!(param.name.as_str(), "width" | "height") {
            continue;
        }
        for word in param.doc.split(|c: char| c.is_whitespace() || c == ',') {
            let word = word.trim_matches(|c: char| {
                !(c.is_ascii_digit() || matches!(c, 'x' | '×'))
            });
            let pair = word.split_once('x').or_else(|| word.split_once('×'));
            let Some((width, height)) = pair else {
                continue;
            };
            let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) else {
                continue;
            };
            if !dimensions.contains(&(width, height)) {
                dimensions.push((width, height));
            }
        }
    }
    dimensions
        .into_iter()
        .map(|(width, height)| {
            FormatPreset::new(format!("{width}×{height}"), width, height)
        })
        .collect()
}

fn catalog_range(entry: Option<&NodeTypeCatalog>, name: &str) -> Option<(f64, f64, f64)> {
    entry?
        .params
        .iter()
        .find(|param| param.name == name)?
        .range
        .as_ref()
        .map(|range| (range.min, range.max, range.step.unwrap_or(1.0)))
}

fn node_param_range(
    node: &Node,
    catalog: &[NodeTypeCatalog],
    name: &str,
) -> Option<(f64, f64, f64)> {
    catalog_range(catalog_entry_for_node(node, catalog), name)
}

fn in_range(value: u32, range: Option<(f64, f64, f64)>) -> bool {
    range.is_none_or(|(min, max, _)| (value as f64) >= min && (value as f64) <= max)
}

fn on_step(value: u32, range: (f64, f64, f64)) -> bool {
    let step = range.2;
    step <= 0.0 || ((value as f64 / step).round() * step - value as f64).abs() < 1e-9
}

pub(crate) fn snap_stepped_value(value: f64, range: (f64, f64, f64)) -> f64 {
    let (min, max, step) = range;
    let snapped = if step.is_finite() && step > 0.0 {
        (value / step).round() * step
    } else {
        value
    };
    snapped.clamp(min, max)
}

fn catalog_entry_for_node<'a>(
    node: &Node,
    catalog: &'a [NodeTypeCatalog],
) -> Option<&'a NodeTypeCatalog> {
    node.domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
        .and_then(|domain| {
            catalog
                .iter()
                .find(|entry| entry.domain.as_deref() == Some(domain))
        })
        .or_else(|| {
            catalog
                .iter()
                .find(|entry| entry.type_name == node.type_name)
        })
}

/// Size choices and number-field bounds for a node that owns both params.
/// Pair lists in catalog docs (notably `Video`) win; otherwise the image
/// presets are clipped to the documented numeric width and height ranges.
pub fn format_options_for_node(
    node: &Node,
    catalog: &[NodeTypeCatalog],
) -> Option<FormatOptions> {
    node_dimensions(node)?;
    // Recipe-derived generators currently retain `Gen` in evaluated nodes,
    // while their catalog row carries the specialised type name. The domain
    // is the stable join for those rows (and maps `video` to `Video`).
    let entry = catalog_entry_for_node(node, catalog);
    let documented = entry.map(doc_format_presets).unwrap_or_default();
    if !documented.is_empty() {
        let width_min = documented.iter().map(|preset| preset.width).min()? as f64;
        let width_max = documented.iter().map(|preset| preset.width).max()? as f64;
        let height_min = documented.iter().map(|preset| preset.height).min()? as f64;
        let height_max = documented.iter().map(|preset| preset.height).max()? as f64;
        let width_range = catalog_range(entry, "width").unwrap_or((width_min, width_max, 1.0));
        let height_range =
            catalog_range(entry, "height").unwrap_or((height_min, height_max, 1.0));
        return Some(FormatOptions {
            presets: documented
                .into_iter()
                .filter(|preset| {
                    on_step(preset.width, width_range) && on_step(preset.height, height_range)
                })
                .collect(),
            width_range,
            height_range,
        });
    }
    let width_range = catalog_range(entry, "width").unwrap_or((256.0, 2048.0, 1.0));
    let height_range = catalog_range(entry, "height").unwrap_or((256.0, 2048.0, 1.0));
    Some(FormatOptions {
        presets: IMAGE_FORMAT_PRESETS
            .iter()
            .filter(|(_, width, height)| {
                in_range(*width, Some(width_range)) && in_range(*height, Some(height_range))
                    && on_step(*width, width_range)
                    && on_step(*height, height_range)
            })
            .map(|(name, width, height)| FormatPreset::new(*name, *width, *height))
            .collect(),
        width_range,
        height_range,
    })
}

pub fn node_dimensions(node: &Node) -> Option<(u32, u32)> {
    let dimension = |name| match node_param(node, name) {
        Some(Literal::Num(value)) if value.is_finite() && *value >= 0.0 => Some(*value as u32),
        _ => None,
    };
    Some((dimension("width")?, dimension("height")?))
}

/// One model id after collapsing the per-fleet-node rows returned by the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub dimmed: bool,
    pub note: String,
}

/// Collapse repeated model ids, count distinct advertising nodes, and put
/// ready/available choices before models that still need acquiring.
pub fn model_choices(response: &ModelsResponse) -> Vec<ModelChoice> {
    #[derive(Default)]
    struct Acc {
        nodes: BTreeSet<String>,
        ready: BTreeSet<String>,
        absent: BTreeSet<String>,
        too_small: BTreeSet<String>,
        admissible: BTreeSet<String>,
        reasons: BTreeSet<String>,
    }

    let mut by_id: BTreeMap<String, Acc> = BTreeMap::new();
    for model in &response.models {
        let entry = by_id.entry(model.id.clone()).or_default();
        entry.nodes.insert(model.node.clone());
        match model.state.as_str() {
            "ready" | "loaded" => {
                entry.ready.insert(model.node.clone());
            }
            "absent" => {
                entry.absent.insert(model.node.clone());
            }
            "too_small" => {
                entry.too_small.insert(model.node.clone());
            }
            _ => {}
        }
        if model.available && model.state != "too_small" {
            entry.admissible.insert(model.node.clone());
        }
        if let Some(note) = model.note.as_ref().filter(|note| !note.is_empty()) {
            entry.reasons.insert(note.clone());
        }
    }
    let mut choices: Vec<_> = by_id
        .into_iter()
        .map(|(id, acc)| {
            let mut label = id.clone();
            if !acc.ready.is_empty() {
                label.push_str(&format!(" · {} ready", acc.ready.len()));
            }
            if !acc.absent.is_empty() {
                label.push_str(&format!(" · {} absent", acc.absent.len()));
            }
            if !acc.too_small.is_empty() {
                label.push_str(&format!(" · {} too small", acc.too_small.len()));
            }
            let accounted = acc
                .ready
                .union(&acc.absent)
                .cloned()
                .collect::<BTreeSet<_>>()
                .union(&acc.too_small)
                .count();
            if accounted < acc.nodes.len() {
                label.push_str(&format!(" · {} other", acc.nodes.len() - accounted));
            }
            let ready_nodes: Vec<_> = acc
                .ready
                .iter()
                .map(|url| {
                    let gpu = response
                        .nodes
                        .iter()
                        .find(|node| node.base_url == *url)
                        .and_then(|node| node.gpu.as_deref());
                    match gpu {
                        Some(gpu) => format!("{} {gpu}", display_node(url)),
                        None => display_node(url).to_string(),
                    }
                })
                .collect();
            let note = if !ready_nodes.is_empty() {
                ready_nodes.join(" · ")
            } else if acc.admissible.is_empty() {
                acc.reasons.into_iter().collect::<Vec<_>>().join(" · ")
            } else {
                "downloads on first use".to_string()
            };
            let dimmed = acc.admissible.is_empty();
            (
                if !acc.ready.is_empty() {
                    0
                } else if dimmed {
                    2
                } else {
                    1
                },
                ModelChoice {
                    id,
                    label,
                    dimmed,
                    note,
                },
            )
        })
        .collect();
    choices.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| left.id.cmp(&right.id))
    });
    choices.into_iter().map(|(_, choice)| choice).collect()
}

fn display_node(base_url: &str) -> &str {
    base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))
        .unwrap_or(base_url)
        .split(':')
        .next()
        .unwrap_or(base_url)
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A picture that fills its card. The transparent two-pixel rim exposes
    // the card border, which was drawn first, while the SDF keeps every
    // resized/zoomed image inside the same rounded body.
    let RoundedPicture = Image{
        width: Fill
        height: Fit
        fit: ImageFit.Horizontal
        draw_bg +: {
            radius: uniform(16.0)
            content_inset: uniform(2.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.content_inset
                    self.content_inset
                    self.rect_size.x - self.content_inset * 2.0
                    self.rect_size.y - self.content_inset * 2.0
                    self.radius
                )
                let c = self.get_color()
                sdf.fill(vec4(c.rgb, c.a * self.opacity))
                return sdf.result
            }
        }
    }

    let EmptyIcon = Svg{
        width: 26
        height: Fit
        animating: false
        draw_svg +: {
            color: theme.flow_surface_input
        }
    }

    let EmptyWell = RoundedView{
        width: Fill
        height: 150
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        spacing: theme.space_2
        draw_bg +: {
            color: theme.flow_surface_deep
            border_radius: 16.0
            content_inset: uniform(2.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.content_inset
                    self.content_inset
                    self.rect_size.x - self.content_inset * 2.0
                    self.rect_size.y - self.content_inset * 2.0
                    self.border_radius
                )
                sdf.fill(self.color)
                return sdf.result
            }
        }
        icon := EmptyIcon{
            draw_svg +: {svg: crate_resource("self:resources/icons/image.svg")}
        }
        note := Label{
            width: Fit
            height: Fit
            text: "no picture yet"
            draw_text +: {
                color: theme.flow_text_empty
                text_style: theme.font_regular{font_size: 9}
            }
        }
    }

    mod.flow.ui.ValueImageBase = #(ValueImage::register_widget(vm))
    mod.flow.ui.ValueImage = set_type_default() do mod.flow.ui.ValueImageBase{
        width: Fill
        height: Fit
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        empty := EmptyWell{}
        image := RoundedPicture{
            visible: false
        }
    }

    // Text in a card gets one quiet, bounded viewport. At canvas zoom this
    // scales with the rest of the card; a user-sized card switches it to Fill.
    mod.flow.ui.TextScroll = ScrollYView{
        width: Fill
        height: Fit{max: FitBound.Abs(160)}
        scroll_bars +: {
            scroll_bar_y +: {
                bar_size: 8
                bar_side_margin: 1
                draw_bg +: {
                    color: #xffffff18
                    color_hover: #xffffff30
                    color_drag: #xffffff48
                }
            }
        }
    }

    mod.flow.ui.ValueTextBase = #(ValueText::register_widget(vm))
    mod.flow.ui.ValueText = set_type_default() do mod.flow.ui.ValueTextBase{
        width: Fill
        height: Fit
        flow: Down
        text_scroll := mod.flow.ui.TextScroll{
            text := Label{
                width: Fill
                height: Fit
                text: ""
                draw_text +: {
                    text_style: theme.font_code{font_size: 9}
                    color: theme.flow_text_code
                }
            }
        }
    }

    mod.flow.ui.ValueViewBase = #(ValueView::register_widget(vm))
    mod.flow.ui.ValueView = set_type_default() do mod.flow.ui.ValueViewBase{
        width: Fill
        height: Fit
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        empty := EmptyWell{
            note +: {text: "no value yet"}
        }
        image := RoundedPicture{
            visible: false
        }
        text_scroll := mod.flow.ui.TextScroll{
            visible: false
            text := Label{
                width: Fill
                height: Fit
                margin: Inset{left: 14 right: 14 top: 12 bottom: 12}
                text: ""
                draw_text +: {
                    color: theme.flow_text_body
                    text_style: theme.font_regular{font_size: 9.5}
                }
            }
        }
    }

    // Stable typed names for custom faces. Media types share ValueView's
    // type-aware empty state and byte-count presentation until a decoder is
    // available; JSON deliberately uses the code-font text renderer.
    mod.flow.ui.ValueJson = mod.flow.ui.ValueText{}
    mod.flow.ui.ValueAudio = mod.flow.ui.ValueView{}
    mod.flow.ui.ValueVideo = mod.flow.ui.ValueView{}
    mod.flow.ui.ValueMesh = mod.flow.ui.ValueView{}

    mod.flow.ui.ModelPickerBase = #(ModelPicker::register_widget(vm))
    mod.flow.ui.ModelPicker = set_type_default() do mod.flow.ui.ModelPickerBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
        select := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_2
            align: Align{y: 0.5}
            Label{
                width: 44
                text: "model"
                draw_text +: {
                    color: theme.flow_text_muted
                    text_style: theme.font_regular{font_size: 9}
                }
            }
            picker := ComboBox{
                width: Fill
                height: 26
                labels: ["hub picks"]
            }
        }
        note := Label{
            width: Fill
            height: Fit
            visible: false
            text: ""
            draw_text +: {
                color: theme.flow_text_hint
                text_style: theme.font_regular{font_size: 8}
            }
        }
    }

    mod.flow.ui.FormatPickerBase = #(FormatPicker::register_widget(vm))
    mod.flow.ui.FormatPicker = set_type_default() do mod.flow.ui.FormatPickerBase{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_1
        align: Align{y: 0.5}
        w_field := mod.widgets.FabValueInput{
            width: 54
            height: 24
            label: "w"
            min: 256
            max: 2048
            step: 8
            snap: 64
            precision: 0
            quantize: true
            param_bind := @width
        }
        h_field := mod.widgets.FabValueInput{
            width: 54
            height: 24
            label: "h"
            min: 256
            max: 2048
            step: 8
            snap: 64
            precision: 0
            quantize: true
            param_bind := @height
        }
        picker := ComboBox{
            width: Fill
            height: 26
            labels: ["Custom"]
        }
        swap := ButtonFlatter{
            width: 26
            height: 26
            text: "⇄"
        }
    }

    // The host wraps direct, named face controls that carry a bind in this
    // row. It keeps user-declared controls aligned with the built-in strip.
    mod.flow.ui.DeclaredInputRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{y: 0.5}
        name := Label{
            width: 44
            height: Fit
            text: ""
            draw_text +: {
                color: theme.flow_text_muted
                text_style: theme.font_regular{font_size: 9}
            }
        }
        value := View{
            width: Fill
            height: Fit
        }
    }
}

/// An image value: the PNG/JPEG bytes become the texture; the picture fills
/// the widget's width and a click asks the host to open it.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueImage {
    #[deref]
    view: View,
    #[rust]
    loaded: bool,
    #[rust]
    card_sized: bool,
}

const TEXT_SCROLL_MAX_HEIGHT: f64 = 160.0;

fn card_height(sized: bool) -> Size {
    if sized {
        Size::fill()
    } else {
        Size::fit()
    }
}

fn text_scroll_height(sized: bool) -> Size {
    if sized {
        Size::fill()
    } else {
        Size::Fit {
            min: None,
            max: Some(FitBound::Abs(TEXT_SCROLL_MAX_HEIGHT)),
        }
    }
}

fn set_view_ref_height(view: &ViewRef, cx: &mut Cx, height: Size) {
    let mut walk = view.walk(cx);
    walk.height = height;
    view.set_walk(cx, walk);
}

fn set_image_card_layout(image: &ImageRef, cx: &mut Cx, sized: bool) {
    let mut walk = image.walk(cx);
    walk.width = Size::fill();
    walk.height = card_height(sized);
    image.set_walk_and_fit(
        cx,
        walk,
        if sized {
            ImageFit::Smallest
        } else {
            ImageFit::Horizontal
        },
    );
}

fn empty_note(ty: PortType) -> &'static str {
    match ty {
        PortType::Image => "no picture yet",
        PortType::Video => "no clip yet",
        PortType::Audio => "no audio yet",
        PortType::Mesh => "no mesh yet",
        PortType::Text | PortType::Json | PortType::List | PortType::Bytes => "no value yet",
    }
}

fn empty_icon_svg(ty: PortType) -> &'static str {
    match PortIcon::for_type(ty) {
        PortIcon::Text => include_str!("../resources/icons/text.svg"),
        PortIcon::Image => include_str!("../resources/icons/image.svg"),
        PortIcon::Audio => include_str!("../resources/icons/audio.svg"),
        PortIcon::Video => include_str!("../resources/icons/video.svg"),
        PortIcon::Mesh => include_str!("../resources/icons/mesh.svg"),
        PortIcon::Json => include_str!("../resources/icons/json.svg"),
        PortIcon::Bytes => include_str!("../resources/icons/bytes.svg"),
    }
}

fn set_empty_type(view: &mut View, cx: &mut Cx, ty: PortType) {
    let icon = view.widget(cx, ids!(empty.icon));
    if let Some(mut icon) = icon.borrow_mut::<Svg>() {
        icon.draw_svg.svg = None;
        icon.draw_svg.load_from_str(empty_icon_svg(ty));
        icon.redraw(cx);
    }
    view.label(cx, ids!(empty.note))
        .set_text(cx, empty_note(ty));
    view.redraw(cx);
}

impl Widget for ValueImage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl ValueImage {
    fn set_card_sized(&mut self, cx: &mut Cx, sized: bool) {
        if self.card_sized == sized {
            return;
        }
        self.card_sized = sized;
        self.view.walk.height = card_height(sized);
        set_image_card_layout(&self.view.image(cx, ids!(image)), cx, sized);
        set_view_ref_height(
            &self.view.view(cx, ids!(empty)),
            cx,
            if sized {
                Size::fill()
            } else {
                Size::Fixed(150.0)
            },
        );
        self.view.redraw(cx);
    }

    fn set_empty_type(&mut self, cx: &mut Cx, ty: PortType) {
        set_empty_type(&mut self.view, cx, ty);
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: &ValueBytes) {
        let image = self.view.image(cx, ids!(image));
        let loaded = if value.content_type.contains("jpeg") || value.content_type.contains("jpg") {
            image.load_jpg_from_data(cx, &value.bytes)
        } else {
            image.load_png_from_data(cx, &value.bytes)
        };
        match loaded {
            Ok(()) => {
                self.loaded = true;
                image.set_visible(cx, true);
                self.view.view(cx, ids!(empty)).set_visible(cx, false);
            }
            Err(error) => {
                self.set_note(cx, &format!("{} · {:?}", value.content_type, error));
            }
        }
        self.view.redraw(cx);
    }

    pub fn set_note(&mut self, cx: &mut Cx, text: &str) {
        if !self.loaded {
            self.view.label(cx, ids!(empty.note)).set_text(cx, text);
        }
        self.view.redraw(cx);
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
}

/// A text / json value in the code font.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueText {
    #[deref]
    view: View,
    #[rust]
    value: String,
    #[rust]
    card_sized: bool,
}

impl Widget for ValueText {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.view.label(cx, ids!(text)).set_text(cx, v);
        self.view.redraw(cx);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

impl ValueText {
    fn set_card_sized(&mut self, cx: &mut Cx, sized: bool) {
        if self.card_sized == sized {
            return;
        }
        self.card_sized = sized;
        self.view.walk.height = card_height(sized);
        set_view_ref_height(
            &self.view.view(cx, ids!(text_scroll)),
            cx,
            text_scroll_height(sized),
        );
        self.view.redraw(cx);
    }
}

/// Shows whatever arrives: an image as a picture, text and json inline,
/// anything else as its content type and byte count.
#[derive(Script, ScriptHook, Widget)]
pub struct ValueView {
    #[deref]
    view: View,
    #[rust]
    value: String,
    #[rust]
    loaded: bool,
    #[rust]
    card_sized: bool,
}

impl Widget for ValueView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.loaded = false;
        self.view.image(cx, ids!(image)).set_visible(cx, false);
        self.view
            .view(cx, ids!(text_scroll))
            .set_visible(cx, !v.is_empty());
        let text = self.view.label(cx, ids!(text));
        text.set_text(cx, v);
        text.set_visible(cx, !v.is_empty());
        self.view.view(cx, ids!(empty)).set_visible(cx, v.is_empty());
        self.view.redraw(cx);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

impl ValueView {
    pub(crate) fn set_card_sized(&mut self, cx: &mut Cx, sized: bool) {
        if self.card_sized == sized {
            return;
        }
        self.card_sized = sized;
        self.view.walk.height = card_height(sized);
        set_image_card_layout(&self.view.image(cx, ids!(image)), cx, sized);
        set_view_ref_height(
            &self.view.view(cx, ids!(empty)),
            cx,
            if sized {
                Size::fill()
            } else {
                Size::Fixed(150.0)
            },
        );
        set_view_ref_height(
            &self.view.view(cx, ids!(text_scroll)),
            cx,
            text_scroll_height(sized),
        );
        self.view.redraw(cx);
    }

    fn set_empty_type(&mut self, cx: &mut Cx, ty: PortType) {
        set_empty_type(&mut self.view, cx, ty);
    }

    pub fn set_image(&mut self, cx: &mut Cx, value: &ValueBytes) {
        let image = self.view.image(cx, ids!(image));
        let loaded = if value.content_type.contains("jpeg") || value.content_type.contains("jpg") {
            image.load_jpg_from_data(cx, &value.bytes)
        } else {
            image.load_png_from_data(cx, &value.bytes)
        };
        match loaded {
            Ok(()) => {
                self.loaded = true;
                image.set_visible(cx, true);
                self.view
                    .view(cx, ids!(text_scroll))
                    .set_visible(cx, false);
                self.view.label(cx, ids!(text)).set_visible(cx, false);
                self.view.view(cx, ids!(empty)).set_visible(cx, false);
            }
            Err(error) => {
                image.set_visible(cx, false);
                self.set_text(cx, &format!("{} · {:?}", value.content_type, error));
            }
        }
        self.view.redraw(cx);
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
}

/// A compact width, height, preset, and orientation control used by the
/// built-in generation faces. Its number fields remain ordinary
/// `param_bind` widgets; preset and swap changes return both values together.
#[derive(Script, ScriptHook, Widget)]
pub struct FormatPicker {
    #[deref]
    view: View,
    #[rust]
    presets: Vec<FormatPreset>,
    #[rust]
    width: u32,
    #[rust]
    height: u32,
    #[rust]
    width_range: (f64, f64, f64),
    #[rust]
    height_range: (f64, f64, f64),
}

impl Widget for FormatPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl FormatPicker {
    fn sync_selected(&self, cx: &mut Cx) {
        let selected = self
            .presets
            .iter()
            .position(|preset| preset.dimensions() == (self.width, self.height))
            .map(|index| index + 1)
            .unwrap_or(0);
        self.view
            .combo_box(cx, ids!(picker))
            .set_selected_item(cx, selected);
    }

    fn set_dimensions(&mut self, cx: &mut Cx, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.view
            .fab_value_input(cx, ids!(w_field))
            .set_value(cx, width as f64);
        self.view
            .fab_value_input(cx, ids!(h_field))
            .set_value(cx, height as f64);
        self.sync_selected(cx);
    }

    fn set_config(
        &mut self,
        cx: &mut Cx,
        options: FormatOptions,
        dimensions: Option<(u32, u32)>,
    ) {
        self.presets = options.presets;
        let mut labels = vec![CUSTOM_FORMAT.to_string()];
        labels.extend(self.presets.iter().map(|preset| preset.name.clone()));
        self.view.combo_box(cx, ids!(picker)).set_labels(cx, labels);
        let (width_min, width_max, width_step) = options.width_range;
        let (height_min, height_max, height_step) = options.height_range;
        self.width_range = options.width_range;
        self.height_range = options.height_range;
        if let Some(mut width) = self
            .view
            .fab_value_input(cx, ids!(w_field))
            .borrow_mut()
        {
            width.set_hint(
                Some(width_min),
                Some(width_max),
                Some((width_step * 0.125).max(1.0)),
            );
        }
        if let Some(mut height) = self
            .view
            .fab_value_input(cx, ids!(h_field))
            .borrow_mut()
        {
            height.set_hint(
                Some(height_min),
                Some(height_max),
                Some((height_step * 0.125).max(1.0)),
            );
        }
        self.view.set_visible(cx, dimensions.is_some());
        if let Some((width, height)) = dimensions {
            self.set_dimensions(cx, width, height);
        }
    }

    /// A preset or swap click. Manual number edits only resynchronise the
    /// label; the ordinary param bindings carry that one changed dimension.
    fn changed(&mut self, cx: &mut Cx, actions: &Actions) -> Option<(u32, u32)> {
        let width_ended = self
            .view
            .fab_value_input(cx, ids!(w_field))
            .ended(actions)
            .is_some();
        let height_ended = self
            .view
            .fab_value_input(cx, ids!(h_field))
            .ended(actions)
            .is_some();
        if width_ended || height_ended {
            let width = snap_stepped_value(
                self.view.fab_value_input(cx, ids!(w_field)).value(),
                self.width_range,
            )
            .round()
            .max(0.0) as u32;
            let height = snap_stepped_value(
                self.view.fab_value_input(cx, ids!(h_field)).value(),
                self.height_range,
            )
            .round()
            .max(0.0) as u32;
            self.set_dimensions(cx, width, height);
            return Some((width, height));
        }
        if let Some(index) = self.view.combo_box(cx, ids!(picker)).changed(actions) {
            let preset = index
                .checked_sub(1)
                .and_then(|index| self.presets.get(index))?
                .clone();
            self.set_dimensions(cx, preset.width, preset.height);
            return Some(preset.dimensions());
        }
        if self.view.button(cx, ids!(swap)).clicked(actions) {
            let dimensions = (self.height, self.width);
            self.set_dimensions(cx, dimensions.0, dimensions.1);
            return Some(dimensions);
        }
        None
    }
}

/// The model name as a dropdown over the hub's live list; the first entry
/// is always "hub picks" (an empty `model` param).
#[derive(Script, ScriptHook, Widget)]
pub struct ModelPicker {
    #[deref]
    view: View,
    #[rust]
    value: String,
    #[rust]
    models: Vec<ModelChoice>,
}

impl Widget for ModelPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.value = v.to_string();
        self.sync_labels(cx);
    }
    fn text(&self) -> String {
        self.value.clone()
    }
}

impl ModelPicker {
    /// The hub's models for this node's domain; the current value stays
    /// selectable even when the list does not carry it.
    pub fn set_models(&mut self, cx: &mut Cx, models: Vec<ModelChoice>) {
        if self.models != models {
            self.models = models;
            self.sync_labels(cx);
        }
    }

    fn sync_labels(&mut self, cx: &mut Cx) {
        let mut labels = vec![HUB_PICKS.to_string()];
        labels.extend(self.models.iter().map(|model| model.label.clone()));
        let selected = self
            .models
            .iter()
            .find(|model| model.id == self.value)
            .map(|model| model.label.clone())
            .unwrap_or_else(|| self.value.clone());
        if !selected.is_empty() && !labels.iter().any(|label| *label == selected) {
            labels.push(self.value.clone());
        }
        let picker = self.view.combo_box(cx, ids!(select.picker));
        picker.set_labels(cx, labels);
        let selected = if self.value.is_empty() { HUB_PICKS } else { &selected };
        picker.set_selected_by_label(selected, cx);
        // The picker's own label already counts the ready nodes; the GPU
        // list under a chosen model was clutter (user, 2026-09-04). The note
        // stays only for a model no node can serve, where it names why.
        let note = self
            .models
            .iter()
            .find(|model| model.id == self.value && model.dimmed)
            .map(|model| model.note.as_str())
            .unwrap_or_default();
        let note_label = self.view.label(cx, ids!(note));
        note_label.set_text(cx, note);
        note_label.set_visible(cx, !note.is_empty());
        self.view.redraw(cx);
    }

    /// The label the user picked, as the `model` param value.
    pub fn picked(&self, cx: &mut Cx, actions: &Actions) -> Option<String> {
        let index = self
            .view
            .combo_box(cx, ids!(select.picker))
            .changed(actions)?;
        Some(
            index
                .checked_sub(1)
                .and_then(|index| self.models.get(index))
                .map(|model| model.id.clone())
                .unwrap_or_else(|| {
                    if index == 0 {
                        String::new()
                    } else {
                        self.value.clone()
                    }
                }),
        )
    }
}

/// Registers the Rust-backed face widgets into `mod.flow.ui` of an isolate.
pub fn register_face_widgets(vm: &mut ScriptVm) {
    self::script_mod(vm);
}

/// The main app only needs the value viewer, but the face widget module is
/// intentionally one registration unit. Give it the same empty namespace an
/// isolate receives from the headless prelude before registering it.
pub fn register_host_widgets(vm: &mut ScriptVm) {
    vm.new_module(id!(flow));
    vm.eval(script! { mod.flow.ui = {} });
    register_face_widgets(vm);
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// What a face handler asked the flow for; posted as an action, acted on by
/// the app on the next dispatch (never inside the handler).
#[derive(Clone, Debug)]
pub enum BridgeCall {
    Input {
        node: String,
        port: String,
        /// The value as JSON text (a string value arrives quoted).
        value_json: String,
    },
    Run {
        outputs: Option<Vec<String>>,
    },
    Cancel,
    Param {
        node: String,
        key: String,
        value_json: String,
    },
}

#[derive(Clone, Debug)]
pub struct FaceBridgeCall {
    pub instance: String,
    pub call: BridgeCall,
}

type NodeObjects = Rc<RefCell<HashMap<ScriptObject, String>>>;

fn value_text(vm: &ScriptVm<'_>, value: ScriptValue) -> Option<String> {
    if let Some(id) = value.as_id() {
        return id.as_string(|name| name.map(str::to_string));
    }
    vm.bx
        .heap
        .string_with(value, |_heap, text| text.to_string())
}

fn node_name(vm: &ScriptVm<'_>, nodes: &NodeObjects, value: ScriptValue) -> Option<String> {
    if let Some(obj) = value.as_object() {
        return nodes.borrow().get(&obj).cloned();
    }
    value_text(vm, value)
}

fn make_bridge(vm: &mut ScriptVm, instance: String, nodes: NodeObjects) -> ScriptObject {
    let bridge = vm.bx.heap.new_object();
    let empty = vm.bx.heap.new_object();
    vm.bx
        .heap
        .set_value_def(bridge, id!(inputs).into(), empty.into());
    let values = vm.bx.heap.new_object();
    vm.bx
        .heap
        .set_value_def(bridge, id!(values).into(), values.into());
    let state = vm.bx.heap.new_string_from_str("idle");
    vm.bx.heap.set_value_def(bridge, id!(state).into(), state);

    let post = {
        let instance = instance.clone();
        move |call: BridgeCall| {
            Cx::post_action(FaceBridgeCall {
                instance: instance.clone(),
                call,
            });
        }
    };

    {
        let post = post.clone();
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(input),
            script_args_def!(node = NIL, port = NIL, value = NIL),
            move |vm, args| {
                let node = script_value!(vm, args.node);
                let port = script_value!(vm, args.port);
                let value = script_value!(vm, args.value);
                let (Some(node), Some(port)) =
                    (node_name(vm, &nodes, node), value_text(vm, port))
                else {
                    return script_err_invalid_args!(
                        vm.trap(),
                        "flow.input(node, port, value): node and port are required"
                    );
                };
                let mut value_json = String::new();
                vm.bx.heap.to_json_inner(value, &mut value_json);
                post(BridgeCall::Input {
                    node,
                    port,
                    value_json,
                });
                TRUE
            },
        );
    }
    {
        let post = post.clone();
        vm.add_method(
            bridge,
            id_lut!(run),
            script_args_def!(options = NIL),
            move |vm, args| {
                let options = script_value!(vm, args.options);
                let mut outputs = None;
                if let Some(obj) = options.as_object() {
                    let list = vm.bx.heap.value(
                        obj.into(),
                        id!(outputs).into(),
                        vm.bx.threads.cur_ref().trap.pass(),
                    );
                    if let Some(array) = list.as_array() {
                        let mut names = Vec::new();
                        for index in 0..vm.bx.heap.array_len(array) {
                            let item = vm.bx.heap.array_index_unchecked(array, index);
                            if let Some(name) = value_text(vm, item) {
                                names.push(name);
                            }
                        }
                        outputs = Some(names);
                    }
                }
                post(BridgeCall::Run { outputs });
                TRUE
            },
        );
    }
    {
        let post = post.clone();
        vm.add_method(bridge, id_lut!(cancel), script_args_def!(), move |_vm, _args| {
            post(BridgeCall::Cancel);
            TRUE
        });
    }
    {
        let post = post.clone();
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(param),
            script_args_def!(node = NIL, key = NIL, value = NIL),
            move |vm, args| {
                let node = script_value!(vm, args.node);
                let key = script_value!(vm, args.key);
                let value = script_value!(vm, args.value);
                let (Some(node), Some(key)) = (node_name(vm, &nodes, node), value_text(vm, key))
                else {
                    return script_err_invalid_args!(
                        vm.trap(),
                        "flow.param(node, key, value): node and key are required"
                    );
                };
                let mut value_json = String::new();
                vm.bx.heap.to_json_inner(value, &mut value_json);
                post(BridgeCall::Param {
                    node,
                    key,
                    value_json,
                });
                TRUE
            },
        );
    }
    {
        let nodes = nodes.clone();
        vm.add_method(
            bridge,
            id_lut!(value),
            script_args_def!(node = NIL, port = NIL),
            move |vm, args| {
                let me = script_value!(vm, args.self);
                let node = script_value!(vm, args.node);
                let port = script_value!(vm, args.port);
                let (Some(node), Some(port)) =
                    (node_name(vm, &nodes, node), value_text(vm, port))
                else {
                    return NIL;
                };
                let Some(me) = me.as_object() else {
                    return NIL;
                };
                let values = vm.bx.heap.value(
                    me.into(),
                    id!(values).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                );
                let Some(values) = values.as_object() else {
                    return NIL;
                };
                let by_node = vm.bx.heap.value(
                    values.into(),
                    LiveId::from_str(&node).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                );
                let Some(by_node) = by_node.as_object() else {
                    return NIL;
                };
                vm.bx.heap.value(
                    by_node.into(),
                    LiveId::from_str(&port).into(),
                    vm.bx.threads.cur_ref().trap.pass(),
                )
            },
        );
    }
    bridge
}

// ---------------------------------------------------------------------------
// Heap helpers (the graph module keeps its own private copies)
// ---------------------------------------------------------------------------

fn own_value(vm: &ScriptVm<'_>, obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    let key: ScriptValue = LiveId::from_str(name).into();
    let data = vm.bx.heap.object_data(obj);
    data.map_get(&key).or_else(|| {
        data.vec
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map(|entry| entry.value)
    })
}

fn deep_value(vm: &ScriptVm<'_>, mut obj: ScriptObject, name: &str) -> Option<ScriptValue> {
    let mut depth = 0;
    loop {
        if let Some(value) = own_value(vm, obj, name) {
            return Some(value);
        }
        obj = vm.bx.heap.proto(obj).as_object()?;
        depth += 1;
        if depth > 64 {
            return None;
        }
    }
}

fn make_mod(file: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: file.to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

fn eval_checked(vm: &mut ScriptVm, file: &str, code: &str) -> Result<ScriptValue, String> {
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(FLOW_INSTRUCTION_LIMIT, |vm| {
        vm.eval(make_mod(file, code))
    });
    let errors = vm.take_errors();
    vm.bx.captured_errors = Some(Vec::new());
    if let Some(error) = errors.first() {
        return Err(error.trim().to_string());
    }
    Ok(value)
}

fn json_to_script(vm: &mut ScriptVm<'_>, value: &makepad_strict_json::Value) -> ScriptValue {
    use makepad_strict_json::Value as Json;
    match value {
        Json::Null => NIL,
        Json::Bool(value) => ScriptValue::from_bool(*value),
        Json::Int(value) => ScriptValue::from_f64(*value as f64),
        Json::F64(value) => ScriptValue::from_f64(*value),
        Json::Str(value) => vm.bx.heap.new_string_from_str(value),
        Json::Arr(values) => {
            let array = vm.bx.heap.new_array();
            for value in values {
                let value = json_to_script(vm, value);
                vm.bx.heap.array_push_unchecked(array, value);
            }
            array.into()
        }
        Json::Obj(values) => {
            let object = vm.bx.heap.new_object();
            for (name, value) in values {
                let value = json_to_script(vm, value);
                vm.bx
                    .heap
                    .set_value_def(object, LiveId::from_str(name).into(), value);
            }
            object.into()
        }
    }
}

/// A value as a handler sees it: text / json inline, media as a handle.
fn value_to_script(
    vm: &mut ScriptVm<'_>,
    value: &ValueRef,
    bytes: Option<&ValueBytes>,
) -> ScriptValue {
    match value.ty {
        PortType::Text => {
            let text = bytes
                .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
                .or_else(|| preview_text(value))
                .unwrap_or_default();
            vm.bx.heap.new_string_from_str(&text)
        }
        PortType::Json | PortType::List => {
            let text = bytes
                .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
                .or_else(|| preview_text(value))
                .unwrap_or_default();
            match makepad_strict_json::parse(text.as_bytes()) {
                Ok(json) => json_to_script(vm, &json),
                Err(_) => vm.bx.heap.new_string_from_str(&text),
            }
        }
        _ => {
            let object = vm.bx.heap.new_object();
            let digest = vm.bx.heap.new_string_from_str(&value.digest);
            vm.bx
                .heap
                .set_value_def(object, id!(digest).into(), digest);
            let content_type = vm.bx.heap.new_string_from_str(&value.content_type);
            vm.bx
                .heap
                .set_value_def(object, id!(content_type).into(), content_type);
            vm.bx.heap.set_value_def(
                object,
                id!(bytes).into(),
                ScriptValue::from_f64(value.bytes as f64),
            );
            object.into()
        }
    }
}

pub fn preview_text(value: &ValueRef) -> Option<String> {
    match &value.preview {
        Some(Literal::Str(text)) => Some(text.clone()),
        Some(Literal::Obj(fields)) => {
            let width = fields.iter().find(|(k, _)| k == "width").map(|(_, v)| v);
            let height = fields.iter().find(|(k, _)| k == "height").map(|(_, v)| v);
            match (width, height) {
                (Some(Literal::Num(w)), Some(Literal::Num(h))) => Some(format!(
                    "{} {}×{} · {}",
                    value.content_type,
                    w,
                    h,
                    size_text(value.bytes)
                )),
                _ => Some(format!("{} · {}", value.content_type, size_text(value.bytes))),
            }
        }
        _ => None,
    }
}

fn stream_scroll_for(show: &Bind) -> Option<WidgetRef> {
    show.stream_scroll.clone().or_else(|| {
        (show.widget.borrow::<ValueText>().is_some()
            || show.widget.borrow::<ValueView>().is_some())
        .then(|| show.widget.child(live_id!(text_scroll)))
        .filter(|scroll| !scroll.is_empty())
    })
}

// ---------------------------------------------------------------------------
// Mounted faces
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Bind {
    pub widget: WidgetRef,
    pub node: String,
    pub port: String,
    stream_scroll: Option<WidgetRef>,
}

#[derive(Default)]
pub struct MountedFace {
    pub root: WidgetRef,
    pub error: Option<String>,
    pub binds: Vec<Bind>,
    pub shows: Vec<Bind>,
    pub params: Vec<(WidgetRef, String)>,
    pub param_binds: Vec<(WidgetRef, String)>,
    param_ranges: HashMap<WidgetUid, (f64, f64, f64)>,
    pub format_pickers: Vec<WidgetRef>,
    pub dropdowns: Vec<WidgetRef>,
    text_scrolls: Vec<WidgetRef>,
    flexible_roots: Vec<WidgetRef>,
    card_sized: bool,
    /// Ask controls are staged until this explicit button is pressed.
    pub answer_button: Option<WidgetRef>,
    pub on_value: Option<ScriptFnRef>,
    pub on_state: Option<ScriptFnRef>,
}

/// One instance's isolate and everything mounted in it.
pub struct FaceHost {
    pub instance: String,
    vm_id: SplashVmId,
    heap_key: usize,
    node_objects: NodeObjects,
    bridge: Option<ScriptObjectRef>,
    pub faces: HashMap<String, MountedFace>,
    pub flow_face: Option<MountedFace>,
    /// The flow file failed to evaluate in the isolate (the server's graph
    /// still draws; only the faces are missing).
    pub error: Option<String>,
    deltas: HashMap<(String, String), String>,
    /// The last output per (node, port) that reached a face, for re-pushes
    /// when its bytes arrive.
    pub last_values: HashMap<(String, String), ValueRef>,
    /// Digests a face wants the bytes of (an image preview).
    pub wanted: Vec<String>,
    /// Window mapping for node faces living in the canvas draw list.
    camera_transform: Option<PopupAnchorTransform>,
    /// Latest local edit for each Ask output. These deliberately do not enter
    /// the app's pending-input journal until the Answer button is pressed.
    staged_asks: HashMap<(String, String), String>,
    /// Canvas back-to-front order, used to offer hits to the visually
    /// frontmost face first.
    event_order: Vec<String>,
    paused_stream_scrolls: HashSet<WidgetUid>,
    pending_stream_scrolls: HashSet<WidgetUid>,
}

fn is_text_scroll(
    vm: &ScriptVm<'_>,
    widget: &WidgetRef,
    text_scroll_proto: Option<ScriptObject>,
) -> bool {
    text_scroll_proto.is_some_and(|prototype| {
        let source = widget.script_source();
        source != ScriptObject::ZERO
            && vm
                .construction_chain(source.into())
                .iter()
                .any(|level| level.object == prototype)
    })
}

fn collect_widgets(
    vm: &ScriptVm<'_>,
    root: &WidgetRef,
    text_scroll_proto: Option<ScriptObject>,
    enclosing_scroll: Option<WidgetRef>,
    out: &mut Vec<(WidgetRef, Option<WidgetRef>, bool)>,
) {
    if root.is_empty() {
        return;
    }
    let is_text_scroll = is_text_scroll(vm, root, text_scroll_proto);
    let enclosing_scroll = if is_text_scroll {
        Some(root.clone())
    } else {
        enclosing_scroll
    };
    out.push((root.clone(), enclosing_scroll.clone(), is_text_scroll));
    root.children(&mut |_, child| {
        collect_widgets(
            vm,
            &child,
            text_scroll_proto,
            enclosing_scroll.clone(),
            out,
        )
    });
}

/// Collect the outermost flexible item in each branch. Value widgets own
/// their internal image/scroll layout, so their descendants must not compete
/// with the wrapper for the face's remaining height.
fn collect_flexible_roots(
    vm: &ScriptVm<'_>,
    root: &WidgetRef,
    text_scroll_proto: Option<ScriptObject>,
    out: &mut Vec<WidgetRef>,
) {
    if root.is_empty() {
        return;
    }
    if root.borrow::<ValueImage>().is_some()
        || root.borrow::<ValueText>().is_some()
        || root.borrow::<ValueView>().is_some()
        || root
            .borrow::<TextInput>()
            .is_some_and(|input| input.is_multiline())
        || is_text_scroll(vm, root, text_scroll_proto)
    {
        out.push(root.clone());
        return;
    }
    root.children(&mut |_, child| {
        collect_flexible_roots(vm, &child, text_scroll_proto, out)
    });
}

fn set_flexible_card_layout(widget: &WidgetRef, cx: &mut Cx, sized: bool) {
    if let Some(mut image) = widget.borrow_mut::<ValueImage>() {
        image.set_card_sized(cx, sized);
    } else if let Some(mut text) = widget.borrow_mut::<ValueText>() {
        text.set_card_sized(cx, sized);
    } else if let Some(mut value) = widget.borrow_mut::<ValueView>() {
        value.set_card_sized(cx, sized);
    } else if let Some(mut input) = widget.borrow_mut::<TextInput>() {
        // A text area fills a sized card and keeps its default height in a
        // card that fits its content.
        input.set_height(cx, if sized { Size::fill() } else { Size::Fixed(96.0) });
    } else if let Some(mut scroll) = widget.borrow_mut::<View>() {
        scroll.walk.height = text_scroll_height(sized);
        scroll.redraw(cx);
    }
}

fn subtree_owns_area(root: &WidgetRef, cx: &Cx, area: Area) -> bool {
    if root.area() == area {
        return true;
    }
    if root
        .borrow::<FabValueInput>()
        .is_some_and(|field| field.text_ime_anchor(cx).is_some())
    {
        return true;
    }
    let mut found = false;
    root.children(&mut |_, child| {
        if !found {
            found = subtree_owns_area(&child, cx, area);
        }
    });
    found
}

fn transformed_ime_cursor(
    local_cursor: Rect,
    local_area_pos: DVec2,
    transform: PopupAnchorTransform,
) -> Rect {
    let screen = transform.rect(local_cursor);
    Rect {
        pos: screen.pos - local_area_pos,
        size: screen.size,
    }
}

fn reposition_text_ime(cx: &mut Cx, root: &WidgetRef, transform: PopupAnchorTransform) {
    fn visit(cx: &mut Cx, root: &WidgetRef, transform: PopupAnchorTransform) -> bool {
        let text_anchor = root.borrow::<TextInput>().and_then(|input| {
            let area = root.area();
            if area.is_empty() || !cx.has_key_focus(area) {
                return None;
            }
            Some((
                area,
                input.cursor_rect_in_absolute(cx)?,
                input.ime_config(),
            ))
        });
        let anchor = text_anchor.or_else(|| {
            root.borrow::<FabValueInput>()
                .and_then(|field| field.text_ime_anchor(cx))
        });
        if let Some((area, local_cursor, config)) = anchor {
            let cursor = transformed_ime_cursor(local_cursor, area.rect(cx).pos, transform);
            cx.show_text_ime_with_config(area, cursor, config);
            return true;
        }
        let mut found = false;
        root.children(&mut |_, child| {
            if !found {
                found = visit(cx, &child, transform);
            }
        });
        found
    }

    visit(cx, root, transform);
}

/// Give each direct `name := Control{bind/param_bind...}` child a real name
/// column. Complex built-in rows (including ModelPicker) already label
/// themselves and are left intact.
fn wrap_declared_inputs(vm: &mut ScriptVm<'_>, root: &WidgetRef) {
    let candidates: Vec<(usize, LiveId, WidgetRef)> = {
        let Some(view) = root.borrow::<View>() else {
            return;
        };
        view.children
            .iter()
            .enumerate()
            .filter_map(|(index, (id, child))| {
                if child.borrow::<ModelPicker>().is_some() {
                    return None;
                }
                // A multi-line text area is its own row: it fills the face
                // and needs no name beside it (the prompt card).
                if child
                    .borrow::<TextInput>()
                    .is_some_and(|input| input.is_multiline())
                {
                    return None;
                }
                let source = child.script_source();
                if source == ScriptObject::ZERO {
                    return None;
                }
                let bound = ["bind", "param_bind"].iter().any(|name| {
                    own_value(vm, source, name).is_some_and(|value| !value.is_nil())
                });
                bound.then(|| (index, *id, child.clone()))
            })
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let row_value = own_value(vm, vm.bx.heap.modules, "flow")
        .and_then(|value| value.as_object())
        .and_then(|flow| own_value(vm, flow, "ui"))
        .and_then(|value| value.as_object())
        .and_then(|ui| own_value(vm, ui, "DeclaredInputRow"));
    let Some(row_value) = row_value else {
        return;
    };
    let strip = root.child(live_id!(params));
    let into_strip = strip.borrow::<View>().is_some();
    let mut rows = Vec::new();
    for (index, id, child) in &candidates {
        let Some(name) = id.as_string(|name| name.map(str::to_string)) else {
            continue;
        };
        let row = WidgetRef::script_from_value(vm, row_value);
        if row.is_empty() {
            continue;
        }
        if let Some(mut slot) = row.child(live_id!(value)).borrow_mut::<View>() {
            slot.children.push((*id, child.clone()));
        }
        row.label(vm.cx_mut(), ids!(name)).set_text(vm.cx_mut(), &name);
        rows.push((*index, *id, row));
    }
    if into_strip {
        if let Some(mut strip) = strip.borrow_mut::<View>() {
            for (_, id, row) in &rows {
                strip.children.push((*id, row.clone()));
            }
        }
        let ids: BTreeSet<_> = rows.iter().map(|(_, id, _)| *id).collect();
        if let Some(mut view) = root.borrow_mut::<View>() {
            view.children.retain(|(id, _)| !ids.contains(id));
        }
    } else {
        for (index, _, row) in rows {
            if let Some(mut view) = root.borrow_mut::<View>() {
                if let Some(entry) = view.children.get_mut(index) {
                    entry.1 = row;
                }
            }
        }
    }
}

/// Which port `@self` means for a node: an input-like node's value lives on
/// its output port (that is the instance's inputs table key), while an Output
/// face displays the value arriving at its input. Anything else's `@self` is
/// its first output for `show` and first input for `bind`.
fn self_port(node: &Node, for_bind: bool) -> Option<String> {
    if node.kind == "input" || node.kind == "ask" {
        return node.outputs.first().map(|port| port.name.clone());
    }
    if for_bind || node.kind == "output" {
        node.inputs.first().map(|input| input.port.clone())
    } else {
        node.outputs.first().map(|port| port.name.clone())
    }
}

impl FaceHost {
    /// Evaluate `source` in a fresh isolate and mount every node's face.
    pub fn mount(
        cx: &mut Cx,
        parent: WidgetUid,
        instance: &str,
        file_name: &str,
        source: &str,
        graph: &Graph,
        catalog: &[NodeTypeCatalog],
    ) -> Self {
        let vm_id = cx.alloc_splash_vm();
        let heap_key = cx.with_script_vm_id_trusted(vm_id, |vm| vm.bx.heap.heap_key());
        let node_objects: NodeObjects = Rc::new(RefCell::new(HashMap::new()));
        let mut host = Self {
            instance: instance.to_string(),
            vm_id,
            heap_key,
            node_objects: node_objects.clone(),
            bridge: None,
            faces: HashMap::new(),
            flow_face: None,
            error: None,
            deltas: HashMap::new(),
            last_values: HashMap::new(),
            wanted: Vec::new(),
            camera_transform: None,
            staged_asks: HashMap::new(),
            event_order: graph.nodes.iter().map(|node| node.id.clone()).collect(),
            paused_stream_scrolls: HashSet::new(),
            pending_stream_scrolls: HashSet::new(),
        };
        let instance_name = instance.to_string();
        let nodes_for_bridge = node_objects.clone();
        let file_name = file_name.to_string();
        let source = source.to_string();
        let node_ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let result: Result<(ScriptObjectRef, ScriptObjectRef), String> = cx
            .with_script_vm_id_trusted(vm_id, |vm| {
                makepad_code_editor::script_mod(vm);
                crate::theme::script_mod(vm);
                vm.new_module(id!(flow));
                eval_checked(vm, PRELUDE_FILE, PRELUDE)?;
                register_face_widgets(vm);
                eval_checked(vm, FACES_FILE, FACES)?;
                eval_checked(vm, RECIPE_FILE, RECIPE_PRELUDE)?;
                let bridge = make_bridge(vm, instance_name, nodes_for_bridge.clone());
                vm.set_injected_global(id!(flow), bridge.into());
                // Faces are the ordinary widgets DSL, so the widget universe
                // is in scope for the flow file; the prefix shares line 1 so
                // every `loc` the server reports still points at the same line.
                let source = format!("use mod.prelude.widgets.* {source}");
                let value = eval_checked(vm, &file_name, &source)?;
                let flow = value
                    .as_object()
                    .ok_or_else(|| "the file's last expression is not a Flow{}".to_string())?;
                let mut objects = nodes_for_bridge.borrow_mut();
                for id in &node_ids {
                    if let Some(obj) = own_value(vm, flow, id).and_then(|value| value.as_object())
                    {
                        objects.insert(obj, id.clone());
                    }
                }
                Ok((
                    vm.bx.heap.new_object_ref(bridge),
                    vm.bx.heap.new_object_ref(flow),
                ))
            });
        let flow = match result {
            Ok((bridge, flow)) => {
                host.bridge = Some(bridge);
                Some(flow)
            }
            Err(error) => {
                // The file failed in the isolate (a face that names an
                // unknown widget, say): the server's graph still draws, with
                // every node wearing its type's default face.
                host.error = Some(error);
                None
            }
        };
        for node in &graph.nodes {
            let face_name = catalog
                .iter()
                .find(|entry| entry.type_name == node.type_name)
                .map(|entry| entry.face.clone())
                .unwrap_or_else(|| "NodeFace".to_string());
            let face = match flow.as_ref() {
                Some(flow) => {
                    host.mount_one(cx, parent, flow, node, Some(&face_name), graph, catalog)
                }
                None => host.mount_value(
                    cx,
                    parent,
                    None,
                    "ui",
                    &node.id,
                    Some(&face_name),
                    graph,
                    catalog,
                ),
            };
            host.faces.insert(node.id.clone(), face);
        }
        let Some(flow) = flow else {
            cx.widget_tree_mark_dirty(parent);
            return host;
        };
        if graph.flow_ui_src.is_some() {
            let flow_obj = flow.as_object();
            let has_face = cx.with_script_vm_id_trusted(vm_id, |vm| {
                deep_value(vm, flow_obj, "ui").is_some_and(|value| value.as_object().is_some())
            });
            if has_face {
                let face = host.mount_value(
                    cx,
                    parent,
                    Some(flow_obj),
                    "ui",
                    "flow",
                    None,
                    graph,
                    catalog,
                );
                host.flow_face = Some(face);
            }
        }
        cx.widget_tree_mark_dirty(parent);
        host
    }

    fn mount_one(
        &mut self,
        cx: &mut Cx,
        parent: WidgetUid,
        flow: &ScriptObjectRef,
        node: &Node,
        default_face: Option<&str>,
        graph: &Graph,
        catalog: &[NodeTypeCatalog],
    ) -> MountedFace {
        let flow_obj = flow.as_object();
        let node_obj = cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            own_value(vm, flow_obj, &node.id).and_then(|value| value.as_object())
        });
        let Some(node_obj) = node_obj else {
            return MountedFace {
                error: Some(format!("{} is not listed in Flow{{}}", node.id)),
                ..Default::default()
            };
        };
        self.mount_value(
            cx,
            parent,
            Some(node_obj),
            "ui",
            &node.id,
            default_face,
            graph,
            catalog,
        )
    }

    /// Mount `owner.<field>` (or the named default face) for `node_id`.
    fn mount_value(
        &mut self,
        cx: &mut Cx,
        parent: WidgetUid,
        owner: Option<ScriptObject>,
        field: &str,
        node_id: &str,
        default_face: Option<&str>,
        graph: &Graph,
        catalog: &[NodeTypeCatalog],
    ) -> MountedFace {
        let vm_id = self.vm_id;
        let node_objects = self.node_objects.clone();
        let graph_node = graph.nodes.iter().find(|node| node.id == node_id).cloned();
        let node_id = node_id.to_string();
        let default_face = default_face.map(str::to_string);
        let mounted = cx.with_script_vm_id_trusted(vm_id, |vm| {
            let mut face_value = owner
                .and_then(|owner| deep_value(vm, owner, field))
                .unwrap_or(NIL);
            let mut face_obj = face_value.as_object();
            vm.bx.captured_errors = Some(Vec::new());
            let mut root = if face_obj.is_some() {
                WidgetRef::script_from_value(vm, face_value)
            } else {
                WidgetRef::empty()
            };
            if root.is_empty() {
                if let Some(name) = default_face.as_deref() {
                    let flow_mod = own_value(vm, vm.bx.heap.modules, "flow")
                        .and_then(|value| value.as_object());
                    let ui = flow_mod
                        .and_then(|module| own_value(vm, module, "ui"))
                        .and_then(|value| value.as_object());
                    if let Some(value) = ui.and_then(|ui| own_value(vm, ui, name)) {
                        face_value = value;
                        face_obj = value.as_object();
                        root = WidgetRef::script_from_value(vm, face_value);
                    }
                }
            }
            let mut errors = vm.take_errors();
            vm.bx.captured_errors = Some(Vec::new());
            let mut face = MountedFace {
                root: root.clone(),
                error: None,
                ..Default::default()
            };
            if root.is_empty() {
                errors.push(format!("{node_id}: the face is not a widget"));
            }
            if !errors.is_empty() {
                face.error = Some(errors.join("\n"));
            }
            wrap_declared_inputs(vm, &root);
            if let Some(face_obj) = face_obj {
                for (hook, slot) in [("on_value", 0usize), ("on_state", 1usize)] {
                    if let Some(fn_obj) = deep_value(vm, face_obj, hook)
                        .and_then(|value| value.as_object())
                        .filter(|obj| vm.bx.heap.is_fn(*obj))
                    {
                        let fn_ref = vm.bx.heap.new_fn_ref(fn_obj);
                        if slot == 0 {
                            face.on_value = Some(fn_ref);
                        } else {
                            face.on_state = Some(fn_ref);
                        }
                    }
                }
            }
            let text_scroll_proto = own_value(vm, vm.bx.heap.modules, "flow")
                .and_then(|value| value.as_object())
                .and_then(|flow| own_value(vm, flow, "ui"))
                .and_then(|value| value.as_object())
                .and_then(|ui| own_value(vm, ui, "TextScroll"))
                .and_then(|value| value.as_object());
            let mut widgets = Vec::new();
            collect_widgets(vm, &root, text_scroll_proto, None, &mut widgets);
            collect_flexible_roots(vm, &root, text_scroll_proto, &mut face.flexible_roots);
            if graph_node.as_ref().is_some_and(|node| node.kind == "ask") {
                let button = root.child(live_id!(answer_button));
                if button.borrow::<Button>().is_some() {
                    face.answer_button = Some(button);
                }
            }
            for (widget, stream_scroll, is_text_scroll) in widgets {
                if is_text_scroll {
                    face.text_scrolls.push(widget.clone());
                }
                if widget.borrow::<FormatPicker>().is_some() {
                    face.format_pickers.push(widget.clone());
                }
                if widget.borrow::<ComboBox>().is_some() || widget.borrow::<DropDown>().is_some() {
                    face.dropdowns.push(widget.clone());
                }
                let src = widget.script_source();
                if src == ScriptObject::ZERO {
                    continue;
                }
                let resolve = |vm: &ScriptVm<'_>, value: ScriptValue, for_bind: bool| -> Option<(String, String)> {
                    if let Some(obj) = value.as_object() {
                        let node = own_value(vm, obj, "node")?;
                        let node = node_name(vm, &node_objects, node)?;
                        let port = own_value(vm, obj, "port").and_then(|port| value_text(vm, port))?;
                        return Some((node, port));
                    }
                    let text = value_text(vm, value)?;
                    if text == "self" {
                        let node = graph_node.as_ref()?;
                        return Some((node.id.clone(), self_port(node, for_bind)?));
                    }
                    if let Some((node, port)) = text.split_once('.') {
                        return Some((node.to_string(), port.to_string()));
                    }
                    Some((node_id.clone(), text))
                };
                if let Some(value) = own_value(vm, src, "bind").filter(|v| !v.is_nil()) {
                    if let Some((node, port)) = resolve(vm, value, true) {
                        face.binds.push(Bind {
                            widget: widget.clone(),
                            node,
                            port,
                            stream_scroll: stream_scroll.clone(),
                        });
                    }
                }
                if let Some(value) = own_value(vm, src, "show").filter(|v| !v.is_nil()) {
                    if let Some((node, port)) = resolve(vm, value, false) {
                        face.shows.push(Bind {
                            widget: widget.clone(),
                            node,
                            port,
                            stream_scroll: stream_scroll.clone(),
                        });
                    }
                }
                if let Some(name) = own_value(vm, src, "param")
                    .filter(|v| !v.is_nil())
                    .and_then(|v| value_text(vm, v))
                {
                    face.params.push((widget.clone(), name));
                }
                if let Some(name) = own_value(vm, src, "param_bind")
                    .filter(|v| !v.is_nil())
                    .and_then(|v| value_text(vm, v))
                {
                    if let Some(range) = graph_node
                        .as_ref()
                        .and_then(|node| node_param_range(node, catalog, &name))
                    {
                        face.param_ranges.insert(widget.widget_uid(), range);
                    }
                    face.param_binds.push((widget.clone(), name));
                }
            }
            face
        });
        let _ = parent;
        if let Some(node) = graph.nodes.iter().find(|node| node.id == node_id) {
            self.fill_params_for(cx, &mounted, node);
            let isolate = enter_isolate(cx, self.vm_id);
            if let Some(options) = format_options_for_node(node, catalog) {
                let dimensions = node_dimensions(node);
                for widget in &mounted.format_pickers {
                    if let Some(mut picker) = widget.borrow_mut::<FormatPicker>() {
                        picker.set_config(cx, options.clone(), dimensions);
                    }
                }
            } else {
                for widget in &mounted.format_pickers {
                    widget.set_visible(cx, false);
                }
            }
            leave_isolate(cx, isolate);
        }
        for show in &mounted.shows {
            let Some(ty) = graph
                .nodes
                .iter()
                .find(|node| node.id == show.node)
                .and_then(|node| {
                    declared_output_type(node).or_else(|| {
                        node.outputs
                            .iter()
                            .find(|port| port.name == show.port)
                            .map(|port| port.ty)
                            .or_else(|| {
                                node.inputs
                                    .iter()
                                    .find(|input| input.port == show.port)
                                    .map(|input| input.ty)
                            })
                    })
                })
            else {
                continue;
            };
            if let Some(mut image) = show.widget.borrow_mut::<ValueImage>() {
                image.set_empty_type(cx, ty);
            } else if let Some(mut view) = show.widget.borrow_mut::<ValueView>() {
                view.set_empty_type(cx, ty);
            }
        }
        mounted
    }

    /// Drop every mounted widget, then the isolate.
    pub fn free(mut self, cx: &mut Cx) {
        self.faces.clear();
        self.flow_face = None;
        self.bridge = None;
        self.node_objects.borrow_mut().clear();
        DropDown::retire_popup_menus_for_heap(cx, self.heap_key);
        cx.free_splash_vm(self.vm_id);
    }

    // -- drawing and events ---------------------------------------------------

    /// Draw one node's face where the caller's turtle is. The face subtree
    /// gets an empty scope: the host's scope data is the canvas's, not
    /// the isolate's.
    pub fn draw_face(&mut self, cx: &mut Cx2d, node: &str, walk: Walk, card_sized: bool) {
        let Some(face) = self.faces.get_mut(node) else {
            return;
        };
        if face.card_sized != card_sized {
            face.card_sized = card_sized;
            if let Some(mut root) = face.root.borrow_mut::<View>() {
                root.walk.height = card_height(card_sized);
                root.redraw(cx);
            }
            let last_flexible = face.flexible_roots.last().map(WidgetRef::widget_uid);
            for flexible in &face.flexible_roots {
                set_flexible_card_layout(
                    flexible,
                    cx,
                    card_sized && Some(flexible.widget_uid()) == last_flexible,
                );
            }
        }
        let root = face.root.clone();
        let text_scrolls = face.text_scrolls.clone();
        if root.is_empty() {
            return;
        }
        let entry = enter_isolate(cx, self.vm_id);
        root.draw_walk_all(cx, &mut Scope::empty(), walk);
        for scroll in text_scrolls {
            if self.pending_stream_scrolls.remove(&scroll.widget_uid()) {
                scroll.set_scroll_pos(cx, dvec2(0.0, f64::MAX));
            }
        }
        leave_isolate(cx, entry);
        if let Some(transform) = self.camera_transform {
            reposition_text_ime(cx, &root, transform);
        }
    }

    pub fn draw_flow_face(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.set_popup_anchor_transform(cx, None);
        let Some(root) = self.flow_face.as_ref().map(|face| face.root.clone()) else {
            return;
        };
        if root.is_empty() {
            return;
        }
        let entry = enter_isolate(cx, self.vm_id);
        root.draw_walk_all(cx, &mut Scope::empty(), walk);
        leave_isolate(cx, entry);
    }

    /// Deliver an event to every mounted face (the ones on screen own the
    /// hit; the rest see nothing they can act on). The faces are laid out in
    /// canvas units under the camera's transform, so pointer positions go
    /// through the inverse camera first, and a hit they claim is written
    /// back to the original event so the canvas does not claim it too.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope, camera: Option<&Camera>) {
        self.set_popup_anchor_transform(cx, camera.map(Camera::popup_anchor_transform));
        let mut roots: Vec<WidgetRef> = self
            .event_order
            .iter()
            .rev()
            .filter_map(|id| self.faces.get(id))
            .map(|face| face.root.clone())
            .filter(|root| !root.is_empty())
            .collect();
        roots.extend(
            self.flow_face
                .iter()
                .map(|face| face.root.clone())
                .filter(|root| !root.is_empty()),
        );
        if roots.is_empty() {
            return;
        }
        let remapped = camera.and_then(|camera| remap_event(event, camera));
        let delivered = remapped.as_ref().unwrap_or(event);
        let scroll_up = matches!(delivered, Event::Scroll(event) if event.scroll.y < 0.0);
        let scroll_bar_press = matches!(delivered, Event::MouseDown(_));
        if scroll_up || scroll_bar_press {
            let abs = match delivered {
                Event::Scroll(event) => Some(event.abs),
                Event::MouseDown(event) => Some(event.abs),
                _ => None,
            };
            if let Some(abs) = abs {
                for scroll in self
                    .faces
                    .values()
                    .chain(self.flow_face.iter())
                    .flat_map(|face| &face.text_scrolls)
                {
                    let rect = scroll.area().clipped_rect(cx);
                    let over_scroll_bar = scroll_bar_press
                        && abs.x >= rect.pos.x + (rect.size.x - 10.0).max(0.0);
                    if rect.contains(abs) && (scroll_up || over_scroll_bar) {
                        self.paused_stream_scrolls.insert(scroll.widget_uid());
                        self.pending_stream_scrolls.remove(&scroll.widget_uid());
                    }
                }
            }
        }
        let entry = enter_isolate(cx, self.vm_id);
        for root in roots {
            root.handle_event(cx, delivered, scope);
        }
        leave_isolate(cx, entry);
        if let Some(remapped) = remapped.as_ref() {
            sync_handled(event, remapped, camera.expect("mapped event requires a camera"));
        }
    }

    pub fn set_z_order(&mut self, order: &[String]) {
        if self.event_order != order {
            self.event_order.clear();
            self.event_order.extend_from_slice(order);
        }
    }

    /// Keyboard events have no position to remap. Once a widget in a node
    /// face owns focus, the app routes those events only through this host so
    /// canvas shortcuts cannot observe them as a second target.
    pub fn owns_key_focus(&self, cx: &Cx) -> bool {
        let focus = cx.key_focus();
        !focus.is_empty()
            && self
                .faces
                .values()
                .chain(self.flow_face.iter())
                .any(|face| subtree_owns_area(&face.root, cx, focus))
    }

    pub fn set_popup_anchor_transform(
        &mut self,
        cx: &mut Cx,
        transform: Option<PopupAnchorTransform>,
    ) {
        self.camera_transform = transform;
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for dropdown in &face.dropdowns {
                if dropdown.borrow::<ComboBox>().is_some() {
                    dropdown
                        .as_combo_box()
                        .set_popup_anchor_transform(cx, transform);
                } else {
                    dropdown
                        .as_drop_down()
                        .set_popup_anchor_transform(cx, transform);
                }
            }
        }
    }

    /// Every face's evaluation error by node: a face's own, plus the flow
    /// file's error attributed to the node whose declaration owns that line.
    pub fn face_errors(&self, graph: &Graph) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for (node, face) in &self.faces {
            if let Some(error) = face.error.as_ref() {
                out.insert(node.clone(), error.clone());
            }
        }
        if let Some(error) = self.error.as_ref() {
            if let Some(owner) = error_owner(error, graph) {
                out.entry(owner).or_insert_with(|| error.clone());
            }
        }
        out
    }

    /// Picture widgets that were clicked → `(node, port)` to open at full size.
    pub fn open_requests(&self, actions: &Actions) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for show in &face.shows {
                let loaded = show
                    .widget
                    .borrow::<ValueImage>()
                    .map(|image| image.is_loaded())
                    .or_else(|| show.widget.borrow::<ValueView>().map(|view| view.is_loaded()))
                    .unwrap_or(false);
                if !loaded {
                    continue;
                }
                let Some(action) = actions.find_widget_action(show.widget.widget_uid()) else {
                    continue;
                };
                if let ViewAction::FingerUp(up) = action.cast() {
                    if up.is_over {
                        out.push((show.node.clone(), show.port.clone()));
                    }
                }
            }
        }
        out
    }

    /// The hub's model list for a node's `model` picker.
    pub fn set_models(&mut self, cx: &mut Cx, node: &str, models: &[ModelChoice]) {
        let Some(face) = self.faces.get(node) else {
            return;
        };
        for (widget, key) in &face.param_binds {
            if key != "model" {
                continue;
            }
            if let Some(mut picker) = widget.borrow_mut::<ModelPicker>() {
                picker.set_models(cx, models.to_vec());
            }
        }
    }

    // -- filling widgets ------------------------------------------------------

    fn fill_params_for(&self, cx: &mut Cx, face: &MountedFace, node: &Node) {
        for (widget, name) in &face.params {
            let text = param_text(node, name);
            set_widget_text(cx, widget, &text);
            if name == "options" {
                if let Some(Literal::Arr(items)) = node_param(node, "options") {
                    let labels: Vec<String> = items.iter().filter_map(literal_text).collect();
                    if widget.borrow::<ComboBox>().is_some() {
                        widget.as_combo_box().set_labels(cx, labels);
                    } else {
                        widget.as_drop_down().set_labels(cx, labels);
                    }
                }
            }
        }
        for (widget, name) in &face.param_binds {
            let Some(value) = node_param(node, name) else {
                continue;
            };
            if let Some(slider) = widget.borrow_mut::<Slider>() {
                if let Literal::Num(number) = value {
                    drop(slider);
                    widget.as_slider().set_value(cx, *number);
                }
                continue;
            }
            if let Some(mut field) = widget.borrow_mut::<FabValueInput>() {
                if let Literal::Num(number) = value {
                    if (field.value() - *number).abs() > 1e-9 {
                        field.set_value(cx, *number);
                    }
                }
                continue;
            }
            if let Some(mut check) = widget.borrow_mut::<CheckBox>() {
                if let Literal::Bool(value) = value {
                    check.set_active(cx, *value, Animate::No);
                }
                continue;
            }
            if let Some(mut color) = widget.borrow_mut::<FabColorPick>() {
                if let Some(text) = literal_text(value) {
                    if let Some((rgba, _)) = parse_hex(&text) {
                        color.set_rgba(cx, rgba);
                    }
                }
                continue;
            }
            if let Some(text) = literal_text(value) {
                set_widget_text(cx, widget, &text);
            }
        }
        if let Some((width, height)) = node_dimensions(node) {
            for widget in &face.format_pickers {
                if let Some(mut picker) = widget.borrow_mut::<FormatPicker>() {
                    picker.set_dimensions(cx, width, height);
                }
            }
        }
        // An input's declared default fills its textbox until the instance
        // carries a value of its own.
        for bind in &face.binds {
            if bind.node != node.id || bind.widget.borrow::<TextInput>().is_none() {
                continue;
            }
            let text = param_text(node, "default");
            if text.is_empty() {
                continue;
            }
            let input = bind.widget.as_text_input();
            if input.text().is_empty() {
                input.set_text(cx, &text);
            }
        }
    }

    /// The graph changed under the instance: refresh every `param` and
    /// `param_bind` widget from the new node params.
    pub fn refresh_params(&mut self, cx: &mut Cx, graph: &Graph) {
        for node in &graph.nodes {
            if let Some(face) = self.faces.get(&node.id) {
                self.fill_params_for(cx, face, node);
            }
        }
    }

    /// Fill every `bind` widget from the instance's inputs table.
    pub fn fill_inputs(&mut self, cx: &mut Cx, row: &InstanceRow) {
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for bind in &face.binds {
                let Some(text) = row.input_text(&bind.node, &bind.port) else {
                    continue;
                };
                if bind.widget.borrow::<TextInput>().is_some() {
                    let input = bind.widget.as_text_input();
                    if input.text() != text {
                        input.set_text(cx, &text);
                    }
                } else if bind.widget.borrow::<ComboBox>().is_some() {
                    bind.widget.as_combo_box().set_selected_by_label(&text, cx);
                } else if bind.widget.borrow::<DropDown>().is_some() {
                    bind.widget.as_drop_down().set_selected_by_label(&text, cx);
                } else if bind.widget.borrow::<Slider>().is_some() {
                    if let Ok(number) = text.parse::<f64>() {
                        bind.widget.as_slider().set_value(cx, number);
                    }
                } else if bind.widget.borrow::<FabValueInput>().is_some() {
                    if let Ok(number) = text.parse::<f64>() {
                        bind.widget.as_fab_value_input().set_value(cx, number);
                    }
                } else if bind.widget.borrow::<CheckBox>().is_some() {
                    if let Ok(value) = text.parse::<bool>() {
                        bind.widget
                            .as_check_box()
                            .set_active(cx, value, Animate::No);
                    }
                } else if bind.widget.borrow::<FabColorPick>().is_some() {
                    if let Some((rgba, _)) = parse_hex(&text) {
                        bind.widget.as_fab_color_pick().set_rgba(cx, rgba);
                    }
                }
            }
        }
        let json = row.inputs.serialize_json();
        self.set_bridge_field(cx, "inputs", &json);
        self.set_bridge_string(cx, "state", &row.state);
    }

    fn set_bridge_field(&mut self, cx: &mut Cx, field: &str, json: &str) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let Ok(parsed) = makepad_strict_json::parse(json.as_bytes()) else {
            return;
        };
        let field = field.to_string();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let value = json_to_script(vm, &parsed);
            vm.bx
                .heap
                .set_value_def(bridge, LiveId::from_str(&field).into(), value);
        });
    }

    fn set_bridge_string(&mut self, cx: &mut Cx, field: &str, text: &str) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let field = field.to_string();
        let text = text.to_string();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let value = vm.bx.heap.new_string_from_str(&text);
            vm.bx
                .heap
                .set_value_def(bridge, LiveId::from_str(&field).into(), value);
        });
    }

    /// Widget changes on `bind` widgets → `(node, port, text)`.
    pub fn bind_changes(&mut self, cx: &Cx, actions: &Actions) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for bind in &face.binds {
                let changed = if bind.widget.borrow::<TextInput>().is_some() {
                    let input = bind.widget.as_text_input();
                    if let Some(text) = input.changed(actions) {
                        Some(text)
                    } else if let Some((text, _)) = input.returned(actions) {
                        Some(text)
                    } else {
                        None
                    }
                } else if bind.widget.borrow::<ComboBox>().is_some() {
                    bind.widget.as_combo_box().changed_label(actions)
                } else if bind.widget.borrow::<DropDown>().is_some() {
                    bind.widget.as_drop_down().changed_label(actions)
                } else if bind.widget.borrow::<Slider>().is_some() {
                    bind.widget
                        .as_slider()
                        .end_slide(actions)
                        .map(|value| value.to_string())
                } else if bind.widget.borrow::<FabValueInput>().is_some() {
                    bind.widget
                        .as_fab_value_input()
                        .changed(actions)
                        .map(|value| value.to_string())
                } else if bind.widget.borrow::<CheckBox>().is_some() {
                    bind.widget
                        .as_check_box()
                        .changed(actions)
                        .map(|value| value.to_string())
                } else if bind.widget.borrow::<FabColorPick>().is_some() {
                    bind.widget.as_fab_color_pick().changed(actions).map(|value| {
                        format_hex([value.x, value.y, value.z, value.w], true)
                    })
                } else {
                    None
                };
                if let Some(value) = changed {
                    let key = (bind.node.clone(), bind.port.clone());
                    if face.answer_button.is_some() {
                        self.staged_asks.insert(key, value);
                    } else {
                        out.push((key.0, key.1, value));
                    }
                }
            }
            if face
                .answer_button
                .as_ref()
                .is_some_and(|button| button.as_button().clicked(actions))
            {
                let mut committed = HashSet::new();
                for bind in &face.binds {
                    let key = (bind.node.clone(), bind.port.clone());
                    if !committed.insert(key.clone()) {
                        continue;
                    }
                    if let Some(value) = self
                        .staged_asks
                        .remove(&key)
                        .or_else(|| current_bound_value(cx, &bind.widget))
                    {
                        out.push((key.0, key.1, value));
                    }
                }
            }
        }
        out
    }

    /// Widget changes on `param_bind` widgets → `(node, key, literal)`.
    pub fn param_changes(
        &self,
        cx: &mut Cx,
        actions: &Actions,
    ) -> Vec<(String, String, Literal)> {
        let mut out = Vec::new();
        for (node, face) in &self.faces {
            for (widget, key) in &face.param_binds {
                if widget.borrow::<Slider>().is_some() {
                    if let Some(value) = widget.as_slider().end_slide(actions) {
                        out.push((node.clone(), key.clone(), Literal::Num(value)));
                    }
                } else if widget.borrow::<FabValueInput>().is_some() {
                    if let Some(value) = widget.as_fab_value_input().ended(actions) {
                        let value = face
                            .param_ranges
                            .get(&widget.widget_uid())
                            .map_or(value, |range| snap_stepped_value(value, *range));
                        widget.as_fab_value_input().set_value(cx, value);
                        out.push((node.clone(), key.clone(), Literal::Num(value)));
                    }
                } else if widget.borrow::<CheckBox>().is_some() {
                    if let Some(value) = widget.as_check_box().changed(actions) {
                        out.push((node.clone(), key.clone(), Literal::Bool(value)));
                    }
                } else if widget.borrow::<FabColorPick>().is_some() {
                    if let Some(value) = widget.as_fab_color_pick().changed(actions) {
                        out.push((
                            node.clone(),
                            key.clone(),
                            Literal::Str(format_hex(
                                [value.x, value.y, value.z, value.w],
                                true,
                            )),
                        ));
                    }
                } else if widget.borrow::<ModelPicker>().is_some() {
                    // Its dropdown is read by `model_changes`.
                } else if widget.borrow::<TextInput>().is_some() {
                    if let Some(text) = widget.as_text_input().changed(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(text)));
                    }
                } else if widget.borrow::<ComboBox>().is_some() {
                    if let Some(label) = widget.as_combo_box().changed_label(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(label)));
                    }
                } else if widget.borrow::<DropDown>().is_some() {
                    if let Some(label) = widget.as_drop_down().changed_label(actions) {
                        out.push((node.clone(), key.clone(), Literal::Str(label)));
                    }
                }
            }
            for widget in &face.format_pickers {
                let dimensions = widget
                    .borrow_mut::<FormatPicker>()
                    .and_then(|mut picker| picker.changed(cx, actions));
                if let Some((width, height)) = dimensions {
                    out.push((node.clone(), "width".into(), Literal::Num(width as f64)));
                    out.push((node.clone(), "height".into(), Literal::Num(height as f64)));
                }
            }
        }
        out
    }

    /// A ModelPicker's dropdown changed → `(node, key, model id)`.
    pub fn model_changes(&self, cx: &mut Cx, actions: &Actions) -> Vec<(String, String, Literal)> {
        let mut out = Vec::new();
        for (node, face) in &self.faces {
            for (widget, key) in &face.param_binds {
                let picked = widget
                    .borrow::<ModelPicker>()
                    .and_then(|picker| picker.picked(cx, actions));
                if let Some(text) = picked {
                    out.push((node.clone(), key.clone(), Literal::Str(text)));
                }
            }
        }
        out
    }

    // -- values, deltas, states ------------------------------------------------

    /// A port produced a value. Display widgets that `show` it are filled;
    /// returns `true` when some widget needs the bytes (an image) and the
    /// caller should fetch them and call again with `bytes`.
    pub fn push_value(
        &mut self,
        cx: &mut Cx,
        node: &str,
        port: &str,
        value: &ValueRef,
        bytes: Option<&ValueBytes>,
    ) -> bool {
        self.last_values
            .insert((node.to_string(), port.to_string()), value.clone());
        self.deltas.remove(&(node.to_string(), port.to_string()));
        let mut wants_bytes = false;
        let text = bytes
            .filter(|_| !value.ty.is_media())
            .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
            .or_else(|| preview_text(value))
            .unwrap_or_else(|| format!("{} · {}", value.content_type, size_text(value.bytes)));
        let mut hooks = Vec::new();
        for (id, face) in self.faces.iter().chain(
            self.flow_face
                .iter()
                .map(|face| (&self.instance, face)),
        ) {
            for show in &face.shows {
                if show.node != node || show.port != port {
                    continue;
                }
                let widget = &show.widget;
                if let Some(mut image) = widget.borrow_mut::<ValueImage>() {
                    match bytes {
                        Some(bytes) if value.ty == PortType::Image => image.set_value(cx, bytes),
                        _ if value.ty == PortType::Image => {
                            wants_bytes = true;
                            image.set_note(cx, "loading…");
                        }
                        _ => image.set_note(cx, &text),
                    }
                } else if let Some(mut view) = widget.borrow_mut::<ValueView>() {
                    match bytes {
                        Some(bytes) if value.ty == PortType::Image => view.set_image(cx, bytes),
                        _ if value.ty == PortType::Image => {
                            wants_bytes = true;
                            view.set_text(cx, "loading…");
                        }
                        _ => view.set_text(cx, &text),
                    }
                } else {
                    if !value.ty.is_media() && bytes.is_none() && value.bytes > 512 {
                        wants_bytes = true;
                    }
                    set_widget_text(cx, widget, &text);
                }
            }
            if let Some(hook) = face.on_value.clone() {
                hooks.push((id.clone(), hook));
            }
        }
        if wants_bytes && !self.wanted.iter().any(|digest| *digest == value.digest) {
            self.wanted.push(value.digest.clone());
        }
        if !hooks.is_empty() {
            let node = node.to_string();
            let port = port.to_string();
            let value = value.clone();
            let bytes = bytes.cloned();
            cx.with_script_vm_id(self.vm_id, |vm| {
                for (_id, hook) in hooks {
                    let node_arg = vm.bx.heap.new_string_from_str(&node);
                    let port_arg: ScriptValue = LiveId::from_str(&port).into();
                    let value_arg = value_to_script(vm, &value, bytes.as_ref());
                    vm.with_instruction_limit(HANDLER_INSTRUCTION_LIMIT, |vm| {
                        vm.call(hook.as_object().into(), &[node_arg, port_arg, value_arg]);
                    });
                }
                for error in vm.take_errors() {
                    log!("flow-ui: on_value handler: {error}");
                }
            });
        }
        self.store_bridge_value(cx, node, port, value, bytes);
        wants_bytes
    }

    fn store_bridge_value(
        &mut self,
        cx: &mut Cx,
        node: &str,
        port: &str,
        value: &ValueRef,
        bytes: Option<&ValueBytes>,
    ) {
        let Some(bridge) = self.bridge.as_ref().map(|bridge| bridge.as_object()) else {
            return;
        };
        let node = node.to_string();
        let port = port.to_string();
        let value = value.clone();
        let bytes = bytes.cloned();
        cx.with_script_vm_id_trusted(self.vm_id, |vm| {
            let values = own_value(vm, bridge, "values")
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| {
                    let values = vm.bx.heap.new_object();
                    vm.bx
                        .heap
                        .set_value_def(bridge, id!(values).into(), values.into());
                    values
                });
            let by_node = own_value(vm, values, &node)
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| {
                    let by_node = vm.bx.heap.new_object();
                    vm.bx.heap.set_value_def(
                        values,
                        LiveId::from_str(&node).into(),
                        by_node.into(),
                    );
                    by_node
                });
            let script_value = value_to_script(vm, &value, bytes.as_ref());
            vm.bx
                .heap
                .set_value_def(by_node, LiveId::from_str(&port).into(), script_value);
        });
    }

    /// Streaming text for a port: appended and shown live.
    pub fn push_delta(&mut self, cx: &mut Cx, node: &str, port: &str, text: &str) {
        let key = (node.to_string(), port.to_string());
        let full = self.deltas.entry(key).or_default();
        full.push_str(text);
        if full.len() > 16 * 1024 {
            let mut cut = full.len() - 16 * 1024;
            while !full.is_char_boundary(cut) {
                cut += 1;
            }
            full.drain(..cut);
        }
        let content_len = full.len();
        full.push_str(STREAM_CARET);
        for face in self.faces.values().chain(self.flow_face.iter()) {
            for show in &face.shows {
                if show.node == node && show.port == port {
                    set_widget_text(cx, &show.widget, full);
                    if let Some(scroll) = stream_scroll_for(show) {
                        let uid = scroll.widget_uid();
                        if !self.paused_stream_scrolls.contains(&uid) {
                            self.pending_stream_scrolls.insert(uid);
                        }
                    }
                }
            }
        }
        full.truncate(content_len);
    }

    /// A node changed state; the face hooks hear about it.
    pub fn push_state(&mut self, cx: &mut Cx, node: &str, state: &str) {
        let hooks: Vec<ScriptFnRef> = self
            .faces
            .values()
            .chain(self.flow_face.iter())
            .filter_map(|face| face.on_state.clone())
            .collect();
        if hooks.is_empty() {
            return;
        }
        let node = node.to_string();
        let state = state.to_string();
        cx.with_script_vm_id(self.vm_id, |vm| {
            for hook in hooks {
                let node_arg = vm.bx.heap.new_string_from_str(&node);
                let state_arg = vm.bx.heap.new_string_from_str(&state);
                vm.with_instruction_limit(HANDLER_INSTRUCTION_LIMIT, |vm| {
                    vm.call(hook.as_object().into(), &[node_arg, state_arg]);
                });
            }
            for error in vm.take_errors() {
                log!("flow-ui: on_state handler: {error}");
            }
        });
    }

    /// A new run started: streamed text starts over.
    pub fn reset_run(&mut self) {
        self.deltas.clear();
        self.staged_asks.clear();
        self.paused_stream_scrolls.clear();
        self.pending_stream_scrolls.clear();
    }

    /// Bytes arrived for a wanted digest: re-push every value that has it.
    pub fn deliver_bytes(&mut self, cx: &mut Cx, cache: &mut ValueCache, digest: &str) {
        self.wanted.retain(|wanted| wanted != digest);
        let Some(bytes) = cache.get(digest) else {
            return;
        };
        let matching: Vec<((String, String), ValueRef)> = self
            .last_values
            .iter()
            .filter(|(_, value)| value.digest == digest)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        for ((node, port), value) in matching {
            self.push_value(cx, &node, &port, &value, Some(&bytes));
        }
    }
}

impl NodeFaces for FaceHost {
    fn draw_face(&mut self, cx: &mut Cx2d, node: &str, walk: Walk, card_sized: bool) {
        FaceHost::draw_face(self, cx, node, walk, card_sized);
    }

    fn set_z_order(&mut self, order: &[String]) {
        FaceHost::set_z_order(self, order);
    }

    fn set_popup_anchor_transform(
        &mut self,
        cx: &mut Cx,
        transform: Option<PopupAnchorTransform>,
    ) {
        FaceHost::set_popup_anchor_transform(self, cx, transform);
    }
}

fn current_bound_value(cx: &Cx, widget: &WidgetRef) -> Option<String> {
    if widget.borrow::<TextInput>().is_some() {
        Some(widget.as_text_input().text())
    } else if widget.borrow::<ComboBox>().is_some() {
        Some(widget.as_combo_box().selected_label())
    } else if widget.borrow::<DropDown>().is_some() {
        Some(widget.as_drop_down().selected_label())
    } else if widget.borrow::<Slider>().is_some() {
        widget.as_slider().value().map(|value| value.to_string())
    } else if widget.borrow::<FabValueInput>().is_some() {
        Some(widget.as_fab_value_input().value().to_string())
    } else if widget.borrow::<CheckBox>().is_some() {
        Some(widget.as_check_box().active(cx).to_string())
    } else if let Some(color) = widget.borrow::<FabColorPick>() {
        Some(format_hex(color.rgba(), true))
    } else {
        None
    }
}

fn node_param<'a>(node: &'a Node, name: &str) -> Option<&'a Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .or_else(|| {
            node.inputs.iter().find_map(|input| match &input.value {
                makepad_flow::NodeInputValue::Literal(value) if input.port == name => Some(value),
                _ => None,
            })
        })
}

fn literal_text(value: &Literal) -> Option<String> {
    match value {
        Literal::Null => None,
        Literal::Bool(value) => Some(value.to_string()),
        Literal::Num(value) => Some(if value.fract() == 0.0 {
            (*value as i64).to_string()
        } else {
            value.to_string()
        }),
        Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
        Literal::Arr(values) => Some(
            values
                .iter()
                .filter_map(literal_text)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Literal::Obj(_) => Some(makepad_flow::Literal::serialize_json(value)),
    }
}

/// The text a `param:` widget shows for a node param (`run` and `ui` are
/// the source spans the graph carries).
pub fn param_text(node: &Node, name: &str) -> String {
    match name {
        "run" => node.fn_src.clone().unwrap_or_default(),
        "ui" => node.face_src.clone().unwrap_or_default(),
        "domain" => node.domain.clone().unwrap_or_default(),
        // One line for a gen node's picture params: `1024 × 1024 · 8 steps · seed 7 · hub picks`.
        "summary" => {
            let mut parts = Vec::new();
            let num = |key: &str| match node_param(node, key) {
                Some(Literal::Num(value)) => Some(*value as i64),
                _ => None,
            };
            if let (Some(w), Some(h)) = (num("width"), num("height")) {
                parts.push(format!("{w} × {h}"));
            }
            if let Some(steps) = num("steps") {
                parts.push(format!("{steps} steps"));
            }
            if let Some(seed) = num("seed") {
                parts.push(format!("seed {seed}"));
            }
            if let Some(seconds) = num("seconds") {
                parts.push(format!("{seconds} s"));
            }
            let model = node_param(node, "model")
                .and_then(literal_text)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| HUB_PICKS.to_string());
            parts.push(model);
            parts.join(" · ")
        }
        _ => match node_param(node, name) {
            Some(value) => literal_text(value).unwrap_or_default(),
            None => match node.inputs.iter().find(|input| input.port == name) {
                Some(input) => match &input.value {
                    makepad_flow::NodeInputValue::Edge(edge) => {
                        format!("← {}.{}", edge.from_node, edge.from_port)
                    }
                    makepad_flow::NodeInputValue::Literal(value) => {
                        literal_text(value).unwrap_or_default()
                    }
                },
                None => String::new(),
            },
        },
    }
}

/// Set text on whatever the widget is (value widgets, labels, markdown,
/// code views, text inputs).
fn set_widget_text(cx: &mut Cx, widget: &WidgetRef, text: &str) {
    if widget.borrow::<CodeView>().is_some() {
        widget.set_text(cx, text);
        return;
    }
    if widget.borrow::<TextInput>().is_some() {
        let input = widget.as_text_input();
        if input.text() != text {
            input.set_text(cx, text);
        }
        return;
    }
    if widget.borrow::<ComboBox>().is_some() {
        widget.as_combo_box().set_selected_by_label(text, cx);
        return;
    }
    if widget.borrow::<DropDown>().is_some() {
        widget.as_drop_down().set_selected_by_label(text, cx);
        return;
    }
    widget.set_text(cx, text);
}

// ---------------------------------------------------------------------------
// Camera-mapped events
// ---------------------------------------------------------------------------

/// A pointer event with its positions mapped through the inverse camera, for
/// faces laid out in canvas units; `None` for events without positions.
fn remap_event(event: &Event, camera: &Camera) -> Option<Event> {
    Some(match event {
        Event::MouseDown(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseDown(e)
        }
        Event::MouseMove(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.lock_delta /= camera.scale;
            Event::MouseMove(e)
        }
        Event::MouseUp(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseUp(e)
        }
        Event::MouseLeave(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::MouseLeave(e)
        }
        Event::Scroll(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            e.scroll /= camera.scale;
            Event::Scroll(e)
        }
        Event::LongPress(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::LongPress(e)
        }
        Event::TouchUpdate(e) => {
            let mut e = e.clone();
            for touch in &mut e.touches {
                touch.abs = camera.screen_to_local(touch.abs);
                touch.radius /= camera.scale;
            }
            Event::TouchUpdate(e)
        }
        Event::SelectionHandleDrag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::SelectionHandleDrag(e)
        }
        Event::Drag(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drag(e)
        }
        Event::Drop(e) => {
            let mut e = e.clone();
            e.abs = camera.screen_to_local(e.abs);
            Event::Drop(e)
        }
        Event::TweakRay(e) => Event::TweakRay(TweakRayEvent {
            abs: camera.screen_to_local(e.abs),
            window_id: e.window_id,
            modifiers: e.modifiers,
            time: e.time,
            dpi_factor: e.dpi_factor,
            hit_widget_uids: RefCell::new(e.hit_widget_uids.borrow().clone()),
            hit_rect: std::cell::Cell::new(e.hit_rect.get()),
        }),
        _ => return None,
    })
}

/// A hit the faces claimed on the mapped clone is a hit on the original.
fn sync_handled(original: &Event, remapped: &Event, camera: &Camera) {
    match (original, remapped) {
        (Event::MouseDown(a), Event::MouseDown(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseMove(a), Event::MouseMove(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::MouseLeave(a), Event::MouseLeave(b)) => {
            if a.handled.get().is_empty() {
                a.handled.set(b.handled.get());
            }
        }
        (Event::Scroll(a), Event::Scroll(b)) => {
            if b.handled_x.get() {
                a.handled_x.set(true);
            }
            if b.handled_y.get() {
                a.handled_y.set(true);
            }
        }
        (Event::TouchUpdate(a), Event::TouchUpdate(b)) => {
            for (x, y) in a.touches.iter().zip(b.touches.iter()) {
                if x.handled.get().is_empty() {
                    x.handled.set(y.handled.get());
                }
                if x.sweep_lock.get().is_empty() {
                    x.sweep_lock.set(y.sweep_lock.get());
                }
            }
        }
        (Event::TweakRay(a), Event::TweakRay(b)) => {
            *a.hit_widget_uids.borrow_mut() = b.hit_widget_uids.borrow().clone();
            a.hit_rect.set(b.hit_rect.get().map(|rect| Rect {
                pos: camera.local_to_screen(rect.pos),
                size: rect.size * camera.scale,
            }));
        }
        _ => {}
    }
}

/// The node whose declaration owns the line an `file:line:col: message`
/// error points at: the last node declared at or before it.
fn error_owner(error: &str, graph: &Graph) -> Option<String> {
    let mut parts = error.splitn(4, ':');
    let _file = parts.next()?;
    let line: u32 = parts.next()?.trim().parse().ok()?;
    graph
        .nodes
        .iter()
        .filter(|node| node.loc.line <= line)
        .max_by_key(|node| node.loc.line)
        .map(|node| node.id.clone())
}

/// `235 KB`, `1.2 MB`, `640 B`.
pub fn size_text(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::{FleetNodeDto, ModelInfoDto};
    use makepad_widgets::makepad_platform::event::{ScrollEvent, ScrollPhase};
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    fn model(id: &str, node: &str, available: bool, state: &str) -> ModelInfoDto {
        ModelInfoDto {
            id: id.into(),
            domain: "image".into(),
            backend: "test".into(),
            node: node.into(),
            available,
            gated: false,
            state: state.into(),
            vram_gb: None,
            note: None,
        }
    }

    #[test]
    fn models_are_deduped_counted_and_ready_first() {
        let response = ModelsResponse {
            nodes: (1..=6)
                .map(|index| FleetNodeDto {
                    base_url: format!("10.0.0.{index}"),
                    fleet: "test".into(),
                    healthy: true,
                    gpu: (index == 1).then(|| "RTX PRO 6000".into()),
                    vram_total_mb: None,
                    vram_usable_mb: None,
                    vram_free_mb: None,
                })
                .collect(),
            models: vec![
                model("flux2-dev", "10.0.0.1", true, "ready"),
                model("flux2-dev", "10.0.0.2", true, "loaded"),
                model("flux2-dev", "10.0.0.3", true, "absent"),
                model("flux2-dev", "10.0.0.4", true, "absent"),
                model("flux2-dev", "10.0.0.5", true, "absent"),
                model("flux2-dev", "10.0.0.6", true, "absent"),
            ],
            snapshot_ms: 1,
        };
        let choices = model_choices(&response);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, "flux2-dev");
        assert_eq!(choices[0].label, "flux2-dev · 2 ready · 4 absent");
        assert_eq!(
            choices[0].note,
            "10.0.0.1 RTX PRO 6000 · 10.0.0.2"
        );
        assert!(!choices[0].dimmed);
    }

    #[test]
    fn model_with_no_admissible_node_is_dimmed_with_the_capacity_reason() {
        let mut too_small = model("flux2-dev", "10.0.0.217", false, "too_small");
        too_small.note = Some("needs 31744 MB, this card can free 30603".into());
        let response = ModelsResponse {
            nodes: vec![FleetNodeDto {
                base_url: "10.0.0.217".into(),
                fleet: "test".into(),
                healthy: true,
                gpu: Some("RTX 5090".into()),
                vram_total_mb: Some(32_607),
                vram_usable_mb: Some(30_603),
                vram_free_mb: Some(29_785),
            }],
            models: vec![too_small],
            snapshot_ms: 1,
        };
        let choices = model_choices(&response);
        assert_eq!(choices[0].label, "flux2-dev · 1 too small");
        assert!(choices[0].dimmed);
        assert_eq!(
            choices[0].note,
            "needs 31744 MB, this card can free 30603"
        );
    }

    #[test]
    fn format_preset_maps_to_dimensions() {
        let presets: Vec<_> = IMAGE_FORMAT_PRESETS
            .iter()
            .map(|(name, width, height)| FormatPreset::new(*name, *width, *height))
            .collect();
        let portrait = presets
            .iter()
            .find(|preset| preset.name == "768×1024 (3:4)")
            .unwrap();
        assert_eq!(portrait.dimensions(), (768, 1024));
    }

    #[test]
    fn dimensions_map_to_preset_name_or_custom() {
        let presets: Vec<_> = IMAGE_FORMAT_PRESETS
            .iter()
            .map(|(name, width, height)| FormatPreset::new(*name, *width, *height))
            .collect();
        assert_eq!(format_preset_name(&presets, 1280, 720), "1280×720 (16:9)");
        assert_eq!(format_preset_name(&presets, 1111, 777), CUSTOM_FORMAT);
    }

    #[test]
    fn stepped_values_snap_to_absolute_multiples_and_clamp() {
        let range = (256.0, 2048.0, 16.0);
        assert_eq!(snap_stepped_value(1064.0, range), 1072.0);
        assert_eq!(snap_stepped_value(248.0, range), 256.0);
        assert_eq!(snap_stepped_value(2057.0, range), 2048.0);
    }

    #[test]
    fn format_presets_are_filtered_per_node_catalog() {
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let graph = makepad_flow::graph::evaluate(
            "use mod.flow.*\nlet image = Image{}\nlet video = Video{}\nlet generic = Gen{width: 800 height: 600}\nFlow{image video generic}\n",
            "<format-presets>",
        )
        .unwrap();
        let node = |name: &str| graph.nodes.iter().find(|node| node.id == name).unwrap();
        let image = format_options_for_node(node("image"), &catalog).unwrap();
        assert_eq!(image.width_range.2, 16.0);
        assert!(image
            .presets
            .iter()
            .all(|preset| preset.width % 16 == 0 && preset.height % 16 == 0));

        let video = format_options_for_node(node("video"), &catalog).unwrap();
        assert_eq!(video.width_range.2, 32.0);
        assert_eq!(video.height_range.2, 32.0);
        assert_eq!(
            video
                .presets
                .iter()
                .map(FormatPreset::dimensions)
                .collect::<Vec<_>>(),
            vec![(640, 352), (864, 480), (960, 544)]
        );

        let generic = format_options_for_node(node("generic"), &catalog).unwrap();
        assert_eq!(generic.presets.len(), IMAGE_FORMAT_PRESETS.len());
    }

    #[test]
    fn face_number_commit_snaps_to_the_catalog_step() {
        let source = "use mod.flow.*\nlet image = Image{}\nFlow{image}\n";
        let graph = makepad_flow::graph::evaluate(source, "<number-snap>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<number-snap>",
            source,
            &graph,
            &catalog,
        );
        let width = host.faces["image"]
            .param_binds
            .iter()
            .find(|(_, key)| key == "width")
            .unwrap()
            .0
            .clone();
        let actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(FabValueInputAction::Ended(1064.0)),
            widget_uid: width.widget_uid(),
            group: None,
        })];
        assert!(host.param_changes(&mut cx, &actions).iter().any(
            |(node, key, value)| node == "image"
                && key == "width"
                && value == &Literal::Num(1072.0)
        ));
        assert_eq!(width.as_fab_value_input().value(), 1072.0);
        host.free(&mut cx);
    }

    #[test]
    fn built_in_text_faces_mount_bounded_scrolls() {
        let source = r#"use mod.flow.*
let llm = Llm{prompt: "hello"}
let function = Fn{in: {} out: [@text] run: |i| {{text: "ok"}}}
let http = Http{url: "https://example.com"}
let ask = Ask{question: "Which?"}
let output = Output{value: llm.text()}
Flow{llm function http ask output}
"#;
        let graph = makepad_flow::graph::evaluate(source, "<text-scrolls>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<text-scrolls>",
            source,
            &graph,
            &catalog,
        );
        for node in ["llm", "function", "http", "ask", "output"] {
            let face = &host.faces[node];
            assert!(face.error.is_none(), "{node}: {:?}", face.error);
            assert!(!face.text_scrolls.is_empty(), "{node} has no text scroll");
        }
        host.free(&mut cx);
    }

    #[test]
    fn placeholder_icon_maps_every_port_type() {
        for (ty, svg) in [
            (PortType::Text, include_str!("../resources/icons/text.svg")),
            (PortType::Image, include_str!("../resources/icons/image.svg")),
            (PortType::Audio, include_str!("../resources/icons/audio.svg")),
            (PortType::Video, include_str!("../resources/icons/video.svg")),
            (PortType::Mesh, include_str!("../resources/icons/mesh.svg")),
            (PortType::Json, include_str!("../resources/icons/json.svg")),
            (PortType::List, include_str!("../resources/icons/json.svg")),
            (PortType::Bytes, include_str!("../resources/icons/bytes.svg")),
        ] {
            assert_eq!(empty_icon_svg(ty), svg, "wrong icon for {}", ty.as_str());
        }
    }

    #[test]
    fn mounted_placeholder_contains_one_type_icon() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash");
        let graph = makepad_flow::graph::evaluate(source, "<placeholder-icon>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<placeholder-icon>",
            source,
            &graph,
            &catalog,
        );
        assert!(host.error.is_none(), "{:?}", host.error);
        // The picture lives on the Output card (the generator card shows
        // only its settings).
        let empty = host
            .faces
            .get("picture")
            .unwrap()
            .root
            .child(live_id!(value))
            .child(live_id!(empty));
        let mut widgets = Vec::new();
        cx.with_script_vm_id_trusted(host.vm_id, |vm| {
            collect_widgets(vm, &empty, None, None, &mut widgets)
        });
        assert_eq!(
            widgets
                .iter()
                .filter(|(widget, _, _)| widget.borrow::<Svg>().is_some())
                .count(),
            1
        );
        host.free(&mut cx);
    }

    #[test]
    fn card_sized_picture_uses_typed_fill_layout_without_script_eval() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash");
        let graph = makepad_flow::graph::evaluate(source, "<typed-picture-size>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<typed-picture-size>",
            source,
            &graph,
            &catalog,
        );
        let preview = host.faces["picture"].root.child(live_id!(value));
        let empty = preview.child(live_id!(empty));
        let image = preview.child(live_id!(image));
        cx.with_vm(|vm| vm.bx.captured_errors = Some(Vec::new()));

        preview
            .borrow_mut::<ValueView>()
            .unwrap()
            .set_card_sized(&mut cx, true);

        assert!(preview.walk(&mut cx).height.is_fill());
        assert!(empty.walk(&mut cx).height.is_fill());
        assert!(image.walk(&mut cx).width.is_fill());
        assert!(image.walk(&mut cx).height.is_fill());
        assert!(matches!(
            image.borrow::<Image>().unwrap().fit(),
            ImageFit::Smallest
        ));
        assert!(cx.with_vm(|vm| vm.take_errors()).is_empty());
        host.free(&mut cx);
    }

    #[test]
    fn named_typed_value_widgets_mount_in_custom_faces() {
        let source = "use mod.flow.*\nlet value = Text{ui: mod.flow.ui.NodeFace{ audio := mod.flow.ui.ValueAudio{} video := mod.flow.ui.ValueVideo{} mesh := mod.flow.ui.ValueMesh{} json := mod.flow.ui.ValueJson{} }}\nFlow{value}\n";
        let graph = makepad_flow::graph::evaluate(source, "<typed-values>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<typed-values>",
            source,
            &graph,
            &catalog,
        );
        assert!(host.error.is_none(), "{:?}", host.error);
        let root = &host.faces.get("value").unwrap().root;
        for id in [live_id!(audio), live_id!(video), live_id!(mesh)] {
            assert!(root.child(id).borrow::<ValueView>().is_some());
        }
        assert!(root.child(live_id!(json)).borrow::<ValueText>().is_some());
        host.free(&mut cx);
    }

    #[test]
    fn ime_cursor_is_reanchored_in_transformed_screen_space() {
        for (scale, expected_pos, expected_size) in [
            (0.5, dvec2(-25.0, -20.0), dvec2(1.0, 9.0)),
            (2.0, dvec2(140.0, 85.0), dvec2(4.0, 36.0)),
        ] {
            let cursor = transformed_ime_cursor(
                rect(110.0, 70.0, 2.0, 18.0),
                dvec2(100.0, 50.0),
                PopupAnchorTransform {
                    scale,
                    translation: dvec2(20.0, -5.0),
                },
            );
            assert_eq!(cursor.pos, expected_pos);
            assert_eq!(cursor.size, expected_size);
        }
    }

    #[test]
    fn zoomed_face_click_scroll_drop_and_tweak_geometry_map_at_half_and_double() {
        for scale in [0.5, 2.0] {
            let camera = Camera {
                view: rect(10.0, 20.0, 800.0, 600.0),
                pan: dvec2(30.0, -5.0),
                scale,
            };
            let screen = dvec2(140.0, 215.0);
            let expected = dvec2(
                makepad_flowgraph::LOCAL_ORIGIN + 100.0 / scale,
                makepad_flowgraph::LOCAL_ORIGIN + 200.0 / scale,
            );
            let click = Event::MouseDown(MouseDownEvent {
                abs: screen,
                button: MouseButton::PRIMARY,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                handled: Cell::new(Area::Empty),
                time: 0.0,
            });
            assert!(matches!(remap_event(&click, &camera), Some(Event::MouseDown(e)) if e.abs == expected));

            let scroll = Event::Scroll(ScrollEvent {
                window_id: WindowId(1, 1),
                scroll: dvec2(8.0, -4.0),
                abs: screen,
                modifiers: KeyModifiers::default(),
                handled_x: Cell::new(false),
                handled_y: Cell::new(false),
                is_mouse: true,
                time: 0.0,
                phase: ScrollPhase::None,
            });
            assert!(matches!(remap_event(&scroll, &camera), Some(Event::Scroll(e)) if e.abs == expected && e.scroll == dvec2(8.0 / scale, -4.0 / scale)));

            let drop = Event::Drop(DropEvent {
                modifiers: KeyModifiers::default(),
                handled: Arc::new(Mutex::new(false)),
                abs: screen,
                items: Arc::new(Vec::new()),
            });
            assert!(matches!(remap_event(&drop, &camera), Some(Event::Drop(e)) if e.abs == expected));

            let original = Event::TweakRay(TweakRayEvent {
                abs: screen,
                window_id: WindowId(1, 1),
                modifiers: KeyModifiers::default(),
                time: 0.0,
                dpi_factor: 1.0,
                hit_widget_uids: RefCell::new(Vec::new()),
                hit_rect: Cell::new(None),
            });
            let mapped = remap_event(&original, &camera).unwrap();
            if let Event::TweakRay(mapped) = &mapped {
                mapped
                    .hit_rect
                    .set(Some(rect(makepad_flowgraph::LOCAL_ORIGIN + 10.0, makepad_flowgraph::LOCAL_ORIGIN + 20.0, 30.0, 40.0)));
            }
            sync_handled(&original, &mapped, &camera);
            let Event::TweakRay(original) = original else { unreachable!() };
            assert_eq!(
                original.hit_rect.get(),
                Some(rect(40.0 + 10.0 * scale, 15.0 + 20.0 * scale, 30.0 * scale, 40.0 * scale))
            );
        }
    }

    #[test]
    fn a_multiline_text_area_is_not_wrapped_in_a_labelled_row() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash");
        let graph = makepad_flow::graph::evaluate(source, "<text-area>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<text-area>",
            source,
            &graph,
            &catalog,
        );
        assert!(host.error.is_none(), "{:?}", host.error);
        let root = host.faces.get("prompt").unwrap().root.clone();
        let area = root.child(live_id!(value));
        assert!(area.borrow::<TextInput>().is_some_and(|input| input.is_multiline()));
        assert!(area.child(live_id!(name)).borrow::<Label>().is_none());
        host.free(&mut cx);
    }

    #[test]
    fn face_declared_picker_is_mounted_as_a_labelled_strip_row() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash");
        let graph = makepad_flow::graph::evaluate(source, "<face-row>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<face-row>",
            source,
            &graph,
            &catalog,
        );
        assert!(host.error.is_none(), "{:?}", host.error);
        let root = host.faces.get("add_style").unwrap().root.clone();
        let row = root.child(live_id!(params)).child(live_id!(style));
        assert_eq!(row.label(&cx, ids!(name)).text(), "style");
        assert!(row
            .child(live_id!(value))
            .child(live_id!(style))
            .borrow::<ComboBox>()
            .is_some());
        host.free(&mut cx);
    }

    #[test]
    fn face_declared_combo_round_trips_its_bound_label() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash");
        let graph = makepad_flow::graph::evaluate(source, "<combo-bind>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<combo-bind>",
            source,
            &graph,
            &catalog,
        );
        let picker = host.faces["add_style"]
            .binds
            .iter()
            .find(|bind| bind.port == "style")
            .expect("style bind")
            .widget
            .as_combo_box();
        assert_eq!(picker.labels(), vec!["photo", "anime", "oil paint"]);
        picker.set_selected_by_label("anime", &mut cx);
        assert_eq!(picker.selected_label(), "anime");

        let actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(ComboBoxAction::Select(2)),
            widget_uid: picker.widget_uid(),
            group: None,
        })];
        assert_eq!(
            host.bind_changes(&cx, &actions),
            vec![("add_style".into(), "style".into(), "oil paint".into())]
        );
        host.free(&mut cx);
    }

    #[test]
    fn repeated_isolate_dropdown_mounts_release_popup_cache_and_vm() {
        let source = include_str!("../../../libs/flow/recipes/templates/prompt-to-image.splash")
            .replace("style := ComboBox", "style := DropDown");
        let graph = makepad_flow::graph::evaluate(&source, "<isolate-retire>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let baseline = DropDown::popup_menu_cache_len(&mut cx);
        for index in 0..4 {
            let host = FaceHost::mount(
                &mut cx,
                WidgetUid(0),
                &format!("test-{index}"),
                "<isolate-retire>",
                &source,
                &graph,
                &catalog,
            );
            let bridge = host.bridge.clone().expect("face bridge");
            assert_eq!(cx.script_ref_vm_id(&bridge), Some(host.vm_id));
            host.free(&mut cx);
            assert_eq!(cx.script_ref_vm_id(&bridge), None);
            assert_eq!(DropDown::popup_menu_cache_len(&mut cx), baseline);
        }
    }

    #[test]
    fn ask_text_is_staged_until_the_answer_button_fires() {
        let source = "use mod.flow.*\nlet ask = Ask{question: \"Which?\"}\nFlow{ask}\n";
        let graph = makepad_flow::graph::evaluate(source, "<ask-stage>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host = FaceHost::mount(
            &mut cx,
            WidgetUid(0),
            "test",
            "<ask-stage>",
            source,
            &graph,
            &catalog,
        );
        let face = host.faces.get("ask").expect("ask face");
        let answer = face
            .binds
            .iter()
            .find(|bind| bind.widget.borrow::<TextInput>().is_some())
            .unwrap()
            .widget
            .clone();
        let button = face.answer_button.clone().expect("explicit Answer button");
        let edit: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(TextInputAction::Changed("draft".into())),
            widget_uid: answer.widget_uid(),
            group: None,
        })];
        assert!(host.bind_changes(&cx, &edit).is_empty());
        let press: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(ButtonAction::Clicked(KeyModifiers::default())),
            widget_uid: button.widget_uid(),
            group: None,
        })];
        assert_eq!(
            host.bind_changes(&cx, &press),
            vec![("ask".into(), "text".into(), "draft".into())]
        );
        host.free(&mut cx);
    }
}
