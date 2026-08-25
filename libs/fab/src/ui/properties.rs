//! Lane D. The properties editor: a vertical icon tab strip, collapsible
//! panels, label-left / value-right rows, and drag-numeric fields that change
//! the thing they name.
//!
//! Function first — every control on every tab is wired:
//! * **Object** — the active element's identity and size; the Hidden checkbox
//!   emits `SetHidden`.
//! * **Element** — the IFC-ish `Property` list the parser found, grouped.
//! * **Material** — the materials this element actually draws with, with
//!   their swatch and PBR numbers.
//! * **Quantities** — the take-off values in a `DataGrid` retinted to the
//!   dark tokens.
//! * **Scene** — model counts, the explode slider (`SetExplode`) and the sun
//!   (`SetSun`: month, day, hour, latitude, north).
//! * **Render** — samples, bounces, preview scale, exposure, denoise
//!   (`SetRenderSettings`).
//!
//! The header pin is real: pinned, the editor keeps showing the element it was
//! pinned to while the selection moves on.

use crate::api::*;
use crate::ui::colorpick::*;
use crate::ui::dragnum::*;
use crate::ui::texview::*;
use crate::ui::widgets::{fold_panel_clicked, set_panel_chevron};
use crate::model::PropertyValue;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let Row = mod.widgets.FabPropRow{}
    // An attribute row wrapped in a tooltip: the row shows the pretty
    // one-line key and value, the tip carries the full raw key and the
    // untruncated value.
    let AttrRow = mod.widgets.FabTipFill{
        visible: false
        mod.widgets.FabPropRow{}
    }
    let Num = mod.widgets.FabDragNumber{}
    // One texture slot of the shown material: label, thumbnail, dimensions.
    // Hidden entirely when the slot is empty.
    let TexRow = View{
        visible: false
        width: Fill
        height: Fit
        flow: Right
        spacing: 6
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 6 top: 2 bottom: 2}
        name := mod.widgets.FabLabelDim{ width: fab.prop_label_width text: "" }
        thumb := mod.widgets.FabTexThumb{}
        dims := mod.widgets.FabLabelSmall{ text: "" }
    }

    mod.widgets.FabPropertiesBase = #(FabProperties::register_widget(vm))
    mod.widgets.FabProperties = set_type_default() do mod.widgets.FabPropertiesBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: {
            color: fab.color_editor
        }
        header := mod.widgets.FabAreaHeader{
            FabTip{ text: "Choose editor"
                editor_type := mod.widgets.FabDropdownButton{ label +: { text: "Properties" } }
            }
            Filler{}
            crumb := mod.widgets.FabLabelDim{ text: "" }
            FabTip{ text: "Pin selected properties"
                pin := mod.widgets.FabIconToggle{
                    width: 20
                    height: 20
                    draw_icon +: { svg: crate_resource("self://resources/icons/pin.svg") }
                }
            }
        }
        body := View{
            width: Fill
            height: Fill
            flow: Right
            tabs := View{
                width: fab.tab_strip_width + 6
                height: Fill
                flow: Down
                padding: Inset{left: 3 top: 4 right: 3 bottom: 4}
                show_bg: true
                draw_bg +: {
                    color: fab.color_editor_alt
                }
                FabTip{ text: "Show object properties"
                    tab_object := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/object.svg") }
                    }
                }
                FabTip{ text: "Show element attributes"
                    tab_element := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/editor_properties.svg") }
                    }
                }
                FabTip{ text: "Show material properties"
                    tab_material := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/material.svg") }
                    }
                }
                FabTip{ text: "Show quantities"
                    tab_quantities := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/layers.svg") }
                    }
                }
                FabTip{ text: "Show scene properties"
                    tab_scene := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/grid.svg") }
                    }
                }
                FabTip{ text: "Show render properties"
                    tab_render := mod.widgets.FabTabIcon{
                        draw_icon +: { svg: crate_resource("self://resources/icons/editor_render.svg") }
                    }
                }
            }

            // ------------------------------------------------- Object
            page_object := mod.widgets.FabScroll{
                padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
                spacing: 2
                object_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Object" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        row_name := Row{ name +: { text: "Name" } }
                        row_class := Row{ name +: { text: "Type" } }
                        row_story := Row{ name +: { text: "Story" } }
                        row_layer := Row{ name +: { text: "Layer" } }
                        row_guid := mod.widgets.FabTipFill{
                            mod.widgets.FabPropRowMono{ name +: { text: "GUID" } }
                        }
                    }
                }
                dims_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Dimensions" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        row_size := Row{ name +: { text: "Size" } }
                        row_centre := Row{ name +: { text: "Centre" } }
                        row_tris := Row{ name +: { text: "Triangles" } }
                    }
                }
                vis_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Visibility" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 10 top: 2 bottom: 6}
                        spacing: 2
                        cb_hidden := mod.widgets.FabCheckBox{ text: "Hidden in viewport" }
                        cb_selected := mod.widgets.FabCheckBox{ text: "Selected" }
                    }
                }
            }

            // ------------------------------------------------- Element
            page_element := mod.widgets.FabScroll{
                visible: false
                padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
                spacing: 2
                props_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Attributes" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        p0 := AttrRow{}
                        p1 := AttrRow{}
                        p2 := AttrRow{}
                        p3 := AttrRow{}
                        p4 := AttrRow{}
                        p5 := AttrRow{}
                        p6 := AttrRow{}
                        p7 := AttrRow{}
                        p8 := AttrRow{}
                        p9 := AttrRow{}
                        p10 := AttrRow{}
                        p11 := AttrRow{}
                        p12 := AttrRow{}
                        p13 := AttrRow{}
                        p14 := AttrRow{}
                        p15 := AttrRow{}
                        p_none := mod.widgets.FabLabelMuted{
                            margin: Inset{left: 8 top: 2}
                            text: "No attributes on this element"
                        }
                    }
                }
            }

            // ------------------------------------------------- Material
            page_material := mod.widgets.FabScroll{
                visible: false
                padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
                spacing: 2
                mat_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Material" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        swatch_row := View{
                            width: Fill
                            height: fab.row_height
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 8 right: 6}
                            spacing: 6
                            mod.widgets.FabLabelDim{ width: fab.prop_label_width text: "Base color" }
                            picker := mod.widgets.FabColorPicker{}
                            hex := mod.widgets.FabLabelMono{ margin: Inset{left: 6} text: "" }
                        }
                        row_mat_name := Row{ name +: { text: "Name" } }
                        row_metallic := Row{ name +: { text: "Metallic" } }
                        row_rough := Row{ name +: { text: "Roughness" } }
                        row_ior := Row{ name +: { text: "IOR" } }
                        row_trans := Row{ name +: { text: "Transmission" } }
                        row_tex := Row{ name +: { text: "Texture" } }
                        row_matsrc := Row{ name +: { text: "Source" } }
                    }
                }
                tex_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Textures" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        tex_base := TexRow{ name +: { text: "Base colour" } }
                        tex_normal := TexRow{ name +: { text: "Normal" } }
                        tex_mr := TexRow{ name +: { text: "Metal · rough" } }
                        tex_emissive := TexRow{ name +: { text: "Emissive" } }
                        tex_none := mod.widgets.FabLabelMuted{
                            margin: Inset{left: 8 top: 4 bottom: 2}
                            text: "No textures — flat base colour"
                        }
                    }
                }
            }

            // ------------------------------------------------- Quantities
            page_quantities := View{
                visible: false
                width: Fill
                height: Fill
                flow: Down
                qty_head := View{
                    width: Fill
                    height: fab.row_height
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 8}
                    qty_title := mod.widgets.FabLabelDim{ text: "Quantities" }
                }
                qty_grid := DataGrid{
                    width: Fill
                    height: Fill
                    rows: 0
                    cols: 2
                    zebra_stripes: true
                    default_col_width: 150.0
                    default_row_height: 20.0
                    row_header_width: 34.0
                    color_bg: fab.color_editor
                    color_cell: fab.color_row_even
                    color_cell_alt: fab.color_row_odd
                    color_text: fab.color_text
                    color_header: fab.color_header
                    color_header_active: fab.color_accent_dim
                    color_header_text: fab.color_text_header
                    color_selection: #x5680c233
                    color_selection_border: fab.color_accent
                    color_drag_marker: fab.color_accent
                    color_resize_guide: fab.color_accent_hover
                    draw_text +: {
                        text_style: theme.font_regular{ font_size: fab.font_size_ui }
                        color: fab.color_text
                    }
                    draw_text_bold +: {
                        text_style: theme.font_bold{ font_size: fab.font_size_ui }
                        color: fab.color_text_header
                    }
                }
            }

            // ------------------------------------------------- Scene
            page_scene := mod.widgets.FabScroll{
                visible: false
                padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
                spacing: 2
                scene_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Scene" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{top: 2 bottom: 6}
                        row_scene := Row{ name +: { text: "Model" } }
                        row_elements := Row{ name +: { text: "Elements" } }
                        row_stories := Row{ name +: { text: "Stories" } }
                        row_layers := Row{ name +: { text: "Layers" } }
                        row_materials := Row{ name +: { text: "Materials" } }
                        row_bounds := Row{ name +: { text: "Bounds" } }
                        row_units := Row{ name +: { text: "Units" } }
                    }
                }
                explode_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Exploded View" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 8 right: 6 top: 2 bottom: 6}
                        spacing: 2
                        num_explode := Num{
                            label: "Amount"
                            min: 0.0
                            max: 1.0
                            step: 0.05
                            snap: 0.1
                            precision: 2
                            show_fill: true
                        }
                        cb_explode_story := mod.widgets.FabCheckBox{ text: "By story" }
                    }
                }
                sun_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Sun" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 8 right: 6 top: 2 bottom: 6}
                        spacing: 2
                        // The same controls the Sun Study sidebar carries, in
                        // the same shape: a value with a meaningful range is a
                        // slider (fill bar, range swept across the row) — the
                        // bare pixel-per-step fields here were the "sun does
                        // not update as I drag" report. Coordinates and the
                        // year keep the unbounded mapping: their ranges are
                        // validity limits, not a track worth sweeping.
                        num_year := Num{ label: "Year" min: 2000.0 max: 2100.0 step: 1.0 snap: 1.0 precision: 0 quantize: true }
                        num_month := Num{ label: "Month" min: 1.0 max: 13.0 step: 1.0 snap: 1.0 precision: 0 wrap: true show_fill: true quantize: true }
                        num_day := Num{ label: "Day" min: 1.0 max: 31.0 step: 1.0 snap: 1.0 precision: 0 show_fill: true quantize: true }
                        num_hour := Num{ label: "Hour" min: 0.0 max: 24.0 step: 0.25 snap: 1.0 wrap: true show_fill: true time_of_day: true text_input +: {is_numeric_only: false} }
                        num_lat := Num{ label: "Latitude" min: -90.0 max: 90.0 step: 1.0 snap: 5.0 precision: 3 suffix: "°" show_fill: true }
                        num_lon := Num{ label: "Longitude" min: -180.0 max: 180.0 step: 1.0 snap: 5.0 precision: 3 suffix: "°" show_fill: true }
                        num_tz := Num{ label: "UTC offset" min: -12.0 max: 14.0 step: 0.25 snap: 0.25 precision: 2 suffix: " h" show_fill: true }
                        num_north := Num{ label: "North" min: -180.0 max: 180.0 step: 1.0 snap: 15.0 precision: 1 wrap: true suffix: "°" show_fill: true }
                        num_turbidity := Num{ label: "Turbidity" min: 1.2 max: 10.0 step: 0.1 snap: 0.5 precision: 1 show_fill: true }
                        num_haze := Num{ label: "Haze" min: 0.0 max: 1.0 step: 0.05 snap: 0.05 precision: 2 show_fill: true }
                        row_elev := Row{ name +: { text: "Elevation" } }
                        cb_shadows := mod.widgets.FabCheckBox{ text: "Shadows" }
                    }
                }
            }

            // ------------------------------------------------- Render
            page_render := mod.widgets.FabScroll{
                visible: false
                padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
                spacing: 2
                sampling_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Sampling" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 8 right: 6 top: 2 bottom: 6}
                        spacing: 2
                        num_samples := Num{ label: "Max samples" min: 64.0 max: 8192.0 step: 16.0 snap: 64.0 precision: 0 }
                        num_bounces := Num{ label: "Bounces" min: 1.0 max: 32.0 step: 1.0 snap: 1.0 precision: 0 quantize: true show_fill: true }
                        num_preview := Num{ label: "Preview scale" min: 0.25 max: 1.0 step: 0.05 snap: 0.25 precision: 2 show_fill: true }
                        row_progress := Row{ name +: { text: "Progress" } }
                    }
                }
                film_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Film" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 8 right: 6 top: 2 bottom: 6}
                        spacing: 2
                        num_exposure := Num{ label: "Exposure EV" min: -6.0 max: 6.0 step: 0.25 snap: 0.5 precision: 2 show_fill: true }
                        num_width := Num{ label: "Width" min: 128.0 max: 7680.0 step: 8.0 snap: 128.0 precision: 0 }
                        num_height := Num{ label: "Height" min: 128.0 max: 4320.0 step: 8.0 snap: 128.0 precision: 0 }
                        cb_denoise := mod.widgets.FabCheckBox{ text: "Denoise" }
                    }
                }
                dof_panel := mod.widgets.FabPanel{
                    header +: { hdr +: { title +: { text: "Depth of Field" } } }
                    body +: {
                        width: Fill height: Fit flow: Down
                        padding: Inset{left: 8 right: 6 top: 2 bottom: 6}
                        spacing: 2
                        num_fstop := Num{ label: "f-stop" min: 0.0 max: 22.0 step: 0.04 snap: 1.0 precision: 2 show_fill: true }
                        num_focus := Num{ label: "Focus dist" min: 0.1 max: 500.0 step: 0.25 snap: 1.0 precision: 2 suffix: " m" }
                    }
                }
            }
        }
        // Enlarged view of a clicked texture thumbnail. Centered modal;
        // click away or Escape closes it. Zero-sized in the panel's own
        // flow — a Modal draws through its overlay pass, but its widget walk
        // would otherwise reserve real height here and clip the pages above.
        tex_modal := Modal{
            width: 0
            height: 0
            content +: {
                tex_view := View{
                    width: Fit
                    height: Fit
                    flow: Down
                    padding: 10
                    spacing: 6
                    align: Align{x: 0.5 y: 0.0}
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                            sdf.fill_keep(fab.color_popover)
                            sdf.stroke(fab.color_popover_border, 1.0)
                            return sdf.result
                        }
                    }
                    big := mod.widgets.FabTexThumb{
                        width: 512
                        height: 512
                    }
                    caption := mod.widgets.FabLabelSmall{ text: "" }
                }
            }
        }
    }
}

