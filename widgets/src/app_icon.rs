//! Application identities with artwork supplied by the active desktop style.
//! App code names an identity; renderers cache parsed SVGs and share source data.
use crate::{desktop_style::DesktopStyle, *};
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct IconAsset {
    pub name: String,
    pub svg: String,
}
struct BundledIcon {
    name: &'static str,
    variants: [&'static str; 7],
}
macro_rules! icon {
    ($name:literal) => {
        BundledIcon {
            name: $name,
            variants: [
                include_str!(concat!("../themes/omarchy/icons/", $name, ".svg")),
                include_str!(concat!("../themes/macos/icons/", $name, ".svg")),
                include_str!(concat!("../themes/windows/icons/", $name, ".svg")),
                include_str!(concat!("../themes/windows-2000/icons/", $name, ".svg")),
                include_str!(concat!("../themes/nextstep/icons/", $name, ".svg")),
                include_str!(concat!("../themes/ios/icons/", $name, ".svg")),
                include_str!(concat!("../themes/android/icons/", $name, ".svg")),
            ],
        }
    };
}
const ICONS: &[BundledIcon] = &[
    icon!("applications"),
    icon!("browser"),
    icon!("files"),
    icon!("terminal"),
    icon!("mixer"),
    icon!("task"),
    icon!("sheets"),
    icon!("photos"),
    icon!("clock"),
    icon!("weather"),
    icon!("fabric"),
    icon!("score"),
    icon!("video"),
    icon!("route"),
    icon!("vj"),
    icon!("fab"),
    icon!("studio"),
    icon!("image"),
    icon!("pdf"),
    icon!("aichat"),
    icon!("counter"),
    icon!("app"),
    icon!("file-folder"),
    icon!("file-generic"),
    icon!("file-image"),
    icon!("file-text"),
    icon!("file-code"),
    icon!("file-audio"),
    icon!("file-video"),
    icon!("file-archive"),
    icon!("file-pdf"),

];

pub fn canonical_name(name: &str) -> &str {
    let name = name.strip_prefix("makepad-").unwrap_or(name);
    name.strip_prefix("app-")
        .or_else(|| name.strip_prefix("example-"))
        .unwrap_or(name)
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Loads all artwork for the wire stylesheet, including locally added app IDs.
pub fn load_assets(style: DesktopStyle) -> Vec<IconAsset> {
    let mut assets: Vec<_> = ICONS
        .iter()
        .map(|icon| IconAsset {
            name: icon.name.into(),
            svg: icon.variants[style as usize].into(),
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(entries) = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("themes")
            .join(style.id())
            .join("icons"),
    ) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("svg") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !valid_name(name) {
                continue;
            }
            if let Ok(svg) = std::fs::read_to_string(&path) {
                if let Some(asset) = assets.iter_mut().find(|a| a.name == name) {
                    asset.svg = svg;
                } else {
                    assets.push(IconAsset {
                        name: name.into(),
                        svg,
                    });
                }
            }
        }
    }
    assets.sort_by(|a, b| a.name.cmp(&b.name));
    assets
}

#[derive(Default)]
struct IconCatalog {
    sources: HashMap<(usize, String), Arc<str>>,
}
pub fn install(cx: &mut Cx, style: DesktopStyle, assets: &[IconAsset]) {
    let catalog = cx.global::<IconCatalog>();
    // Preserve source identities when a style is reapplied unchanged. Parsed
    // geometry remains valid across appearance switches and isolate reloads.
    catalog
        .sources
        .retain(|(s, n), _| *s != style as usize || assets.iter().any(|a| a.name == *n));
    for asset in assets.iter().filter(|a| valid_name(&a.name)) {
        let key = (style as usize, asset.name.clone());
        if catalog
            .sources
            .get(&key)
            .is_none_or(|s| s.as_ref() != asset.svg)
        {
            catalog.sources.insert(key, Arc::from(asset.svg.as_str()));
        }
    }
}
pub fn source(cx: &mut Cx, style: DesktopStyle, name: &str) -> Arc<str> {
    let key = (style as usize, canonical_name(name).to_string());
    if let Some(source) = cx.global::<IconCatalog>().sources.get(&key) {
        return source.clone();
    }
    // Explicitly styled compositor draws can precede application registration.
    if !cx
        .global::<IconCatalog>()
        .sources
        .keys()
        .any(|(s, _)| *s == style as usize)
    {
        install(cx, style, &load_assets(style));
    }
    let catalog = cx.global::<IconCatalog>();
    catalog
        .sources
        .get(&key)
        .or_else(|| catalog.sources.get(&(style as usize, "app".into())))
        .cloned()
        .unwrap_or_else(|| Arc::from(ICONS.last().unwrap().variants[style as usize]))
}

struct CachedIcon {
    source: Arc<str>,
    draw: DrawSvg,
}
#[derive(Default)]
pub struct AppIconDraw {
    icons: HashMap<(usize, String), CachedIcon>,
}
impl AppIconDraw {
    /// Shared by widgets, window chrome, launchers and the compositor's shelves.
    pub fn draw(
        &mut self,
        cx: &mut Cx2d,
        name: &str,
        style: DesktopStyle,
        rect: Rect,
        opacity: f32,
        ink: Vec4f,
    ) {
        let name = canonical_name(name);
        let source = source(cx, style, name);
        let key = (style as usize, name.into());
        let cached = self.icons.entry(key).or_insert_with(|| CachedIcon {
            source: Arc::from(""),
            draw: cx.with_vm(|vm| DrawSvg::script_new_with_default(vm)),
        });
        if !Arc::ptr_eq(&cached.source, &source) {
            cached.draw.load_from_str(&source);
            // Icon canvases include intentional optical padding. Keep that
            // canvas instead of auto-cropping every identity to its ink bounds;
            // classic 16px grids then also stay on whole pixels at native size.
            if let Some(doc) = &cached.draw.svg_doc {
                let (width, height) = doc.logical_size();
                cached.draw.content_bounds = (0.0, 0.0, width, height);
            }
            cached.source = source;
        }
        cached.draw.color = if style == DesktopStyle::Omarchy {
            ink
        } else {
            vec4(-1.0, -1.0, -1.0, -1.0)
        };
        cached.draw.opacity = opacity;
        cached.draw.draw_abs(cx, rect);
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.AppIcon = set_type_default() do #(AppIcon::register_widget(vm)) {
        width: 24 height: 24
        name: "app"
        style: "auto"
        color: theme.color_text
        opacity: 1.0
    }
}
#[derive(Script, Widget)]
pub struct AppIcon {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[live]
    name: ArcStringMut,
    #[live]
    style: ArcStringMut,
    #[live]
    color: Vec4f,
    #[live(1.0)]
    opacity: f32,
    #[rust]
    resolved: DesktopStyle,
    #[rust]
    renderer: AppIconDraw,
    #[redraw]
    #[rust]
    area: Area,
}
impl ScriptHook for AppIcon {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let selected = desktop_style::current_name(vm);
        let name = if self.style.as_ref() == "auto" {
            selected.as_deref().unwrap_or(if cfg!(target_os = "ios") {
                "ios"
            } else if cfg!(target_os = "android") {
                "android"
            } else if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(target_os = "windows") {
                "windows"
            } else {
                "omarchy"
            })
        } else {
            self.style.as_ref()
        };
        self.resolved = DesktopStyle::parse(name).unwrap_or(if name == "macos-dark" {
            DesktopStyle::Macos
        } else {
            DesktopStyle::Omarchy
        });
    }
}
impl AppIcon {
    pub fn set_name(&mut self, cx: &mut Cx, name: &str) {
        if self.name.as_ref() != name {
            self.name.set(name);
            self.area.redraw(cx);
        }
    }
}
impl Widget for AppIcon {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.renderer.draw(
            cx,
            self.name.as_ref(),
            self.resolved,
            rect,
            self.opacity,
            self.color,
        );
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_identity_has_distinct_artwork_for_every_desktop() {
        for icon in ICONS {
            for (i, svg) in icon.variants.iter().enumerate() {
                assert!(svg.contains("<svg"), "{} {i}", icon.name);
                // Document glyphs may be shared by closely related families.
                for other in &icon.variants[..if icon.name.starts_with("file-") {0}else{i}] {
                    assert_ne!(svg, other, "{} {i}", icon.name);
                }
            }
        }
    }
    #[test]
    fn reapply_reuses_sources_edits_invalidate_and_missing_icons_fall_back() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut assets = load_assets(DesktopStyle::Macos);
        install(&mut cx, DesktopStyle::Macos, &assets);
        let original = source(&mut cx, DesktopStyle::Macos, "terminal");
        install(&mut cx, DesktopStyle::Macos, &assets);
        assert!(Arc::ptr_eq(
            &original,
            &source(&mut cx, DesktopStyle::Macos, "terminal")
        ));
        assets
            .iter_mut()
            .find(|a| a.name == "terminal")
            .unwrap()
            .svg
            .push_str("\n");
        install(&mut cx, DesktopStyle::Macos, &assets);
        assert!(!Arc::ptr_eq(
            &original,
            &source(&mut cx, DesktopStyle::Macos, "terminal")
        ));
        assert!(Arc::ptr_eq(
            &source(&mut cx, DesktopStyle::Macos, "unknown"),
            &source(&mut cx, DesktopStyle::Macos, "app")
        ));
        assert_eq!(canonical_name("makepad-app-score"), "score");
        assert!(!valid_name("../secret"));
    }
}
