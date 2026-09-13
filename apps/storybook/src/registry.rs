//! The story registry.
//!
//! A story is a record: where it sits in the navigator (category and
//! component), the name of its DSL template under `mod.stories`, the date it
//! was added, a note in Markdown, and the controls the controls panel may
//! drive on its subject. The records are plain constants in the story files;
//! `all()` walks them in the order the files register, which is the order the
//! navigator shows.
use crate::makepad_widgets::*;

/// The date the catalogue was started: how far back "new" reaches until
/// the person sets the reach in days themselves. What the settings keep is
/// that number of days, and the date a story is measured against is today
/// less that reach, so it moves with the calendar rather than staying put.
pub const DEFAULT_BASELINE: &str = "2026-09-05";

pub struct Story {
    /// `category/component/name`, lowercase, unique. The search key, the
    /// settings key, the test key and the remote-op key.
    pub key: &'static str,
    /// The navigator's top-level folder.
    pub category: &'static str,
    /// The navigator's second-level folder, normally the widget's DSL name.
    pub component: &'static str,
    /// Other declarations this page shows besides its own `component`.
    ///
    /// A page routinely renders more than the one widget it is filed under:
    /// the segmented control lives on the button-group page and the chip
    /// group on the chip page. Only the coverage count reads this, and
    /// without it those widgets report as undocumented while standing on
    /// their own catalogue page. It has to be written down rather than
    /// found: a story template's children do not exist until the page is
    /// built, so there is nothing to walk until something builds it.
    pub also: &'static [&'static str],
    /// The row in the navigator.
    pub name: &'static str,
    /// The template's name under `mod.stories`.
    pub dsl: &'static str,
    /// ISO date by which the WIDGET this story documents was in the
    /// library — not the date the story was written. A page written today
    /// about a widget that shipped last year carries the widget's date, so
    /// the NEW marker answers "what was added", never "what was documented".
    /// The pages under Overview are about the catalogue, not a widget, and
    /// carry a date before any baseline.
    pub added: &'static str,
    /// Free-form tags the search also matches ("ported", "layout", ...).
    pub tags: &'static [&'static str],
    /// The note shown in the docs panel, in Markdown.
    pub doc: &'static str,
    /// Id of the widget inside the story the docs and controls address;
    /// empty means the story's root.
    pub subject: &'static str,
    /// The cargo feature the story's widget family needs, if any. Such a
    /// story is listed greyed with the feature name when the feature is off.
    pub feature: Option<&'static str>,
    /// Live controls for the subject.
    pub controls: &'static [Control],
    /// A story's own action handler, run every actions pass with the story
    /// root, for demos that react to their own buttons.
    pub on_actions: Option<fn(&mut Cx, &WidgetRef, &Actions)>,
}

pub struct Control {
    pub label: &'static str,
    /// Id path of the widget the control writes to, relative to the story
    /// root; empty means the subject.
    pub target: &'static str,
    pub kind: ControlKind,
}