const PAGES: [(&[LiveId], &[LiveId]); 6] = [
    (ids!(body.tabs.tab_object), ids!(body.page_object)),
    (ids!(body.tabs.tab_element), ids!(body.page_element)),
    (ids!(body.tabs.tab_material), ids!(body.page_material)),
    (ids!(body.tabs.tab_quantities), ids!(body.page_quantities)),
    (ids!(body.tabs.tab_scene), ids!(body.page_scene)),
    (ids!(body.tabs.tab_render), ids!(body.page_render)),
];

const PANELS: [&[LiveId]; 12] = [
    ids!(body.page_object.object_panel),
    ids!(body.page_object.dims_panel),
    ids!(body.page_object.vis_panel),
    ids!(body.page_element.props_panel),
    ids!(body.page_material.mat_panel),
    ids!(body.page_material.tex_panel),
    ids!(body.page_scene.scene_panel),
    ids!(body.page_scene.explode_panel),
    ids!(body.page_scene.sun_panel),
    ids!(body.page_render.sampling_panel),
    ids!(body.page_render.film_panel),
    ids!(body.page_render.dof_panel),
];

const PROP_ROWS: usize = 16;

#[derive(Script, ScriptHook, Widget)]
pub struct FabProperties {
    #[deref]
    view: View,
    #[rust]
    tab: usize,
    #[rust]
    synced_tab: Option<usize>,
    #[rust]
    pinned: Option<ElementId>,
    #[rust]
    synced_values: bool,
    /// The scene revision the fields were last synced against. A model load
    /// mutates the sun in place (`install_scene` applies site metadata with
    /// no `SetSun` action for the event hook to observe), so without this
    /// the panel keeps showing pre-load values until the user's first edit —
    /// and that edit then snaps latitude/longitude/UTC/north to the model's
    /// site mid-drag, teleporting the sun for no visible reason.
    #[rust]
    synced_scene: Option<u64>,
    /// Panel width the attribute-row label/value split was last computed
    /// for: the value column keeps a readable minimum, the label gives way.
    #[rust]
    attr_split_avail: f64,
    #[rust]
    qty: Vec<(String, String)>,
    /// The material whose colour/textures the Material tab currently shows —
    /// the target of the colour picker's edits.
    #[rust]
    shown_material: Option<MaterialId>,
}

