//! Arranging the screens the direct renderer drives into one wide desktop.
//!
//! Screens sit left to right along their top edges at their native
//! resolutions. The renderer draws the whole desktop once into one
//! composition image; each screen shows its own rectangle of it, unscaled.
//! A saved order of connector names overrides the default order; screens it
//! does not name follow in default order. A screen that would make the
//! composition larger than the GPU can render is left out of the desktop.
//!
//! Pure geometry, so it is unit tested without a GPU: no Vulkan, no DRM.

/// A screen offered to the layout: its connector name and native mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenMode {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl ScreenMode {
    pub fn new(name: &str, width: u32, height: u32) -> Self {
        Self { name: name.to_string(), width, height }
    }
}

/// A rectangle of the wide desktop in native pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SliceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Where one screen sits; `None` when it is not part of the desktop (no
/// mode yet, or the desktop would exceed the GPU's limits).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedScreen {
    pub name: String,
    pub rect: Option<SliceRect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WideLayout {
    /// Every offered screen, in desktop order (placed ones left to right).
    pub screens: Vec<PlacedScreen>,
    /// The desktop size: the composition the renderer draws.
    pub width: u32,
    pub height: u32,
}

impl WideLayout {
    pub fn rect_of(&self, name: &str) -> Option<SliceRect> {
        self.screens.iter().find(|screen| screen.name == name).and_then(|screen| screen.rect)
    }
}

/// Lays `screens` (given in default order) out left to right. Names in
/// `preferred` come first, in that order; unknown names are ignored. A
/// screen is left out when adding it would exceed `max_width` or
/// `max_height`.
pub fn arrange(screens: &[ScreenMode], preferred: &[String], max_width: u32, max_height: u32) -> WideLayout {
    let mut ordered: Vec<&ScreenMode> = Vec::with_capacity(screens.len());
    for name in preferred {
        if let Some(screen) = screens.iter().find(|s| &s.name == name) {
            if !ordered.iter().any(|s| s.name == screen.name) {
                ordered.push(screen);
            }
        }
    }
    for screen in screens {
        if !ordered.iter().any(|s| s.name == screen.name) {
            ordered.push(screen);
        }
    }

    let mut layout = WideLayout::default();
    for screen in ordered {
        let fits = screen.width > 0
            && screen.height > 0
            && layout.width.checked_add(screen.width).is_some_and(|right| right <= max_width)
            && screen.height <= max_height;
        let rect = fits.then(|| SliceRect { x: layout.width, y: 0, width: screen.width, height: screen.height });
        if fits {
            layout.width += screen.width;
            layout.height = layout.height.max(screen.height);
        }
        layout.screens.push(PlacedScreen { name: screen.name.clone(), rect });
    }
    layout
}

/// How one screen shows its slice: draw the whole composition shifted so the
/// slice's origin lands on the screen's top-left corner, unscaled, and clip
/// to the slice. Returns the viewport `[x, y, width, height]` and the scissor
/// size (its offset is always 0,0).
pub fn slice_viewport(rect: SliceRect, composition: (u32, u32), output: (u32, u32)) -> ([f32; 4], (u32, u32)) {
    (
        [-(rect.x as f32), -(rect.y as f32), composition.0 as f32, composition.1 as f32],
        (rect.width.min(output.0), rect.height.min(output.1)),
    )
}

/// The smallest rectangle holding all `rects`; `None` when there are none.
/// A card's slice of the wide desktop is the bounding box of its screens.
pub fn bounding(rects: impl IntoIterator<Item = SliceRect>) -> Option<SliceRect> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for rect in rects {
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
        bounds = Some(match bounds {
            None => (rect.x, rect.y, right, bottom),
            Some((x, y, r, b)) => (x.min(rect.x), y.min(rect.y), r.max(right), b.max(bottom)),
        });
    }
    bounds.map(|(x, y, right, bottom)| SliceRect { x, y, width: right - x, height: bottom - y })
}

/// Per-connector display modes from text like
/// `card0-HDMI-A-2=3840x2160@30, card1-DP-1=1920x1080`. Entries without `=`
/// or with an empty side are ignored; surrounding spaces are trimmed. The mode
/// part uses the same forms as `MAKEPAD_DRM_MODE`.
pub fn parse_mode_overrides(text: &str) -> Vec<(String, String)> {
    text.split([',', ';'])
        .filter_map(|entry| {
            let (connector, mode) = entry.split_once('=')?;
            let (connector, mode) = (connector.trim(), mode.trim());
            (!connector.is_empty() && !mode.is_empty()).then(|| (connector.to_string(), mode.to_string()))
        })
        .collect()
}

