//! The curve editor page: the material bench's curve editor on the three
//! curves it edits most, a revolve profile, half a tooth with the whole one
//! it makes, and a wing height against the cap's top, and a readout of the
//! curve each edit settles on.
//!
//! The readout is a label with an id, so `/snap?q=curve` on the `--remote`
//! surface reads the last commit where no screenshot is available; each
//! editor's own snapshot value is its anchors.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Column = View{
        width: 560
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    mod.stories.FoundationsMotionCurveEditor = StoryPage{
        StoryNote{text: "A curve of anchors over a unit box. Press empty canvas to add a point, drag one to move it, drag an inner one out of the box to delete it; Shift-click picks several, and they drag together. The row under the canvas sets the kind of what is picked and shows the kind it has: Smooth (round), Corner (square), Horizontal (wide, held level) or Point (diamond, straight lines)."}

        StoryHeading{text: "A revolve profile"}
        Column{
            profile := CurveEditor{
                anchors: [[0, 1, 1], [0.28, 0.97, 1], [0.34, 0.3, 2], [0.93, 0.25, 1], [1, 0, 2]]
                left_label: "CENTRE"
                right_label: "SKIRT RIM"
            }
        }

        StoryHeading{text: "Half a tooth, and the whole one it makes"}
        StoryNote{text: "The faint line across the top is this half followed by its own mirror: whether a crest meets its neighbour in a point or a flat only shows in the pair."}
        Column{
            tooth := CurveEditor{
                anchors: [[0, 1, 2], [0.4, 0.72, 1], [1, 0, 2]]
                left_label: "CREST"
                right_label: "TROUGH"
                mirror: true
            }
        }

        StoryHeading{text: "A height against a reference"}
        Column{
            wing := CurveEditor{
                anchors: [[0, 0.72, 0], [0.5, 0.72, 3], [0.8, 0.55, 1], [1, 0.12, 1]]
                left_label: "ROOT"
                right_label: "TIP"
                guide: 0.5
                guide_label: "CAP TOP"
            }
        }

        StoryHeading{text: "What the last edit settled on"}
        curve_readout := Label{
            width: Fill
            text: "no edit yet"
            draw_text +: {
                text_style: theme.font_code{font_size: theme.font_size_p}
                color: theme.color_text
            }
        }
    }
}

/// The readout line: which editor, then each anchor as x, y and kind.
fn describe(name: &str, anchors: &[CurveAnchor]) -> String {
    let mut s = format!("{name}: {} anchors |", anchors.len());
    for a in anchors {
        let kind = match a[2] as u8 {
            0 => "point",
            1 => "smooth",
            2 => "corner",
            _ => "horizontal",
        };
        let _ = write!(s, " ({:.2}, {:.2}) {kind}", a[0], a[1]);
    }
    s
}

fn curve_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for (id, name) in [(ids!(profile), "profile"), (ids!(tooth), "tooth"), (ids!(wing), "wing height")] {
        if let Some(anchors) = root.curve_editor(cx, id).committed(actions) {
            root.label(cx, ids!(curve_readout))
                .set_text(cx, &describe(name, &anchors));
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/curve-editor",
    category: "Foundations",
    component: "Motion",
    also: &["CurveEditor"],
    name: "Curve editor",
    dsl: "FoundationsMotionCurveEditor",
    added: "2026-09-27",
    tags: &["new", "curve", "bezier", "profile", "anchors", "editor"],
    doc: "# Curve editor\n\n`CurveEditor` edits a curve of anchors over a unit box: `[x, y, kind]` rows, x and y in 0..1, joined by cubic Beziers whose tangents are derived from the neighbours. The kind says how: **Point** (0) runs straight to both neighbours, **Smooth** (1) is mirrored and goes flat at a peak or a trough, **Corner** (2) clamps each side on its own, **Horizontal** (3) is smooth held level. It is the material bench's editor, ported; `curve_polyline` and `curve_resample` are the bench's own sampling, so the curve drawn is the curve a bake reads.\n\n- **Press** empty canvas to add a smooth anchor between its neighbours; press an anchor to pick it, Shift or Ctrl to pick several.\n- **Drag** moves what is picked; the ends keep x 0 and 1, and each anchor stays inside its neighbours. An inner anchor dragged out of the box turns dashed and goes on release.\n- **The kind row** sets the kind of what is picked and lights the kind every picked anchor shares (Horizontal on several also levels them at the last one's height); Delete removes picked inner anchors.\n- **Actions**: `changed(&actions)` on every edit, `committed(&actions)` when a gesture ends; `set_anchors` and `anchors` from code.\n\n`left_label`, `right_label`, `guide` with `guide_label`, and `mirror` (the curve followed by its mirror, faint, across the top) dress the canvas. Tangent handles are drawn but not dragged yet.",
    subject: "profile",
    feature: None,
    controls: &[Control {
        label: "Mirror",
        target: "tooth",
        kind: ControlKind::Bool {
            prop: "mirror",
            default: true,
        },
    }],
    on_actions: Some(curve_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;
    use crate::makepad_widgets::makepad_script::trap::NoTrap;

    /// The page is markup the compiler never reads: building it is what
    /// turns a misspelt widget, a curve the editor cannot read or a control
    /// aimed at nothing into a failed test rather than an empty canvas.
    #[test]
    fn the_page_builds_and_each_editor_holds_its_curve() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
        });
        let _ = makepad_platform::shader_error::take();
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile");
        for target in [story.subject, story.controls[0].target, "wing", "curve_readout"] {
            assert!(!page.widget(&cx, &id_path(target)).is_empty(), "no widget at {target}");
        }
        assert_eq!(page.curve_editor(&cx, ids!(profile)).anchors().len(), 5);
        assert_eq!(
            page.curve_editor(&cx, ids!(tooth)).anchors(),
            vec![[0.0, 1.0, 2.0], [0.4, 0.72, 1.0], [1.0, 0.0, 2.0]]
        );
        assert_eq!(page.curve_editor(&cx, ids!(wing)).anchors()[1], [0.5, 0.72, 3.0]);
    }
}