/// One populated texture slot of the shown material, resolved from the
/// editable document (the only place all slots live; the runtime scene keeps
/// base colour only).
struct TexSlotInfo {
    /// Texture name from the document.
    tex_name: String,
    w: u32,
    h: u32,
    rgba: std::sync::Arc<[u8]>,
    /// Stable identity of the decoded image: the allocation pointer of its
    /// pixel data. The thumbnail uploads once per key, never per frame.
    key: u64,
    /// UV repeats per meter, when the source declared one.
    repeat: Option<[f32; 2]>,
}

const TEX_ROW_IDS: [&[LiveId]; 4] = [
    ids!(body.page_material.tex_panel.tex_base),
    ids!(body.page_material.tex_panel.tex_normal),
    ids!(body.page_material.tex_panel.tex_mr),
    ids!(body.page_material.tex_panel.tex_emissive),
];

/// The populated slots of `mat`, in `TEX_ROW_IDS` order (row index, info).
fn material_textures(state: &AppState, mat: MaterialId) -> Vec<(usize, TexSlotInfo)> {
    let Some(doc) = state.scene.document.as_ref() else {
        return Vec::new();
    };
    let Some(m) = doc.materials().get(mat.index()) else {
        return Vec::new();
    };
    let slots = [
        (0usize, &m.base_color_texture),
        (1, &m.normal_texture),
        (2, &m.metallic_roughness_texture),
        (3, &m.emissive_texture),
    ];
    let mut out = Vec::new();
    for (row, slot) in slots {
        let Some(slot) = slot.as_ref() else { continue };
        let Some(tex) = doc.textures().iter().find(|t| t.id == slot.texture) else {
            continue;
        };
        if tex.width == 0 || tex.height == 0 {
            continue;
        }
        let repeat = (slot.scale != [1.0, 1.0]).then_some(slot.scale);
        out.push((
            row,
            TexSlotInfo {
                tex_name: tex.name.clone(),
                w: tex.width,
                h: tex.height,
                rgba: tex.rgba8.clone(),
                key: std::sync::Arc::as_ptr(&tex.rgba8) as *const u8 as u64,
                repeat,
            },
        ));
    }
    out
}