/// A left-to-right order of connector names from text like
/// `card0-HDMI-A-2, card1-HDMI-A-1` (commas or semicolons; blanks ignored).
pub fn parse_display_order(text: &str) -> Vec<String> {
    text.split([',', ';']).map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: u32 = 16384;

    fn names(layout: &WideLayout) -> Vec<&str> {
        layout.screens.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn one_screen_is_the_whole_desktop() {
        let layout = arrange(&[ScreenMode::new("card1-HDMI-A-1", 3840, 2160)], &[], BIG, BIG);
        assert_eq!((layout.width, layout.height), (3840, 2160));
        assert_eq!(
            layout.rect_of("card1-HDMI-A-1"),
            Some(SliceRect { x: 0, y: 0, width: 3840, height: 2160 })
        );
    }

    #[test]
    fn screens_line_up_left_to_right_along_the_top() {
        let layout = arrange(
            &[ScreenMode::new("a", 3840, 2160), ScreenMode::new("b", 1920, 1080)],
            &[],
            BIG,
            BIG,
        );
        assert_eq!((layout.width, layout.height), (5760, 2160));
        assert_eq!(layout.rect_of("a"), Some(SliceRect { x: 0, y: 0, width: 3840, height: 2160 }));
        assert_eq!(layout.rect_of("b"), Some(SliceRect { x: 3840, y: 0, width: 1920, height: 1080 }));
    }

    #[test]
    fn a_saved_order_comes_first_and_the_rest_follow() {
        let screens = [
            ScreenMode::new("a", 100, 50),
            ScreenMode::new("b", 100, 50),
            ScreenMode::new("c", 100, 50),
        ];
        let preferred = vec!["c".to_string(), "gone".to_string(), "a".to_string(), "c".to_string()];
        let layout = arrange(&screens, &preferred, BIG, BIG);
        assert_eq!(names(&layout), vec!["c", "a", "b"]);
        assert_eq!(layout.rect_of("c").unwrap().x, 0);
        assert_eq!(layout.rect_of("a").unwrap().x, 100);
        assert_eq!(layout.rect_of("b").unwrap().x, 200);
    }

    #[test]
    fn removing_a_screen_keeps_those_to_its_left_in_place() {
        let all = [ScreenMode::new("a", 100, 50), ScreenMode::new("b", 200, 50), ScreenMode::new("c", 300, 50)];
        let before = arrange(&all, &[], BIG, BIG);
        let after = arrange(&[all[0].clone(), all[1].clone()], &[], BIG, BIG);
        assert_eq!(before.rect_of("a"), after.rect_of("a"));
        assert_eq!(before.rect_of("b"), after.rect_of("b"));
        assert_eq!((after.width, before.width), (300, 600));
    }

    #[test]
    fn a_screen_beyond_the_gpu_limit_is_left_out() {
        let layout = arrange(
            &[ScreenMode::new("a", 4000, 100), ScreenMode::new("b", 4000, 100), ScreenMode::new("c", 100, 100)],
            &[],
            8192,
            8192,
        );
        assert_eq!(layout.rect_of("b"), Some(SliceRect { x: 4000, y: 0, width: 4000, height: 100 }));
        assert_eq!(layout.rect_of("c"), Some(SliceRect { x: 8000, y: 0, width: 100, height: 100 }));
        let tight = arrange(&[ScreenMode::new("a", 4000, 100), ScreenMode::new("b", 4500, 100)], &[], 8192, 8192);
        assert_eq!(tight.rect_of("b"), None);
        assert_eq!((tight.width, tight.height), (4000, 100));
        let tall = arrange(&[ScreenMode::new("a", 100, 9000)], &[], 8192, 8192);
        assert_eq!(tall.rect_of("a"), None);
        assert_eq!((tall.width, tall.height), (0, 0));
    }

    #[test]
    fn a_screen_without_a_mode_is_left_out() {
        let layout = arrange(&[ScreenMode::new("a", 0, 0), ScreenMode::new("b", 100, 50)], &[], BIG, BIG);
        assert_eq!(layout.rect_of("a"), None);
        assert_eq!(layout.rect_of("b"), Some(SliceRect { x: 0, y: 0, width: 100, height: 50 }));
    }

    #[test]
    fn a_slice_draws_the_composition_shifted_and_clipped() {
        let rect = SliceRect { x: 3840, y: 0, width: 1920, height: 1080 };
        let (viewport, scissor) = slice_viewport(rect, (5760, 2160), (1920, 1080));
        assert_eq!(viewport, [-3840.0, 0.0, 5760.0, 2160.0]);
        assert_eq!(scissor, (1920, 1080));
        let (viewport, scissor) = slice_viewport(SliceRect { x: 0, y: 0, width: 3840, height: 2160 }, (3840, 2160), (3840, 2160));
        assert_eq!(viewport, [0.0, 0.0, 3840.0, 2160.0]);
        assert_eq!(scissor, (3840, 2160));
    }

    #[test]
    fn a_card_slice_is_the_bounding_box_of_its_screens() {
        assert_eq!(bounding([]), None);
        let a = SliceRect { x: 3840, y: 0, width: 1920, height: 1080 };
        let b = SliceRect { x: 5760, y: 0, width: 3840, height: 2160 };
        assert_eq!(bounding([a]), Some(a));
        assert_eq!(bounding([b, a]), Some(SliceRect { x: 3840, y: 0, width: 5760, height: 2160 }));
        let low = SliceRect { x: 0, y: 200, width: 100, height: 50 };
        let high = SliceRect { x: 150, y: 10, width: 100, height: 100 };
        assert_eq!(bounding([low, high]), Some(SliceRect { x: 0, y: 10, width: 250, height: 240 }));
    }

    #[test]
    fn mode_overrides_are_read_per_connector() {
        assert_eq!(
            parse_mode_overrides(" card0-HDMI-A-2=3840x2160@30 ; card1-DP-1 = 1920x1080,broken,=x,y="),
            vec![
                ("card0-HDMI-A-2".to_string(), "3840x2160@30".to_string()),
                ("card1-DP-1".to_string(), "1920x1080".to_string()),
            ]
        );
        assert_eq!(parse_mode_overrides(""), vec![]);
    }

    #[test]
    fn display_order_is_read_from_text() {
        assert_eq!(
            parse_display_order(" card0-HDMI-A-2 ,card1-HDMI-A-1;; "),
            vec!["card0-HDMI-A-2".to_string(), "card1-HDMI-A-1".to_string()]
        );
        assert!(parse_display_order("").is_empty());
    }
}
