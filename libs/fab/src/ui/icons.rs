//! Lane D. Icons are **our own SVG files** in `libs/fab/resources/icons/`
//! rendered through the stock `Icon` widget (`DrawSvg`, tessellated onto one
//! quad, tinted by `draw_icon.color`). Never a bitmap, never Fab's sheet.
//!
//! Style rules for the set:
//! * `viewBox 0 0 16 16`, drawn on the 16 px grid, rendered at 16 px (rows,
//!   tabs) or 20 px (toolbar); optical center, 1 px inset all round.
//! * strokes `1.25` (line glyphs) / `1.5` (emphasis), round caps + joins,
//!   `fill="#000"` for solid glyphs — the SVG color is irrelevant, the widget
//!   tints with `fab.color_text` / `color_text_active` / `color_accent`.
//! * corner radius `1` on rects, no gradients, no text, ≤ 3 sub-shapes.
//!
//! `DrawSvg` derefs to `DrawVector`: never place an icon under a parent that
//! aligns to the far edge (`align: Align{x: 1.0}` / `{y: 1.0}`) — the deferred
//! turtle shift displaces vector geometry. Use `Filler{}` to push instead.

use makepad_widgets::*;

/// The icon set. `path()` is the `crate_resource("self://…")` argument; the
/// DSL references the same files literally. `ALL` exists so a test can assert
/// that every name in here has a file on disk.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    // editors
    EditorViewport,
    EditorOutliner,
    EditorProperties,
    EditorSheets,
    EditorInfo,
    EditorRender,
    EditorTours,
    // element classes
    ElWall,
    ElSlab,
    ElRoof,
    ElDoor,
    ElWindow,
    ElColumn,
    ElBeam,
    ElStair,
    ElRailing,
    ElFurniture,
    ElSite,
    ElMesh,
    ElGroup,
    // shading
    Wireframe,
    Solid,
    Material,
    Realtime,
    Rendered,
    HiddenLine,
    Xray,
    Overlays,
    // tools
    Select,
    BoxSelect,
    Measure,
    MeasureArea,
    MeasureAngle,
    Section,
    SectionBox,
    Explode,
    Isolate,
    SnapMagnet,
    Ruler,
    Walk,
    WalkTool,
    // toggles / marks
    Eye,
    EyeOff,
    Check,
    Dot,
    Dots,
    RadioOn,
    RadioOff,
    Pin,
    Filter,
    Search,
    Grid,
    Gear,
    Object,
    Sun,
    Camera,
    Layers,
    Story,
    Lock,
    Unlock,
    Orbit,
    Pan,
    Zoom,
    Ortho,
    Persp,
    Play,
    Pause,
    Plus,
    Minus,
    Close,
    TriangleRight,
    TriangleDown,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    ChevronUp,
    // files / navigation
    Folder,
    FolderOpen,
    File,
    Home,
    Star,
    Refresh,
    Clock,
    // window / layout
    Maximize,
    Restore,
    SplitH,
    SplitV,
    CornerGrip,
    // misc
    Help,
    Keyboard,
    Palette,
    Image,
    Link,
    Lightbulb,
    Stats,
    Warning,
    DragX,
    ShadingStudio,
    Fab,
}