fn prop_row_ids() -> [&'static [LiveId]; PROP_ROWS] {
    [
        ids!(body.page_element.props_panel.p0),
        ids!(body.page_element.props_panel.p1),
        ids!(body.page_element.props_panel.p2),
        ids!(body.page_element.props_panel.p3),
        ids!(body.page_element.props_panel.p4),
        ids!(body.page_element.props_panel.p5),
        ids!(body.page_element.props_panel.p6),
        ids!(body.page_element.props_panel.p7),
        ids!(body.page_element.props_panel.p8),
        ids!(body.page_element.props_panel.p9),
        ids!(body.page_element.props_panel.p10),
        ids!(body.page_element.props_panel.p11),
        ids!(body.page_element.props_panel.p12),
        ids!(body.page_element.props_panel.p13),
        ids!(body.page_element.props_panel.p14),
        ids!(body.page_element.props_panel.p15),
    ]
}

/// The one-line display key for an attribute row. The section is already
/// called Attributes and every key arrives under a top-level "Properties"
/// group — repeating that group row by row says nothing, so it is dropped;
/// a deeper path ("Properties · Structure · Load") keeps its tail
/// ("Structure · Load"). Slug keys ("fire_rating", "FireRating") are spaced
/// and title-cased; keys that already read as words pass through.
fn attribute_label(group: &str, name: &str) -> String {
    let mut parts: Vec<&str> = group
        .split('·')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.first().is_some_and(|g| {
        g.eq_ignore_ascii_case("properties") || g.eq_ignore_ascii_case("attributes")
    }) {
        parts.remove(0);
    }
    let name = prettify_key(name);
    if parts.is_empty() {
        name
    } else {
        format!("{} · {}", parts.join(" · "), name)
    }
}