pub enum ControlKind {
    Bool { prop: &'static str, default: bool },
    Number { prop: &'static str, min: f64, max: f64, step: f64, default: f64 },
    Choice { prop: &'static str, options: &'static [&'static str], default: usize },
    Text { prop: &'static str, default: &'static str },
    Color { prop: &'static str, default: u32 },
    Disabled { default: bool },
}

/// Every story, in navigator order.
pub fn all() -> impl Iterator<Item = &'static Story> {
    crate::stories::tables().iter().flat_map(|table| table.iter())
}

/// The story under this key. A key that no longer names a page finds the
/// page that holds its content now, so a saved last story, a remote op or a
/// note written before the catalogue was regrouped still opens something.
pub fn find(key: &str) -> Option<&'static Story> {
    find_live(key).or_else(|| moved_to(key).and_then(find_live))
}

fn find_live(key: &str) -> Option<&'static Story> {
    all().find(|story| story.key == key)
}

/// The live key an old key moved to, or None when the key never moved.
pub fn moved_to(key: &str) -> Option<&'static str> {
    MOVED.iter().find(|(old, _)| *old == key).map(|(_, new)| *new)
}

/// Keys that named a page before the catalogue was regrouped, each with the
/// key of the page that holds that content now. A page that moved keeps its
/// content under a new key; a page that merged into another is gone, and its
/// key leads to the page it joined. Every new key is a live page and never
/// another old key, so one step always lands.
pub const MOVED: &[(&str, &str)] = &[
    // Overview
    ("text/slug/overview", "overview/large-text/overview"),
    // Layout
    ("containers/layout/overview", "layout/layout/overview"),
    ("containers/layout/responsive", "layout/layout/responsive"),
    ("navigation/adaptiveview/overview", "layout/layout/responsive"),
    ("containers/grid/overview", "layout/grid/overview"),
    ("containers/masonry/overview", "layout/masonry/overview"),
    ("containers/splitpane/overview", "layout/splitpane/overview"),
    ("containers/splitter/overview", "layout/splitpane/overview"),
    ("containers/dock/overview", "layout/dock/overview"),
    ("containers/alignscroll/overview", "layout/scrolling/overview"),
    ("containers/scrollbar/overview", "layout/scrolling/overview"),
    ("containers/annotatedscrollbar/overview", "layout/scrolling/marks-and-shadows"),
    // Containers
    ("containers/viewshapes/overview", "containers/view/overview"),
    ("containers/cornercapview/overview", "containers/view/corner-caps"),
    ("layout/accordion/overview", "containers/accordion/overview"),
    ("navigation/pageflip/overview", "containers/pageflip/overview"),
    ("navigation/slidesview/overview", "containers/pageflip/overview"),
    ("containers/surfaces/overview", "containers/movingpanels/overview"),
    ("containers/glasspanel/overview", "containers/glass/overview"),
    ("containers/glasssurfaces/overview", "containers/glass/surfaces"),
    ("containers/glasssurfaces/popups", "containers/glass/sheets"),
    ("containers/glassfloatingsurface/overview", "containers/glass/floating-surface"),
    ("inputs/glass/controls", "containers/glass/controls"),
    // Text
    ("text/text/overview", "text/label/text-styles"),
    ("text/html/overview", "text/textflow/html"),
    ("text/markdown/overview", "text/textflow/markdown"),
    ("text/codeview/overview", "text/codeblock/overview"),
    ("containers/marquee/overview", "text/marquee/overview"),
    // Media
    ("media/icon/tint-and-rotation", "media/icon/overview"),
    ("media/iconset/overview", "media/icon/overview"),
    ("media/image/rounded-and-cropped", "media/image/overview"),
    ("media/imageblend/overview", "media/image/overview"),
    ("media/animatedgif/overview", "media/image/overview"),
    ("media/rotatedimage/overview", "media/image/overview"),
    ("media/media/overview", "media/image/loading-and-fallback"),
    ("data-display/svg/overview", "media/svg/overview"),
    ("data-display/svg/animated-and-shaded", "media/svg/overview"),
    ("data-display/vector/overview", "media/svg/vector"),
    ("data-display/vector/file-and-dsl", "media/svg/vector"),
    ("data-display/vector/layered-icon", "media/svg/vector"),
    ("media/video/overview", "media/playbackbar/overview"),
    // Actions
    ("actions/button/variants", "actions/button/overview"),
    ("actions/linklabel/overview", "actions/button/overview"),
    // Inputs
    ("inputs/field-well/overview", "inputs/textinput/field-well"),
    ("inputs/valueinput/overview", "inputs/numberfield/overview"),
    ("inputs/slider/taper", "inputs/slider/overview"),
    ("inputs/rangeslider/overview", "inputs/slider/range-slider"),
    ("inputs/rotary/bipolar", "inputs/rotary/overview"),
    ("inputs/calendar/overview", "inputs/datepicker/calendar"),
    ("inputs/color-field/overview", "inputs/color-picker/overview"),
    ("inputs/fabcontrols/overview", "inputs/property-inspector/overview"),
    // Selection
    ("inputs/checkbox/overview", "selection/checkbox/overview"),
    ("inputs/checkbox/states", "selection/checkbox/overview"),
    ("inputs/radiogroup/overview", "selection/radiogroup/overview"),
    ("inputs/radiobutton/overview", "selection/radiogroup/overview"),
    ("inputs/select/overview", "selection/select/overview"),
    ("inputs/dropdown/overview", "selection/select/overview"),
    ("inputs/combobox/overview", "selection/select/combo-box"),
    ("inputs/chip/overview", "selection/chip/overview"),
    ("inputs/wheelpicker/overview", "selection/wheelpicker/overview"),
    ("inputs/column-picker/overview", "selection/column-picker/overview"),
    ("inputs/svg-select/overview", "selection/svg-select/overview"),
    // Navigation
    ("navigation/pageheader/overview", "navigation/toolbar/page-header"),
    ("navigation/windowchrome/overview", "navigation/toolbar/window-chrome"),
    ("inputs/dropcontrols/overview", "navigation/toolbar/drop-controls"),
    // Overlay
    ("navigation/menu/overview", "overlay/menu/overview"),
    ("navigation/pie-menu/overview", "overlay/pie-menu/overview"),
    ("feedback/tip/overview", "overlay/tip/overview"),
    ("overlay/messages/overview", "overlay/tip/overview"),
    ("overlay/nesting/overview", "overlay/popover/overview"),
    ("overlay/modal/overview", "overlay/dialog/modal"),
    // Feedback
    ("feedback/level-meter/overview", "feedback/progress/level-meter"),
    // Collections
    ("data-display/lists/overview", "collections/lists/overview"),
    ("data-display/portallist/overview", "collections/lists/overview"),
    ("data-display/list-item/overview", "collections/lists/list-item"),
    ("data-display/table/overview", "collections/table/overview"),
    ("data-display/tree/overview", "collections/tree/overview"),
    ("data-display/tree/files", "collections/tree/files"),
    ("data-display/datagrid/overview", "collections/datagrid/overview"),
    ("data-display/datagrid/editing", "collections/datagrid/overview"),
    ("data-display/tilelist/overview", "collections/tilelist/overview"),
    ("data-display/itemgrid/overview", "collections/tilelist/item-grid"),
    ("containers/kanbanboard/overview", "collections/kanbanboard/overview"),
    ("data-display/loglist/overview", "collections/loglist/overview"),
    // Data display
    ("data-display/charts/several-lines", "data-display/charts/overview"),
];

/// New means added on or after the baseline. Dates are ISO, so the string
/// order is the date order.
/// True when the widget this story documents arrived on or after the
/// baseline. A new story about an old widget is not new.
pub fn is_new(story: &Story, baseline: &str) -> bool {
    story.added >= baseline
}

/// The story a search query lands on: the first whose key, name, component,
/// category, tags, the other widgets it draws, or any word written for it in
/// `synonyms.json` contains the query, case-insensitively.
pub fn matches(story: &Story, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    story.key.contains(&q)
        || story.name.to_lowercase().contains(&q)
        || story.component.to_lowercase().contains(&q)
        || story.category.to_lowercase().contains(&q)
        || story.tags.iter().any(|t| t.to_lowercase().contains(&q))
        // The other widgets the page draws. The segmented control lives on
        // the button-group page and the chip group on the chip page, and
        // searching for either by name was finding nothing at all: `also`
        // was written down for the coverage count and never read here.
        || story.also.iter().any(|a| a.to_lowercase().contains(&q))
        // And what people call these things when they do not know what we
        // call them.
        || crate::synonyms::matches(story.component, &q)
        || story.also.iter().any(|a| crate::synonyms::matches(a, &q))
}

pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Three segments, each a slug. A key travels in a URL and a route, so
    /// every segment is lowercase words joined by dashes: `data display` is
    /// not one.
    fn is_slug_key(key: &str) -> bool {
        key.split('/').count() == 3
            && key.split('/').all(|segment| {
                !segment.is_empty()
                    && segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
    }

    #[test]
    fn keys_are_unique_lowercase_and_three_segments() {
        let mut seen = HashSet::new();
        for story in all() {
            assert!(seen.insert(story.key), "duplicate story key {}", story.key);
            assert!(is_slug_key(story.key), "{} is not category/component/name in slugs", story.key);
            // The first segment follows the category the page is filed under.
            let category = story.category.to_lowercase().replace(' ', "-");
            assert!(
                story.key.starts_with(&format!("{category}/")),
                "{} is filed under {}",
                story.key,
                story.category
            );
        }
    }

    #[test]
    fn every_old_key_is_a_slug_listed_once_and_names_no_live_page() {
        let mut seen = HashSet::new();
        for (old, _) in MOVED {
            assert!(is_slug_key(old), "{old} is not a slug key");
            assert!(seen.insert(*old), "{old} is listed twice");
            assert!(find_live(old).is_none(), "{old} still names a live page");
        }
    }

    #[test]
    fn every_old_key_leads_to_a_live_page_in_one_step() {
        for (old, new) in MOVED {
            assert!(find_live(new).is_some(), "{old} leads to {new}, which is no page");
            assert!(moved_to(new).is_none(), "{old} leads to {new}, which is itself an old key");
        }
    }

    #[test]
    fn an_old_key_opens_the_page_it_moved_to() {
        for (old, new) in MOVED {
            assert_eq!(find(old).map(|story| story.key), Some(*new), "{old}");
        }
        // A live key finds itself, and a key that is neither finds nothing.
        assert_eq!(
            find("overview/welcome/welcome").map(|story| story.key),
            Some("overview/welcome/welcome")
        );
        assert!(find("no/such/story").is_none());
    }

    /// The approved tree: every category in order, every component in order
    /// inside it, and every page in order inside its component.
    const TREE: &[(&str, &[(&str, &[&str])])] = &[
        ("Overview", &[("Welcome", &["Welcome"]), ("Coverage", &["Coverage"]), ("Large text", &["Overview"])]),
        (
            "Foundations",
            &[
                ("Colour", &["Roles", "Surfaces", "Status", "Palette"]),
                ("Type", &["Scale"]),
                ("Spacing", &["Scale"]),
                ("Size", &["Scale"]),
                ("Shape", &["Radius"]),
                ("Elevation", &["Levels"]),
                ("State", &["Layers"]),
                ("Motion", &["Overview"]),
            ],
        ),
        (
            "Layout",
            &[
                ("Layout", &["Overview", "Responsive"]),
                ("Grid", &["Overview"]),
                ("Masonry", &["Overview"]),
                ("SplitPane", &["Overview"]),
                ("Dock", &["Overview"]),
                ("Scrolling", &["Overview", "Marks and shadows"]),
                ("Divider", &["Overview"]),
            ],
        ),
        (
            "Containers",
            &[
                ("View", &["Overview", "Corner caps"]),
                ("Card", &["Overview"]),
                ("Accordion", &["Overview"]),
                ("Carousel", &["Overview"]),
                ("PageFlip", &["Overview"]),
                ("MovingPanels", &["Overview"]),
                ("Glass", &["Overview", "Surfaces", "Sheets", "Floating surface", "Controls"]),
                ("Splash", &["Overview"]),
            ],
        ),
        (
            "Text",
            &[
                ("Label", &["Overview", "Text styles"]),
                ("TextFlow", &["Overview", "Html", "Markdown"]),
                ("RichTextEditor", &["Overview"]),
                ("CodeBlock", &["Overview"]),
                ("Marquee", &["Overview"]),
            ],
        ),
        (
            "Media",
            &[
                ("Icon", &["Overview"]),
                ("Image", &["Overview", "Loading and fallback", "Nine-slice"]),
                ("Svg", &["Overview", "Vector"]),
                ("PlaybackBar", &["Overview"]),
                ("Waveform", &["Overview"]),
            ],
        ),
        (
            "Actions",
            &[
                ("Button", &["Overview"]),
                ("ButtonGroup", &["Overview"]),
                ("FloatingAction", &["Overview", "Anchors and layouts"]),
            ],
        ),
        (
            "Inputs",
            &[
                ("TextInput", &["Overview", "Field well"]),
                ("NumberField", &["Overview"]),
                ("Slider", &["Overview", "Range slider"]),
                ("Rotary", &["Overview", "Knob"]),
                ("Rating", &["Overview"]),
                ("TagField", &["Overview"]),
                ("DatePicker", &["Overview", "Calendar"]),
                ("TimePicker", &["Overview"]),
                ("ColorPicker", &["Overview"]),
                ("Dropzone", &["Overview"]),
                ("Form", &["Overview"]),
                ("PropertyInspector", &["Overview"]),
            ],
        ),
        (
            "Selection",
            &[
                ("CheckBox", &["Overview"]),
                ("RadioGroup", &["Overview"]),
                ("Select", &["Overview", "Combo box"]),
                ("Chip", &["Overview"]),
                ("WheelPicker", &["Overview"]),
                ("ColumnPicker", &["Overview"]),
                ("SvgSelect", &["Overview"]),
            ],
        ),
        (
            "Navigation",
            &[
                ("Toolbar", &["Overview", "Page header", "Window chrome", "Drop controls"]),
                ("Tabs", &["Overview"]),
                ("PillNav", &["Overview"]),
                ("NavList", &["Overview"]),
                ("HamburgerMenu", &["Overview"]),
                ("Breadcrumb", &["Overview"]),
                ("Pagination", &["Overview"]),
                ("StackNavigation", &["Overview"]),
            ],
        ),
        (
            "Overlay",
            &[
                ("Tip", &["Overview"]),
                ("Popover", &["Overview"]),
                ("Menu", &["Overview"]),
                ("PieMenu", &["Overview", "Radial menu"]),
                ("CommandPalette", &["Overview"]),
                ("Dialog", &["Overview", "Modal"]),
                ("Drawer", &["Overview"]),
                ("FloatingPanel", &["Overview"]),
                ("Tour", &["Overview"]),
            ],
        ),
        (
            "Feedback",
            &[
                ("Alert", &["Overview"]),
                ("Toast", &["Overview"]),
                ("Progress", &["Overview", "Level meter"]),
                ("Spinner", &["Overview"]),
                ("Placeholder", &["Overview"]),
                ("EmptyState", &["Overview"]),
            ],
        ),
        (
            "Collections",
            &[
                ("Lists", &["Overview", "List item"]),
                ("Table", &["Overview"]),
                ("Tree", &["Overview", "Files"]),
                ("DataGrid", &["Overview"]),
                ("TileList", &["Overview", "Item grid"]),
                ("KanbanBoard", &["Overview"]),
                ("LogList", &["Overview"]),
            ],
        ),
        (
            "Data display",
            &[
                ("Badge", &["Overview"]),
                ("Avatar", &["Overview"]),
                ("Kbd", &["Overview"]),
                ("Charts", &["Overview", "Shapes"]),
                ("Timeline", &["Overview"]),
                ("ChatBubble", &["Overview"]),
            ],
        ),
    ];

    #[test]
    fn the_catalogue_reads_in_the_approved_order() {
        // The navigator draws categories, components and pages in the order
        // it first meets them in `all()`, so this order is the tree's.
        let expected: Vec<(&str, &str, &str)> = TREE
            .iter()
            .flat_map(|(category, components)| {
                components.iter().flat_map(move |(component, pages)| {
                    pages.iter().map(move |page| (*category, *component, *page))
                })
            })
            .collect();
        let actual: Vec<(&str, &str, &str)> =
            all().map(|story| (story.category, story.component, story.name)).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn templates_are_unique() {
        let mut seen = HashSet::new();
        for story in all() {
            assert!(seen.insert(story.dsl), "two stories share the template {}", story.dsl);
        }
    }

    #[test]
    fn added_dates_are_iso() {
        for story in all() {
            assert!(is_iso_date(story.added), "{} has a bad date {}", story.key, story.added);
        }
    }

    /// The two tags that make a claim about age must agree with the date,
    /// in both directions: a story tagged "new" is about a widget this
    /// programme added, and one tagged "ported" is about a widget the
    /// library already had, whatever day its page was written.
    #[test]
    fn the_age_tags_agree_with_the_marker() {
        for story in all() {
            if story.tags.contains(&"new") {
                assert!(
                    is_new(story, DEFAULT_BASELINE),
                    "{} is tagged new but its widget predates the baseline",
                    story.key
                );
            }
            if story.tags.contains(&"ported") {
                assert!(
                    !is_new(story, DEFAULT_BASELINE),
                    "{} is tagged ported but is marked new",
                    story.key
                );
            }
        }
    }

    #[test]
    fn baseline_marks_on_or_after() {
        let s = Story {
            key: "a/b/c",
            category: "A",
            component: "B",
            also: &[],
            name: "C",
            dsl: "X",
            added: "2026-09-05",
            tags: &[],
            doc: "",
            subject: "",
            feature: None,
            controls: &[],
            on_actions: None,
        };
        assert!(is_new(&s, "2026-09-05"));
        assert!(is_new(&s, "2026-01-01"));
        assert!(!is_new(&s, "2026-09-06"));
    }
}