impl Icon {
    pub const ALL: &'static [Icon] = &[
        Icon::EditorViewport,
        Icon::EditorOutliner,
        Icon::EditorProperties,
        Icon::EditorSheets,
        Icon::EditorInfo,
        Icon::EditorRender,
        Icon::EditorTours,
        Icon::ElWall,
        Icon::ElSlab,
        Icon::ElRoof,
        Icon::ElDoor,
        Icon::ElWindow,
        Icon::ElColumn,
        Icon::ElBeam,
        Icon::ElStair,
        Icon::ElRailing,
        Icon::ElFurniture,
        Icon::ElSite,
        Icon::ElMesh,
        Icon::ElGroup,
        Icon::Wireframe,
        Icon::Solid,
        Icon::Material,
        Icon::Realtime,
        Icon::Rendered,
        Icon::HiddenLine,
        Icon::Xray,
        Icon::Overlays,
        Icon::Select,
        Icon::BoxSelect,
        Icon::Measure,
        Icon::MeasureArea,
        Icon::MeasureAngle,
        Icon::Section,
        Icon::SectionBox,
        Icon::Explode,
        Icon::Isolate,
        Icon::SnapMagnet,
        Icon::Ruler,
        Icon::Walk,
        Icon::WalkTool,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Check,
        Icon::Dot,
        Icon::Dots,
        Icon::RadioOn,
        Icon::RadioOff,
        Icon::Pin,
        Icon::Filter,
        Icon::Search,
        Icon::Grid,
        Icon::Gear,
        Icon::Object,
        Icon::Sun,
        Icon::Camera,
        Icon::Layers,
        Icon::Story,
        Icon::Lock,
        Icon::Unlock,
        Icon::Orbit,
        Icon::Pan,
        Icon::Zoom,
        Icon::Ortho,
        Icon::Persp,
        Icon::Play,
        Icon::Pause,
        Icon::Plus,
        Icon::Minus,
        Icon::Close,
        Icon::TriangleRight,
        Icon::TriangleDown,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronUp,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::File,
        Icon::Home,
        Icon::Star,
        Icon::Refresh,
        Icon::Clock,
        Icon::Maximize,
        Icon::Restore,
        Icon::SplitH,
        Icon::SplitV,
        Icon::CornerGrip,
        Icon::Help,
        Icon::Keyboard,
        Icon::Palette,
        Icon::Image,
        Icon::Link,
        Icon::Lightbulb,
        Icon::Stats,
        Icon::Warning,
        Icon::DragX,
        Icon::ShadingStudio,
        Icon::Fab,
    ];

    pub fn file(self) -> &'static str {
        match self {
            Icon::EditorViewport => "editor_viewport",
            Icon::EditorOutliner => "editor_outliner",
            Icon::EditorProperties => "editor_properties",
            Icon::EditorSheets => "editor_sheets",
            Icon::EditorInfo => "editor_info",
            Icon::EditorRender => "editor_render",
            Icon::EditorTours => "editor_tours",
            Icon::ElWall => "el_wall",
            Icon::ElSlab => "el_slab",
            Icon::ElRoof => "el_roof",
            Icon::ElDoor => "el_door",
            Icon::ElWindow => "el_window",
            Icon::ElColumn => "el_column",
            Icon::ElBeam => "el_beam",
            Icon::ElStair => "el_stair",
            Icon::ElRailing => "el_railing",
            Icon::ElFurniture => "el_furniture",
            Icon::ElSite => "el_site",
            Icon::ElMesh => "el_mesh",
            Icon::ElGroup => "el_group",
            Icon::Wireframe => "wireframe",
            Icon::Solid => "solid",
            Icon::Material => "material",
            Icon::Realtime => "realtime",
            Icon::Rendered => "rendered",
            Icon::HiddenLine => "hidden_line",
            Icon::Xray => "xray",
            Icon::Overlays => "overlays",
            Icon::Select => "select",
            Icon::BoxSelect => "box_select",
            Icon::Measure => "measure",
            Icon::MeasureArea => "measure_area",
            Icon::MeasureAngle => "measure_angle",
            Icon::Section => "section",
            Icon::SectionBox => "section_box",
            Icon::Explode => "explode",
            Icon::Isolate => "isolate",
            Icon::SnapMagnet => "snap_magnet",
            Icon::Ruler => "ruler",
            Icon::Walk => "walk",
            Icon::WalkTool => "walk_tool",
            Icon::Eye => "eye",
            Icon::EyeOff => "eye_off",
            Icon::Check => "check",
            Icon::Dot => "dot",
            Icon::Dots => "dots",
            Icon::RadioOn => "radio_on",
            Icon::RadioOff => "radio_off",
            Icon::Pin => "pin",
            Icon::Filter => "filter",
            Icon::Search => "search",
            Icon::Grid => "grid",
            Icon::Gear => "gear",
            Icon::Object => "object",
            Icon::Sun => "sun",
            Icon::Camera => "camera",
            Icon::Layers => "layers",
            Icon::Story => "story",
            Icon::Lock => "lock",
            Icon::Unlock => "unlock",
            Icon::Orbit => "orbit",
            Icon::Pan => "pan",
            Icon::Zoom => "zoom",
            Icon::Ortho => "ortho",
            Icon::Persp => "persp",
            Icon::Play => "play",
            Icon::Pause => "pause",
            Icon::Plus => "plus",
            Icon::Minus => "minus",
            Icon::Close => "close",
            Icon::TriangleRight => "triangle_right",
            Icon::TriangleDown => "triangle_down",
            Icon::ChevronRight => "chevron_right",
            Icon::ChevronDown => "chevron_down",
            Icon::ChevronLeft => "chevron_left",
            Icon::ChevronUp => "chevron_up",
            Icon::Folder => "folder",
            Icon::FolderOpen => "folder_open",
            Icon::File => "file",
            Icon::Home => "home",
            Icon::Star => "star",
            Icon::Refresh => "refresh",
            Icon::Clock => "clock",
            Icon::Maximize => "maximize",
            Icon::Restore => "restore",
            Icon::SplitH => "split_h",
            Icon::SplitV => "split_v",
            Icon::CornerGrip => "corner_grip",
            Icon::Help => "help",
            Icon::Keyboard => "keyboard",
            Icon::Palette => "palette",
            Icon::Image => "image",
            Icon::Link => "link",
            Icon::Lightbulb => "lightbulb",
            Icon::Stats => "stats",
            Icon::Warning => "warning",
            Icon::DragX => "drag_x",
            Icon::ShadingStudio => "shading_studio",
            Icon::Fab => "fab",
        }
    }

    pub fn path(self) -> String {
        format!("self://resources/icons/{}.svg", self.file())
    }
}