/// "fire_rating" / "FireRating" → "Fire Rating"; a key that already contains
/// spaces is left exactly as the document wrote it.
fn prettify_key(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains(' ') {
        return raw.to_string();
    }
    let mut words: Vec<String> = Vec::new();
    for chunk in raw.split(['_', '-']) {
        if chunk.is_empty() {
            continue;
        }
        // Split camelCase humps: a lower/digit followed by an upper starts
        // a new word.
        let mut word = String::new();
        let mut prev_lower = false;
        for c in chunk.chars() {
            if c.is_uppercase() && prev_lower {
                words.push(word.clone());
                word.clear();
            }
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
            word.push(c);
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    if words.is_empty() {
        return raw.to_string();
    }
    words
        .iter()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn value_text(v: &PropertyValue, units: &Units) -> String {
    match v {
        PropertyValue::Text(s) => s.clone(),
        PropertyValue::Number(n) => format!("{n:.3}"),
        PropertyValue::Integer(i) => i.to_string(),
        PropertyValue::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        PropertyValue::Length(m) => units.format_length(*m),
        PropertyValue::Area(a) => units.format_area(*a),
        PropertyValue::Volume(v) => units.format_volume(*v),
        PropertyValue::Angle(a) => format!("{a:.2}°"),
    }
}

impl FabProperties {
    fn set_row(&self, cx: &mut Cx, row: &[LiveId], value: &str) {
        self.view.widget(cx, row).label(cx, ids!(value)).set_text(cx, value);
    }

    fn shown_element(&self, state: &AppState) -> Option<ElementId> {
        self.pinned
            .filter(|id| state.scene.element(*id).is_some())
            .or(state.scene_state.selection.active)
    }

    fn num(&self, cx: &mut Cx, id: &[LiveId]) -> FabDragNumberRef {
        self.view.fab_drag_number(cx, id)
    }
}

impl Widget for FabProperties {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        // The enlarged-texture modal closes on Escape (no focusable content,
        // so the Modal's own key path never sees the key).
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape {
                let modal = self.view.modal(cx, ids!(tex_modal));
                if modal.is_open() {
                    modal.close(cx);
                }
            }
        }
        let Event::Actions(actions) = event else {
            return;
        };
        if shell_actions(actions).any(|a| {
            matches!(
                a,
                ShellAction::SetRenderSettings(_) | ShellAction::SetSun(_)
            )
        }) {
            self.synced_values = false;
        }

        // Tab strip.
        let set = self.view.radio_button_set(
            cx,
            ids_array!(
                body.tabs.tab_object,
                body.tabs.tab_element,
                body.tabs.tab_material,
                body.tabs.tab_quantities,
                body.tabs.tab_scene,
                body.tabs.tab_render,
            ),
        );
        if let Some(i) = set.selected(cx, actions) {
            self.tab = i.min(5);
            self.synced_tab = None;
            cx.action(ShellAction::SetPropertiesTab(
                PropertiesTab::ALL[self.tab.min(5)],
            ));
            self.view.redraw(cx);
        }

        // Collapsible panels.
        for panel in PANELS {
            fold_panel_clicked(&self.view, cx, actions, panel);
        }

        // The pin.
        if self
            .view
            .radio_button(cx, ids!(header.pin))
            .clicked(actions)
        {
            if self.pinned.is_some() {
                self.pinned = None;
            } else if let Some(state) = scope.data.get_mut::<AppState>() {
                self.pinned = state.scene_state.selection.active;
            }
            self.synced_values = false;
            self.view.redraw(cx);
        }

        // Object visibility.
        if let Some(on) = self
            .view
            .check_box(cx, ids!(body.page_object.vis_panel.cb_hidden))
            .changed(actions)
        {
            let id = scope
                .data
                .get_mut::<AppState>()
                .and_then(|s| self.shown_element(s));
            if let Some(id) = id {
                cx.action(ShellAction::SetHidden(id, on));
            }
        }
        if let Some(on) = self
            .view
            .check_box(cx, ids!(body.page_object.vis_panel.cb_selected))
            .changed(actions)
        {
            let id = scope
                .data
                .get_mut::<AppState>()
                .and_then(|s| self.shown_element(s));
            if let Some(id) = id {
                if on {
                    cx.action(ShellAction::SelectAdd(id));
                } else {
                    cx.action(ShellAction::SelectToggle(id));
                }
            }
        }

        // Explode.
        let explode_now = scope.data.get_mut::<AppState>().map(|s| s.scene_state.explode);
        if let Some(mut ex) = explode_now {
            let mut changed = false;
            if let Some(v) = self
                .num(cx, ids!(body.page_scene.explode_panel.num_explode))
                .changed(actions)
            {
                ex.amount = v as f32;
                changed = true;
            }
            if let Some(on) = self
                .view
                .check_box(cx, ids!(body.page_scene.explode_panel.cb_explode_story))
                .changed(actions)
            {
                ex.mode = if on {
                    ExplodeMode::ByStory
                } else {
                    ExplodeMode::ByElement
                };
                changed = true;
            }
            if changed {
                cx.action(ShellAction::SetExplode(ex));
            }
        }

        // Sun.
        let sun_now = scope.data.get_mut::<AppState>().map(|s| s.sun);
        if let Some(mut sun) = sun_now {
            let mut changed = false;
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_year)).changed(actions) {
                sun.date.year = v.round().clamp(2000.0, 2100.0) as i32;
                sun.date.day = sun
                    .date
                    .day
                    .min(days_in_month(sun.date.year, sun.date.month));
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_month)).changed(actions) {
                sun.date.month = v.round().clamp(1.0, 12.0) as u8;
                sun.date.day = sun
                    .date
                    .day
                    .min(days_in_month(sun.date.year, sun.date.month));
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_day)).changed(actions) {
                sun.date.day = (v.round().clamp(1.0, 31.0) as u8)
                    .min(days_in_month(sun.date.year, sun.date.month));
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_hour)).changed(actions) {
                sun.time_local = v as f32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_lat)).changed(actions) {
                sun.latitude = v as f32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_lon)).changed(actions) {
                sun.longitude = v as f32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_tz)).changed(actions) {
                sun.tz_offset = v as f32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_scene.sun_panel.num_north)).changed(actions) {
                sun.north_deg = v as f32;
                changed = true;
            }
            if let Some(v) = self
                .num(cx, ids!(body.page_scene.sun_panel.num_turbidity))
                .changed(actions)
            {
                sun.turbidity = v as f32;
                changed = true;
            }
            if let Some(v) = self
                .num(cx, ids!(body.page_scene.sun_panel.num_haze))
                .changed(actions)
            {
                sun.haze = v as f32;
                changed = true;
            }
            if let Some(v) = self
                .num(cx, ids!(body.page_render.film_panel.num_exposure))
                .changed(actions)
            {
                sun.exposure_ev = v as f32;
                changed = true;
            }
            if let Some(on) = self
                .view
                .check_box(cx, ids!(body.page_scene.sun_panel.cb_shadows))
                .changed(actions)
            {
                cx.action(ShellAction::SetSunShadows(on));
            }
            if changed {
                cx.action(ShellAction::SetSun(sun));
            }
        }

        // Material colour: the picker publishes live while its wheel or rows
        // are dragged, so the material follows the hand in both viewports.
        if let Some(mat) = self.shown_material {
            let picker = self
                .view
                .fab_color_picker(cx, ids!(body.page_material.mat_panel.swatch_row.picker));
            if let Some(c) = picker.changed(actions).or_else(|| picker.ended(actions)) {
                cx.action(ShellAction::SetMaterialBaseColor(mat, [c.x, c.y, c.z, c.w]));
            }
        }

        // Texture thumbnails: click opens the enlarged view.
        for (i, row_id) in TEX_ROW_IDS.iter().enumerate() {
            let thumb = self.view.widget(cx, row_id).fab_tex_thumb(cx, ids!(thumb));
            if !thumb.clicked(actions) {
                continue;
            }
            let Some(mat) = self.shown_material else { continue };
            let Some(state) = scope.data.get_mut::<AppState>() else {
                continue;
            };
            let slots = material_textures(state, mat);
            let mat_name = state
                .scene
                .material(mat)
                .map(|m| m.name.clone())
                .unwrap_or_default();
            if let Some((_, info)) = slots.into_iter().find(|(r, _)| *r == i) {
                self.view
                    .fab_tex_thumb(cx, ids!(tex_modal.big))
                    .set_image(cx, info.key, info.w, info.h, &info.rgba);
                let extra = info
                    .repeat
                    .map(|r| format!(" · {:.2} × {:.2} rpt/m", r[0], r[1]))
                    .unwrap_or_default();
                let cap = format!(
                    "{mat_name} — {} · {} × {} px{extra}",
                    info.tex_name, info.w, info.h
                );
                self.view.label(cx, ids!(tex_modal.caption)).set_text(cx, &cap);
                self.view.modal(cx, ids!(tex_modal)).open(cx);
            }
        }

        // Render.
        let render_now = scope.data.get_mut::<AppState>().map(|s| s.render);
        if let Some(mut r) = render_now {
            let mut changed = false;
            if let Some(v) = self.num(cx, ids!(body.page_render.sampling_panel.num_samples)).changed(actions) {
                r.max_samples = RenderSettings::clamp_max_samples(v.round() as u32);
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_render.sampling_panel.num_bounces)).changed(actions) {
                r.bounces = v.round().max(1.0) as u32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_render.sampling_panel.num_preview)).changed(actions) {
                r.preview_scale = v as f32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_render.film_panel.num_width)).changed(actions) {
                r.width = v.round().max(16.0) as u32;
                changed = true;
            }
            if let Some(v) = self.num(cx, ids!(body.page_render.film_panel.num_height)).changed(actions) {
                r.height = v.round().max(16.0) as u32;
                changed = true;
            }
            if let Some(on) = self
                .view
                .check_box(cx, ids!(body.page_render.film_panel.cb_denoise))
                .changed(actions)
            {
                r.denoise = on;
                changed = true;
            }
            if changed {
                cx.action(ShellAction::SetRenderSettings(r));
            }
        }

        // Depth of field lives on the active viewport's camera.
        let dof = scope
            .data
            .get_mut::<AppState>()
            .map(|s| (s.active_view, s.view().camera.f_stop, s.view().camera.focus_distance));
        if let Some((v, _f, _d)) = dof {
            if let Some(val) = self.num(cx, ids!(body.page_render.dof_panel.num_fstop)).changed(actions) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    state.view_at_mut(v).camera.f_stop = val as f32;
                    state.view_at_mut(v).render_dirty = true;
                }
            }
            if let Some(val) = self.num(cx, ids!(body.page_render.dof_panel.num_focus)).changed(actions) {
                if let Some(state) = scope.data.get_mut::<AppState>() {
                    state.view_at_mut(v).camera.focus_distance = val as f32;
                    state.view_at_mut(v).render_dirty = true;
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.synced_tab != Some(self.tab) {
            self.synced_tab = Some(self.tab);
            for (i, (tab, page)) in PAGES.iter().enumerate() {
                let on = i == self.tab;
                self.view
                    .radio_button(cx, tab)
                    .set_active(cx, on, Animate::No);
                self.view.widget(cx, page).set_visible(cx, on);
            }
            for panel in PANELS {
                set_panel_chevron(&self.view, cx, panel, true);
            }
        }

        if let Some(state) = scope.data.get_mut::<AppState>() {
            let units = state.scene.units;
            let shown = self.shown_element(state);
            self.view
                .radio_button(cx, ids!(header.pin))
                .set_active(cx, self.pinned.is_some(), Animate::No);

            match shown.and_then(|id| state.scene.element(id)) {
                Some(e) => {
                    let story = state
                        .scene
                        .story_of(e.id)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "—".into());
                    self.set_row(cx, ids!(body.page_object.object_panel.row_name), &e.name);
                    self.set_row(cx, ids!(body.page_object.object_panel.row_class), e.class.label());
                    self.set_row(cx, ids!(body.page_object.object_panel.row_story), &story);
                    let layer = e
                        .layer
                        .and_then(|l| state.scene.layers.get(l.index()))
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "—".into());
                    self.set_row(cx, ids!(body.page_object.object_panel.row_layer), &layer);
                    let guid = if e.guid.is_empty() { "—" } else { &e.guid };
                    self.set_row(cx, ids!(body.page_object.object_panel.row_guid), guid);
                    // The GUID rarely fits on one line; its tooltip carries
                    // the whole of it.
                    let mut guid_tip = self
                        .view
                        .widget(cx, ids!(body.page_object.object_panel.row_guid));
                    let guid_full = format!("GUID — {guid}");
                    script_apply_eval!(cx, guid_tip, { text: #(guid_full) });
                    let (size, centre) = if e.has_geometry() {
                        let ext = aabb_extent(&e.bounds);
                        let c = aabb_center(&e.bounds);
                        (
                            format!(
                                "{} × {} × {}",
                                units.format_length(ext.x as f64),
                                units.format_length(ext.y as f64),
                                units.format_length(ext.z as f64)
                            ),
                            format!(
                                "{}, {}, {}",
                                units.format_length(c.x as f64),
                                units.format_length(c.y as f64),
                                units.format_length(c.z as f64)
                            ),
                        )
                    } else {
                        ("—".into(), "—".into())
                    };
                    self.set_row(cx, ids!(body.page_object.dims_panel.row_size), &size);
                    self.set_row(cx, ids!(body.page_object.dims_panel.row_centre), &centre);
                    self.set_row(
                        cx,
                        ids!(body.page_object.dims_panel.row_tris),
                        &e.triangle_count.to_string(),
                    );
                    let pinned_mark = if self.pinned.is_some() { " 📌" } else { "" };
                    let _ = pinned_mark;
                    self.view
                        .label(cx, ids!(header.crumb))
                        .set_text(cx, &format!("{story} › {}", e.name));

                    let hidden = state.scene_state.hidden.contains(&e.id);
                    self.view
                        .check_box(cx, ids!(body.page_object.vis_panel.cb_hidden))
                        .set_active(cx, hidden, Animate::No);
                    self.view
                        .check_box(cx, ids!(body.page_object.vis_panel.cb_selected))
                        .set_active(cx, state.scene_state.selection.contains(e.id), Animate::No);

                    // ---- Element attributes
                    let rows = prop_row_ids();
                    for (i, id) in rows.iter().enumerate() {
                        let mut row = self.view.widget(cx, id);
                        match e.properties.get(i) {
                            Some(p) => {
                                row.set_visible(cx, true);
                                let name = attribute_label(&p.group, &p.name);
                                let value = value_text(&p.value, &units);
                                row.label(cx, ids!(name)).set_text(cx, &name);
                                row.label(cx, ids!(value)).set_text(cx, &value);
                                // The tooltip keeps the full raw key path and
                                // the untruncated value reachable when the
                                // one-line row elides them.
                                let full = if p.group.is_empty() {
                                    format!("{} — {}", p.name, value)
                                } else {
                                    format!("{} · {} — {}", p.group, p.name, value)
                                };
                                script_apply_eval!(cx, row, { text: #(full) });
                            }
                            None => row.set_visible(cx, false),
                        }
                    }
                    self.view
                        .widget(cx, ids!(body.page_element.props_panel.p_none))
                        .set_visible(cx, e.properties.is_empty());

                    // ---- Material actually used by this element
                    let mat_id = e
                        .ranges
                        .first()
                        .and_then(|(b, _, _)| state.scene.batches.get(*b as usize))
                        .map(|b| b.material);
                    let mat = mat_id.and_then(|m| state.scene.materials.get(m.index()));
                    self.shown_material = mat.map(|m| m.id);
                    match mat {
                        Some(m) => {
                            let c = m.base_color;
                            let picker = self
                                .view
                                .fab_color_picker(cx, ids!(body.page_material.mat_panel.swatch_row.picker));
                            // Never fight the user's own drag: the picker
                            // is the writer while its popover is up.
                            if !picker.is_open() {
                                picker.set_color(cx, c);
                            }
                            self.view
                                .label(cx, ids!(body.page_material.mat_panel.swatch_row.hex))
                                .set_text(
                                    cx,
                                    &format!(
                                        "#{:02X}{:02X}{:02X}  α {:.2}",
                                        (c[0].clamp(0.0, 1.0) * 255.0) as u32,
                                        (c[1].clamp(0.0, 1.0) * 255.0) as u32,
                                        (c[2].clamp(0.0, 1.0) * 255.0) as u32,
                                        c[3]
                                    ),
                                );
                            self.set_row(cx, ids!(body.page_material.mat_panel.row_mat_name), &m.name);
                            self.set_row(
                                cx,
                                ids!(body.page_material.mat_panel.row_metallic),
                                &format!("{:.2}", m.metallic),
                            );
                            self.set_row(
                                cx,
                                ids!(body.page_material.mat_panel.row_rough),
                                &format!("{:.2}", m.roughness),
                            );
                            self.set_row(
                                cx,
                                ids!(body.page_material.mat_panel.row_ior),
                                &format!("{:.2}", m.ior),
                            );
                            self.set_row(
                                cx,
                                ids!(body.page_material.mat_panel.row_trans),
                                &format!("{:.2}", m.transmission),
                            );
                            // ---- The material's texture slots, from the
                            // document (the only place all of them live).
                            let slots = material_textures(state, m.id);
                            self.set_row(
                                cx,
                                ids!(body.page_material.mat_panel.row_tex),
                                &if slots.is_empty() {
                                    "none".to_string()
                                } else {
                                    slots.len().to_string()
                                },
                            );
                            for (i, row_id) in TEX_ROW_IDS.iter().enumerate() {
                                let row = self.view.widget(cx, row_id);
                                match slots.iter().find(|(r, _)| *r == i) {
                                    Some((_, info)) => {
                                        row.set_visible(cx, true);
                                        row.fab_tex_thumb(cx, ids!(thumb)).set_image(
                                            cx, info.key, info.w, info.h, &info.rgba,
                                        );
                                        let extra = info
                                            .repeat
                                            .map(|r| {
                                                format!(" · {:.2} × {:.2} rpt/m", r[0], r[1])
                                            })
                                            .unwrap_or_default();
                                        row.label(cx, ids!(dims)).set_text(
                                            cx,
                                            &format!("{} × {} px{extra}", info.w, info.h),
                                        );
                                    }
                                    None => row.set_visible(cx, false),
                                }
                            }
                            self.view
                                .widget(cx, ids!(body.page_material.tex_panel.tex_none))
                                .set_visible(cx, slots.is_empty());
                        }
                        None => {
                            for r in [
                                ids!(body.page_material.mat_panel.row_mat_name),
                                ids!(body.page_material.mat_panel.row_metallic),
                                ids!(body.page_material.mat_panel.row_rough),
                                ids!(body.page_material.mat_panel.row_ior),
                                ids!(body.page_material.mat_panel.row_trans),
                                ids!(body.page_material.mat_panel.row_tex),
                            ] {
                                self.set_row(cx, r, "—");
                            }
                            for row_id in TEX_ROW_IDS.iter() {
                                self.view.widget(cx, row_id).set_visible(cx, false);
                            }
                            self.view
                                .widget(cx, ids!(body.page_material.tex_panel.tex_none))
                                .set_visible(cx, true);
                        }
                    }
                    self.qty = e
                        .quantities
                        .iter()
                        .map(|q| (q.name.clone(), value_text(&q.value, &units)))
                        .collect();
                    self.view
                        .label(cx, ids!(body.page_quantities.qty_head.qty_title))
                        .set_text(
                            cx,
                            &format!("{} — {} quantities", e.name, e.quantities.len()),
                        );
                }
                None => {
                    for r in [
                        ids!(body.page_object.object_panel.row_name),
                        ids!(body.page_object.object_panel.row_class),
                        ids!(body.page_object.object_panel.row_story),
                        ids!(body.page_object.object_panel.row_layer),
                        ids!(body.page_object.object_panel.row_guid),
                        ids!(body.page_object.dims_panel.row_size),
                        ids!(body.page_object.dims_panel.row_centre),
                        ids!(body.page_object.dims_panel.row_tris),
                    ] {
                        self.set_row(cx, r, "—");
                    }
                    self.view.label(cx, ids!(header.crumb)).set_text(cx, "Nothing selected");
                    self.shown_material = None;
                    for row_id in TEX_ROW_IDS.iter() {
                        self.view.widget(cx, row_id).set_visible(cx, false);
                    }
                    self.view
                        .widget(cx, ids!(body.page_material.tex_panel.tex_none))
                        .set_visible(cx, true);
                    for id in prop_row_ids() {
                        self.view.widget(cx, id).set_visible(cx, false);
                    }
                    self.view
                        .widget(cx, ids!(body.page_element.props_panel.p_none))
                        .set_visible(cx, true);
                    self.qty.clear();
                    self.view
                        .label(cx, ids!(body.page_quantities.qty_head.qty_title))
                        .set_text(cx, "Quantities — nothing selected");
                }
            }

            // ---- Scene
            let sc = &state.scene;
            self.set_row(
                cx,
                ids!(body.page_scene.scene_panel.row_scene),
                if sc.name.is_empty() { "—" } else { &sc.name },
            );
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_elements), &sc.stats.elements.to_string());
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_stories), &sc.stories.len().to_string());
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_layers), &sc.layers.len().to_string());
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_materials), &sc.materials.len().to_string());
            let bounds = if aabb_is_empty(&sc.bounds) {
                "—".to_string()
            } else {
                let ext = aabb_extent(&sc.bounds);
                format!(
                    "{} × {} × {}",
                    units.format_length(ext.x as f64),
                    units.format_length(ext.y as f64),
                    units.format_length(ext.z as f64)
                )
            };
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_bounds), &bounds);
            self.set_row(cx, ids!(body.page_scene.scene_panel.row_units), units.display.suffix());
            self.set_row(
                cx,
                ids!(body.page_scene.sun_panel.row_elev),
                &format!("{:.1}°", state.sun.elevation_deg()),
            );
            self.set_row(
                cx,
                ids!(body.page_render.sampling_panel.row_progress),
                &format!(
                    "{} / {} spp · {:.1} s",
                    state.render.samples_done, state.render.max_samples, state.render.elapsed_s
                ),
            );

            // Push the model values into the fields once (and whenever the
            // model changed underneath us) — never while the user drags.
            if self.synced_scene != Some(state.scene_revision) {
                self.synced_scene = Some(state.scene_revision);
                self.synced_values = false;
            }
            if !self.synced_values {
                self.synced_values = true;
                let sun = state.sun;
                let r = state.render;
                let ex = state.scene_state.explode;
                let cam = state.view().camera;
                self.num(cx, ids!(body.page_scene.sun_panel.num_year)).set_value(cx, sun.date.year as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_month)).set_value(cx, sun.date.month as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_day)).set_value(cx, sun.date.day as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_hour)).set_value(cx, sun.time_local as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_lat)).set_value(cx, sun.latitude as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_lon)).set_value(cx, sun.longitude as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_tz)).set_value(cx, sun.tz_offset as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_north)).set_value(cx, sun.north_deg as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_turbidity)).set_value(cx, sun.turbidity as f64);
                self.num(cx, ids!(body.page_scene.sun_panel.num_haze)).set_value(cx, sun.haze as f64);
                self.view
                    .check_box(cx, ids!(body.page_scene.sun_panel.cb_shadows))
                    .set_active(cx, state.sun_shadows, Animate::No);
                self.num(cx, ids!(body.page_scene.explode_panel.num_explode)).set_value(cx, ex.amount as f64);
                self.view
                    .check_box(cx, ids!(body.page_scene.explode_panel.cb_explode_story))
                    .set_active(cx, ex.mode == ExplodeMode::ByStory, Animate::No);
                self.num(cx, ids!(body.page_render.sampling_panel.num_samples)).set_value(cx, r.max_samples as f64);
                self.num(cx, ids!(body.page_render.sampling_panel.num_bounces)).set_value(cx, r.bounces as f64);
                self.num(cx, ids!(body.page_render.sampling_panel.num_preview)).set_value(cx, r.preview_scale as f64);
                self.num(cx, ids!(body.page_render.film_panel.num_exposure)).set_value(cx, sun.exposure_ev as f64);
                self.num(cx, ids!(body.page_render.film_panel.num_width)).set_value(cx, r.width as f64);
                self.num(cx, ids!(body.page_render.film_panel.num_height)).set_value(cx, r.height as f64);
                self.view
                    .check_box(cx, ids!(body.page_render.film_panel.cb_denoise))
                    .set_active(cx, r.denoise, Animate::No);
                self.num(cx, ids!(body.page_render.dof_panel.num_fstop)).set_value(cx, cam.f_stop as f64);
                self.num(cx, ids!(body.page_render.dof_panel.num_focus)).set_value(cx, cam.focus_distance as f64);
            }
        }

        let qty = self.qty.clone();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            grid.set_col_labels(vec!["Quantity".to_string(), "Value".to_string()]);
            grid.set_grid_size(qty.len(), 2);
            while let Some(cell) = grid.next_cell(cx) {
                let text = match qty.get(cell.row) {
                    Some((k, v)) => {
                        if cell.col == 0 {
                            k.clone()
                        } else {
                            v.clone()
                        }
                    }
                    None => String::new(),
                };
                grid.cell_text_styled(
                    cx,
                    &cell,
                    &text,
                    CellStyle {
                        align: if cell.col == 1 { 1.0 } else { 0.0 },
                        ..CellStyle::default()
                    },
                );
            }
        }
        self.sync_attr_label_split(cx);
        DrawStep::done()
    }
}

