//! The splitter story: two panes and a bar, the floors, the fold, and
//! SplitPane — the same widget with the floors kept as a law, a grip, the
//! keyboard and a bar that brings a folded pane back.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Pane = RectView{
        width: Fill
        height: Fill
        align: Align{x: 0.5, y: 0.5}
        draw_bg +: {color: theme.color_surface_container_low}
    }

    mod.stories.SplitterOverview = StoryPage{
        StoryNote{text: "Two panes and a bar between them. Splitter is what the dock builds its panels from, and it is the one widget on this page: SplitPane, further down, is Splitter with three of its properties switched on and a grip drawn on the bar, for a layout a person arranges and an app saves."}

        StoryHeading{text: "Drag the bar"}
        StoryNote{text: "Splitter says where its bar is as an align, and the readout follows it through a drag. Leave the bar near the middle and it keeps a fraction as the window resizes, Weighted; take it towards an edge and it keeps a number of points against that edge, FromA or FromB. Each pane here has an 80 point floor. The floors are min_vertical and max_vertical, because those are named for the bar and this bar stands upright."}
        StoryRow{
            View{
                width: Fill height: 160.
                subject := Splitter{
                    axis: Horizontal
                    align: Weighted(0.5)
                    min_vertical: 80.
                    max_vertical: 80.
                    a: Pane{Label{text: "A, 80 minimum"}}
                    b: Pane{Label{text: "B, 80 minimum"}}
                }
            }
        }
        StoryRow{
            plain_where := Label{text: "align: Weighted(0.50)"}
        }
        StoryNote{text: "Fold a pane and it goes to nothing: the floor does not fight a fold, which is why a host closes a panel by folding it and not by writing a position of zero. The align is never touched, so opening the pane again is the bar that went away. The bar stays behind the folded pane but on this splitter it does nothing until the host unfolds: switch on bar_reopens in the controls and a drag on it brings the pane back."}
        StoryRow{
            plain_fold_a := Button{text: "fold A"}
            plain_fold_b := Button{text: "fold B"}
            plain_fold_none := Button{text: "open both"}
            plain_folded := Label{text: "collapse: None"}
        }

        StoryHeading{text: "SplitPane: the floors as a law"}
        StoryNote{text: "SplitPane is Splitter with exact_floors, key_step and bar_reopens switched on, a thicker bar and three grip marks. The grip says the bar is something to take hold of. Both panes here are kept to at least 120 points, and under the law that is 120 points of pane: the bar's own thickness is not taken out of the second pane's share. A drag is written back in the align's own kind, so this one stays a number of points from the first pane's edge however near the middle it is left — which is the number a host writes down to have this layout back tomorrow."}
        StoryRow{
            View{
                width: Fill height: 180.
                law := SplitPane{
                    axis: Horizontal
                    align: FromA(240.)
                    min_vertical: 120.
                    max_vertical: 120.
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }
        StoryRow{
            where_ := Label{text: "bar at 240, align: FromA(240)"}
        }

        StoryHeading{text: "What was asked for, and where the bar stands"}
        StoryNote{text: "These buttons call set_align with the align on them. Zero and two thousand are both brought inside the floors, and the align is left saying what was asked: the two part company exactly when the floors or the window cannot honour the ask. Under the law the answer is there to read at once, where the plain Splitter's position is the layout pass's to set at the next draw."}
        StoryRow{
            to_zero := Button{text: "set_align(FromA(0))"}
            to_mid := Button{text: "set_align(FromA(300))"}
            to_huge := Button{text: "set_align(FromA(2000))"}
        }
        StoryRow{
            law_note := Label{text: "press one"}
        }
        StoryRow{
            View{
                width: Fill height: 160.
                lawful := SplitPane{
                    axis: Horizontal
                    align: FromA(300.)
                    min_vertical: 160.
                    max_vertical: 90.
                    a: Pane{Label{text: "at least 160"}}
                    b: Pane{Label{text: "at least 90"}}
                }
            }
        }

        StoryHeading{text: "The bar is the way back from a fold"}
        StoryNote{text: "Folding is a state and not a position of zero, here as on the plain Splitter. What bar_reopens adds is the way back: the bar stays behind whichever pane is folded, and dragging it out brings the pane back at its floor and then follows your hand. A press alone does nothing, because the bar is also the thing you click on your way to something else."}
        StoryRow{
            fold_a := Button{text: "fold A"}
            fold_b := Button{text: "fold B"}
            fold_open := Button{text: "open both"}
            fold_note := Label{text: "collapse: None"}
        }
        StoryRow{
            View{
                width: Fill height: 160.
                folding := SplitPane{
                    axis: Horizontal
                    align: FromA(260.)
                    min_vertical: 140.
                    max_vertical: 140.
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "Click the bar below and let go without moving it — the bar takes focus and lights up. The arrows move it by key_step, which is 32 here so the steps are easy to see, and Home and End send it to the two clamps. They are the only way to land on a clamp exactly: a drag can be pushed into one but never says where it is. A key_step of nothing, which is what the plain Splitter has, keeps the bar off the keyboard altogether."}
        StoryRow{
            View{
                width: Fill height: 160.
                keyed := SplitPane{
                    axis: Horizontal
                    align: FromA(300.)
                    min_vertical: 100.
                    max_vertical: 100.
                    key_step: 32.
                    a: Pane{Label{text: "arrows, Home, End"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }

        StoryHeading{text: "Either way round"}
        StoryNote{text: "Side by side or stacked is the axis, and the floors and the grip read the same way in both. A Horizontal axis splits horizontally, so its panes sit left and right and its bar stands upright; the stacked one's bar lies flat, so its floors are min_horizontal and max_horizontal."}
        StoryRow{
            View{
                width: 300. height: 200.
                SplitPane{
                    axis: Horizontal
                    align: FromA(140.)
                    min_vertical: 60.
                    max_vertical: 60.
                    a: Pane{Label{text: "left"}}
                    b: Pane{Label{text: "right"}}
                }
            }
            View{
                width: 300. height: 200.
                SplitPane{
                    axis: Vertical
                    align: FromA(90.)
                    min_horizontal: 50.
                    max_horizontal: 50.
                    a: Pane{Label{text: "above"}}
                    b: Pane{Label{text: "below"}}
                }
            }
        }

        StoryHeading{text: "A window too small is a passing condition"}
        StoryNote{text: "The same split twice, both asked for 260 points and both keeping each pane to at least 90. On the right there is room and the bar stands where it was asked to. On the left there is not room for both floors at all, so each pane misses its own by the same share rather than one of them taking the whole shortfall — a squeezed layout goes on looking like the layout. Nothing was written to the align to bring that about, which is why the one on the right is still at 260."}
        StoryRow{
            View{
                width: 160. height: 140.
                squeezed := SplitPane{
                    axis: Horizontal
                    align: FromA(260.)
                    min_vertical: 90.
                    max_vertical: 90.
                    a: Pane{Label{text: "90"}}
                    b: Pane{Label{text: "90"}}
                }
            }
            View{
                width: 400. height: 140.
                roomy := SplitPane{
                    axis: Horizontal
                    align: FromA(260.)
                    min_vertical: 90.
                    max_vertical: 90.
                    a: Pane{Label{text: "90"}}
                    b: Pane{Label{text: "90"}}
                }
            }
        }
    }
}

fn align_text(align: Option<SplitterAlign>) -> String {
    match align {
        Some(SplitterAlign::Weighted(f)) => format!("align: Weighted({f:.2})"),
        Some(SplitterAlign::FromA(p)) => format!("align: FromA({p:.0})"),
        Some(SplitterAlign::FromB(p)) => format!("align: FromB({p:.0})"),
        None => "align: none".to_string(),
    }
}

fn collapse_text(collapse: SplitterCollapse) -> &'static str {
    match collapse {
        SplitterCollapse::A => "collapse: A, and the bar is still there",
        SplitterCollapse::B => "collapse: B, and the bar is still there",
        SplitterCollapse::None => "collapse: None",
    }
}

/// The fold a row of three buttons asks for, if one of them was pressed.
fn fold_asked(
    cx: &mut Cx,
    root: &WidgetRef,
    actions: &Actions,
    a: &[LiveId],
    b: &[LiveId],
    none: &[LiveId],
) -> Option<SplitterCollapse> {
    if root.button(cx, a).clicked(actions) {
        Some(SplitterCollapse::A)
    } else if root.button(cx, b).clicked(actions) {
        Some(SplitterCollapse::B)
    } else if root.button(cx, none).clicked(actions) {
        Some(SplitterCollapse::None)
    } else {
        None
    }
}

/// The plain splitter's section: its align readout and its fold buttons.
fn splitter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let plain = root.splitter(cx, ids!(subject));
    if plain.changed(actions).is_some() {
        root.label(cx, ids!(plain_where)).set_text(cx, &align_text(plain.align()));
    }

    if let Some(collapse) =
        fold_asked(cx, root, actions, ids!(plain_fold_a), ids!(plain_fold_b), ids!(plain_fold_none))
    {
        plain.set_collapse(cx, collapse);
        root.label(cx, ids!(plain_folded)).set_text(cx, collapse_text(collapse));
    }
    // With bar_reopens switched on in the controls a drag unfolds it too,
    // and the readout has to follow that as well as the buttons.
    if let Some(collapse) = plain.collapsed(actions) {
        root.label(cx, ids!(plain_folded)).set_text(cx, collapse_text(collapse));
    }
}

/// SplitPane's sections.
fn split_pane_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let law = root.splitter(cx, ids!(law));
    if law.changed(actions).is_some() {
        let text = format!(
            "bar at {:.0}, {}",
            law.position().unwrap_or(0.0),
            align_text(law.align())
        );
        root.label(cx, ids!(where_)).set_text(cx, &text);
    }

    let lawful = root.splitter(cx, ids!(lawful));
    let mut asked = None;
    if root.button(cx, ids!(to_zero)).clicked(actions) {
        asked = Some(0.0);
    }
    if root.button(cx, ids!(to_mid)).clicked(actions) {
        asked = Some(300.0);
    }
    if root.button(cx, ids!(to_huge)).clicked(actions) {
        asked = Some(2000.0);
    }
    if let Some(points) = asked {
        lawful.set_align(cx, SplitterAlign::FromA(points));
        let (lo, hi) = lawful.limits().unwrap_or((0.0, 0.0));
        let text = format!(
            "asked for {:.0}, bar at {:.0}, clamps at {lo:.0} and {hi:.0}",
            lawful.asked_position().unwrap_or(0.0),
            lawful.position().unwrap_or(0.0)
        );
        root.label(cx, ids!(law_note)).set_text(cx, &text);
    }

    let folding = root.splitter(cx, ids!(folding));
    if let Some(collapse) = fold_asked(cx, root, actions, ids!(fold_a), ids!(fold_b), ids!(fold_open)) {
        folding.set_collapse(cx, collapse);
        root.label(cx, ids!(fold_note)).set_text(cx, collapse_text(collapse));
    }
    // Dragging the bar out of a fold reopens it too, and the readout has to
    // follow that as well as the buttons.
    if let Some(collapse) = folding.collapsed(actions) {
        root.label(cx, ids!(fold_note)).set_text(cx, collapse_text(collapse));
    }
}

/// One record has one handler: the plain splitter's section, then
/// SplitPane's.
fn splitter_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    splitter_actions(cx, root, actions);
    split_pane_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/splitter/overview",
    category: "Layout",
    component: "Splitter",
    also: &["SplitPane"],
    name: "Overview",
    dsl: "SplitterOverview",
    added: "2025-05-06",
    tags: &["ported", "layout", "split", "panes", "resize", "minimum", "keyboard", "persist"],
    doc: "# Splitter

Two panes and a bar between them. It is the widget the dock builds its panels from, and `SplitPane` is a preset of it rather than a second widget.

`a` and `b` are the panes and `axis` says whether they sit side by side or stacked. Where the bar sits is an `align`: `Weighted(f)` keeps a fraction as the window resizes, `FromA(points)` and `FromB(points)` keep a fixed distance against one edge. A drag reports the axis and the align on every move, as `changed`, and once more as `settled` when the gesture ends having moved the bar.

## Two numbers, not one

`align` is what was **asked** for. `position()` is where the bar **stands**: the ask, brought inside the floors and the room that exists this pass. `asked_position()` is the ask in the same points. They are the same number almost always, and they part company exactly when the floors or the window cannot honour the ask.

That is deliberate. If a squeeze wrote itself back into the ask, a window briefly dragged narrow would shrink a saved layout for good, and every application that persists a sidebar width would slowly lose it. So the squeeze moves the bar and leaves the align alone, and opening the window out again puts the bar back where it was.

A hand on the bar is different: a drag past the floor is a request for the floor, and it is stored as the floor.

## The floors

**The floor fields are named for the bar, not for the axis, and the two words are opposites.** A `Horizontal` axis splits horizontally, so the panes sit left and right and the bar between them is *vertical* — and it is `min_vertical` and `max_vertical` that bound it. A `Vertical` axis stacks the panes and uses the horizontal pair. `min_` is pane A's floor and `max_` is pane B's.

The floors hold for a drag and for an align a host writes. They do not hold for a fold.

## Folding

`collapse`, or `set_collapse` from code, folds a pane to nothing, past any floor. It is a state and not a position of zero, so the align is never written and unfolding is the bar that went away, with nobody having remembered it. A host closes a panel this way and not by writing zero, which the floor would bring straight back.

**Folding leaves the bar behind.** Whichever pane is folded, the bar is still drawn at that edge. On the plain splitter it does nothing while the pane is folded, which suits a host that folds and unfolds from a button of its own. `bar_reopens` makes the bar the way back: a drag out of the fold brings the pane back at its floor and waits there until the finger has passed it, and an arrow key brings it back to where it was.

## The keyboard

`key_step` is the points one arrow press moves the bar. At nothing, which is the default, the bar is off the keyboard altogether: no focus stop, and a press does not take the focus. With a step the bar is a focus stop and lights up when it has the focus; left and up take room from the first pane whichever way the split runs, and Home and End are the two clamps themselves, which nothing else reaches exactly. `limits()` says where they are.

## The floors as a law

`exact_floors` changes the arithmetic the floors are kept with, and it is off by default because the dock is laid out by the other one.

| | off | on |
|---|---|---|
| the floors are measured in | the whole room, so the bar's thickness comes out of pane B's floor | the room the two panes share, so each gets its whole floor |
| a room too small for both floors | puts the bar in the middle | is shared so each pane misses its floor by the same share of it |
| a drag starts from | what the align asks for | where the bar stands |
| a drag is written back as | a share near the middle, points near an edge | the kind of align it already was |
| `position()` after `set_align` | is set at the next draw | answers at once |

The shared shortfall is why a two-hundred point sidebar beside a fifty point gutter stays four times the gutter all the way down: a layout being squeezed goes on looking like the layout. Keeping the align's kind is what lets a host persist points and never be handed a share of the window to put back.

## SplitPane

```
mod.widgets.SplitPane = mod.widgets.Splitter{
    exact_floors: true
    key_step: 16.0
    bar_reopens: true
    size: 8.0
    align: mod.widgets.SplitterAlign.FromA(240.0)
    min_vertical: 80.0
    max_vertical: 80.0
    min_horizontal: 80.0
    max_horizontal: 80.0
    draw_bg +: {bar_size: 44.0 border_radius: 2.0 grip_dot: 1.25}
}
```

That is the whole of it: there is no Rust type behind the name, and `root.splitter(..)` is how code reaches one. It sets both pairs of floors so that one stood on end keeps the floors it had lying down. `grip_dot` is the radius of the three marks on the bar and is nothing on the plain splitter; `grip_gap` spaces them and `grip_color` tints them.

## Persisting it

`settled` reports the align to write down when a gesture ends; `changed` reports every frame of one, for a readout. Put a saved align back with `set_align` and under the law it arrives through the floors, so a bar remembered from a wider window still comes back legal.

It splits two panes and does not nest, tile or reorder them; three panes are two of these, and a set of panels a person rearranges is the dock.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Law", target: "subject", kind: ControlKind::Bool { prop: "exact_floors", default: false } },
        Control { label: "Bar reopens", target: "subject", kind: ControlKind::Bool { prop: "bar_reopens", default: false } },
        Control { label: "Key step", target: "subject", kind: ControlKind::Number { prop: "key_step", min: 0., max: 96., step: 1., default: 0. } },
        Control { label: "Least A", target: "subject", kind: ControlKind::Number { prop: "min_vertical", min: 0., max: 400., step: 10., default: 80. } },
        Control { label: "Least B", target: "subject", kind: ControlKind::Number { prop: "max_vertical", min: 0., max: 400., step: 10., default: 80. } },
        Control { label: "Bar", target: "subject", kind: ControlKind::Number { prop: "size", min: 2., max: 24., step: 1., default: 6. } },
    ],
    on_actions: Some(splitter_page_actions),
}];