/// The element-class icon the outliner and properties use.
pub fn element_icon(class: &crate::api::ElementClass) -> Icon {
    use crate::api::ElementClass::*;
    match class {
        Wall | CurtainWall => Icon::ElWall,
        Slab => Icon::ElSlab,
        Roof | Shell => Icon::ElRoof,
        Door | Opening => Icon::ElDoor,
        Window | Skylight => Icon::ElWindow,
        Column => Icon::ElColumn,
        Beam => Icon::ElBeam,
        Stair => Icon::ElStair,
        Railing => Icon::ElRailing,
        Furniture | Object | Lamp => Icon::ElFurniture,
        Site | Zone => Icon::ElSite,
        Group => Icon::ElGroup,
        _ => Icon::ElMesh,
    }
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    // DrawVector (DrawSvg) must not sit under an aligning parent — deferred
    // turtle alignment is why chevrons used to ride high. The icon fills its
    // own box; the *parent row* centres that box with padding, never align.
    mod.widgets.FabIcon = Icon{
        width: fab.icon_size
        height: fab.icon_size
        align: Align{x: 0.0 y: 0.0}
        icon_walk: Walk{
            width: fab.icon_size
            height: fab.icon_size
        }
        draw_icon +: {
            color: fab.color_text
        }
    }

    mod.widgets.FabIconActive = mod.widgets.FabIcon{
        draw_icon +: {
            color: fab.color_text_active
        }
    }

    mod.widgets.FabIconDim = mod.widgets.FabIcon{
        draw_icon +: {
            color: fab.color_text_dim
        }
    }

    mod.widgets.FabIconMuted = mod.widgets.FabIcon{
        draw_icon +: {
            color: fab.color_text_muted
        }
    }

    mod.widgets.FabIconAccent = mod.widgets.FabIcon{
        draw_icon +: {
            color: fab.color_accent_hover
        }
    }

    mod.widgets.FabIconSmall = mod.widgets.FabIcon{
        width: fab.icon_size_sm
        height: fab.icon_size_sm
        icon_walk: Walk{
            width: fab.icon_size_sm
            height: fab.icon_size_sm
        }
        draw_icon +: {
            color: fab.color_text_dim
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `Icon` variant must have a file on disk — a missing SVG renders
    /// as nothing at all, which is invisible in a code review.
    #[test]
    fn every_icon_has_a_file() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut missing = Vec::new();
        for icon in Icon::ALL {
            let p = root.join("resources/icons").join(format!("{}.svg", icon.file()));
            if !p.exists() {
                missing.push(icon.file());
            }
        }
        assert!(missing.is_empty(), "missing icon files: {missing:?}");
    }

    #[test]
    fn icon_names_are_unique() {
        let mut names: Vec<&str> = Icon::ALL.iter().map(|i| i.file()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate icon file names");
    }
}