impl FabProperties {
    /// Split each attribute row between key and value for the panel's
    /// current width: the value column keeps a readable minimum and the
    /// label column gives way first, down to a floor where only its
    /// ellipsis remains (the tooltip still carries the whole key).
    fn sync_attr_label_split(&mut self, cx: &mut Cx2d) {
        const VALUE_MIN: f64 = 90.0;
        const LABEL_MAX: f64 = 92.0; // fab.prop_label_width
        const LABEL_MIN: f64 = 36.0;
        const ROW_CHROME: f64 = 30.0; // row padding + spacing + panel inset
        let avail = self.view.area().rect(cx).size.x;
        if avail <= 0.0 || (avail - self.attr_split_avail).abs() < 1.0 {
            return;
        }
        self.attr_split_avail = avail;
        let name_w = (avail - ROW_CHROME - VALUE_MIN).clamp(LABEL_MIN, LABEL_MAX);
        for id in prop_row_ids() {
            let mut name = self.view.widget(cx, id).widget(cx, ids!(name));
            script_apply_eval!(cx, name, { width: #(name_w) });
        }
        self.view.redraw(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redundant_properties_group_is_dropped() {
        assert_eq!(
            attribute_label("Properties", "Building Material"),
            "Building Material"
        );
        assert_eq!(attribute_label("Attributes", "ID"), "ID");
        assert_eq!(attribute_label("", "Type"), "Type");
    }

    #[test]
    fn a_deeper_path_keeps_its_tail() {
        assert_eq!(
            attribute_label("Properties · Structure", "Load"),
            "Structure · Load"
        );
        assert_eq!(
            attribute_label("Pset_WallCommon", "IsExternal"),
            "Pset_WallCommon · Is External"
        );
    }

    #[test]
    fn slug_keys_become_words_and_spaced_keys_pass_through() {
        assert_eq!(prettify_key("fire_rating"), "Fire Rating");
        assert_eq!(prettify_key("FireRating"), "Fire Rating");
        assert_eq!(prettify_key("thermal-transmittance"), "Thermal Transmittance");
        assert_eq!(prettify_key("Building Material"), "Building Material");
        assert_eq!(prettify_key("ID"), "ID");
    }
}
