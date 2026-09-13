//! The split pane story: two panes, a bar with a grip, and floors that hold
//! against the host as well as against the hand; and the plain splitter,
//! whose floors hold against the hand only.
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

    mod.stories.SplitPaneOverview = StoryPage{
        StoryNote{text: "Two panes and a bar, where the smallest each pane may be is a rule the whole widget keeps: a drag, a host writing a position, a restored layout and a window pulled too narrow all pass through the same arithmetic. Use it for a layout a person arranges and an app saves. The plain Splitter at the foot of this page keeps its floors for the hand only, which suits a host that closes panels by writing a position, and it is what the dock builds its panels from."}

        StoryHeading{text: "Drag the bar"}
        StoryNote{text: "The grip says the bar is something to take hold of. Both panes here are kept to at least 120 points, so the bar stops well short of either edge. It reports where it settled, in points measured from the first pane's edge — that number is what a host writes down to have this layout back tomorrow."}
        StoryRow{
            View{
                width: Fill height: 180.
                subject := SplitPane{
                    axis: Horizontal
                    position: 240.
                    min_a: 120.
                    min_b: 120.
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }
        StoryRow{
            where_ := Label{text: "bar at 240, asked for 240"}
        }

        StoryHeading{text: "A clamp the host cannot argue with either"}
        StoryNote{text: "These buttons call set_position with the number on them. Zero and two thousand are both refused and land on the clamp instead, which is the whole difference from the plain Splitter at the foot of this page: there, a written position is taken as given and the floors only ever bound the drag."}
        StoryRow{
            to_zero := Button{text: "set_position(0)"}
            to_mid := Button{text: "set_position(300)"}
            to_huge := Button{text: "set_position(2000)"}
        }
        StoryRow{
            law_note := Label{text: "press one"}
        }
        StoryRow{
            View{
                width: Fill height: 160.
                lawful := SplitPane{
                    axis: Horizontal
                    position: 300.
                    min_a: 160.
                    min_b: 90.
                    a: Pane{Label{text: "at least 160"}}
                    b: Pane{Label{text: "at least 90"}}
                }
            }
        }

        StoryHeading{text: "Folding is a state, not a position of zero"}
        StoryNote{text: "This is what buys the rule above. Closing a pane says fold, so it never writes a position, so the floors have nothing to fight and can hold everywhere else. The bar stays behind whichever pane is folded: it is the only way back, and dragging it out brings the pane back at its floor and then follows your hand."}
        StoryRow{
            fold_a := Button{text: "fold A"}
            fold_b := Button{text: "fold B"}
            fold_open := Button{text: "open both"}
            fold_note := Label{text: "fold: Open"}
        }
        StoryRow{
            View{
                width: Fill height: 160.
                folding := SplitPane{
                    axis: Horizontal
                    position: 260.
                    min_a: 140.
                    min_b: 140.
                    a: Pane{Label{text: "A"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "Click the bar below and let go without moving it — the bar takes focus and lights up. The arrows move it by key_step, which is 32 here so the steps are easy to see, and Home and End send it to the two clamps. They are the only way to land on a clamp exactly: a drag can be pushed into one but never says where it is."}
        StoryRow{
            View{
                width: Fill height: 160.
                keyed := SplitPane{
                    axis: Horizontal
                    position: 300.
                    min_a: 100.
                    min_b: 100.
                    key_step: 32.
                    a: Pane{Label{text: "arrows, Home, End"}}
                    b: Pane{Label{text: "B"}}
                }
            }
        }

        StoryHeading{text: "Either way round"}
        StoryNote{text: "Side by side or stacked is the axis, and the floors and the grip read the same way in both. A Horizontal axis splits horizontally, so its panes sit left and right and its bar stands upright."}
        StoryRow{
            View{
                width: 300. height: 200.
                SplitPane{
                    axis: Horizontal
                    position: 140.
                    min_a: 60.
                    min_b: 60.
                    a: Pane{Label{text: "left"}}
                    b: Pane{Label{text: "right"}}
                }
            }
            View{
                width: 300. height: 200.
                SplitPane{
                    axis: Vertical
                    position: 90.
                    min_a: 50.
                    min_b: 50.
                    a: Pane{Label{text: "above"}}
                    b: Pane{Label{text: "below"}}
                }
            }
        }

        StoryHeading{text: "A window too small is a passing condition"}
        StoryNote{text: "The same split twice, both asked for 260 points and both keeping each pane to at least 90. On the right there is room and the bar stands where it was asked to. On the left there is not room for both floors at all, so each pane misses its own by the same share rather than one of them taking the whole shortfall — a squeezed layout goes on looking like the layout. Nothing was written to the position to bring that about, which is why the one on the right is still at 260."}
        StoryRow{
            View{
                width: 160. height: 140.
                squeezed := SplitPane{
                    axis: Horizontal
                    position: 260.
                    min_a: 90.
                    min_b: 90.
                    a: Pane{Label{text: "90"}}
                    b: Pane{Label{text: "90"}}
                }
            }
            View{
                width: 400. height: 140.
                roomy := SplitPane{
                    axis: Horizontal
                    position: 260.
                    min_a: 90.
                    min_b: 90.
                    a: Pane{Label{text: "90"}}
                    b: Pane{Label{text: "90"}}
                }
            }
        }

        StoryHeading{text: "The plain Splitter"}
        StoryNote{text: "Splitter says where its bar is as an align, and the readout follows it through a drag. Leave the bar near the middle and it keeps a fraction as the window resizes, Weighted; take it towards an edge and it keeps a number of points against that edge, FromA or FromB. Each pane here has an 80 point floor, and the floor stops the drag and nothing else. The floors are min_vertical and max_vertical, because those are named for the bar and this bar stands upright."}
        StoryRow{
            View{
                width: Fill height: 160.
                plain := Splitter{
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
        StoryNote{text: "Fold a pane and it goes to nothing: the floor does not fight it. A host asking for a closed panel is not a person dragging, and a splitter that clamped its layout would reopen every panel an app tried to close. The bar stays behind the folded pane, because it is the only way back."}
        StoryRow{
            plain_fold_a := Button{text: "fold A"}
            plain_fold_b := Button{text: "fold B"}
            plain_fold_none := Button{text: "open both"}
            plain_folded := Label{text: "collapse: None"}
        }
    }
}

fn fold_text(fold: SplitFold) -> &'static str {
    match fold {
        SplitFold::A => "fold: A, and the bar is still there",
        SplitFold::B => "fold: B, and the bar is still there",
        SplitFold::Open => "fold: Open",
    }
}

fn split_pane_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let subject = root.split_pane(cx, ids!(subject));
    if subject.moved(actions).is_some() {
        let text = format!(
            "bar at {:.0}, asked for {:.0}",
            subject.position(),
            subject.asked_position()
        );
        root.label(cx, ids!(where_)).set_text(cx, &text);
    }

    let lawful = root.split_pane(cx, ids!(lawful));
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
        lawful.set_position(cx, points);
        let (lo, hi) = lawful.limits();
        let text = format!(
            "asked for {points:.0}, bar at {:.0}, clamps at {lo:.0} and {hi:.0}",
            lawful.position()
        );
        root.label(cx, ids!(law_note)).set_text(cx, &text);
    }

    let folding = root.split_pane(cx, ids!(folding));
    let mut fold = None;
    if root.button(cx, ids!(fold_a)).clicked(actions) {
        fold = Some(SplitFold::A);
    }
    if root.button(cx, ids!(fold_b)).clicked(actions) {
        fold = Some(SplitFold::B);
    }
    if root.button(cx, ids!(fold_open)).clicked(actions) {
        fold = Some(SplitFold::Open);
    }
    if let Some(fold) = fold {
        folding.set_fold(cx, fold);
        root.label(cx, ids!(fold_note)).set_text(cx, fold_text(fold));
    }
    // Dragging the bar out of a fold reopens it too, and the readout has to
    // follow that as well as the buttons.
    if let Some(fold) = folding.folded(actions) {
        root.label(cx, ids!(fold_note)).set_text(cx, fold_text(fold));
    }
}

/// The plain splitter's section: its align readout and its fold buttons.
fn splitter_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let plain = root.splitter(cx, ids!(plain));
    if plain.changed(actions).is_some() {
        let text = match plain.align() {
            Some(SplitterAlign::Weighted(f)) => format!("align: Weighted({f:.2})"),
            Some(SplitterAlign::FromA(p)) => format!("align: FromA({p:.0})"),
            Some(SplitterAlign::FromB(p)) => format!("align: FromB({p:.0})"),
            None => "align: none".to_string(),
        };
        root.label(cx, ids!(plain_where)).set_text(cx, &text);
    }

    let mut set = None;
    if root.button(cx, ids!(plain_fold_a)).clicked(actions) {
        set = Some(SplitterCollapse::A);
    }
    if root.button(cx, ids!(plain_fold_b)).clicked(actions) {
        set = Some(SplitterCollapse::B);
    }
    if root.button(cx, ids!(plain_fold_none)).clicked(actions) {
        set = Some(SplitterCollapse::None);
    }
    if let Some(c) = set {
        plain.set_collapse(cx, c);
        let text = match c {
            SplitterCollapse::A => "collapse: A",
            SplitterCollapse::B => "collapse: B",
            SplitterCollapse::None => "collapse: None",
        };
        root.label(cx, ids!(plain_folded)).set_text(cx, text);
    }
}

/// One record has one handler: the split pane's sections, then the plain
/// splitter's.
fn split_pane_page_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    split_pane_actions(cx, root, actions);
    splitter_actions(cx, root, actions);
}

pub const STORIES: &[Story] = &[Story {
    key: "layout/splitpane/overview",
    category: "Layout",
    component: "SplitPane",
    also: &["Splitter"],
    name: "Overview",
    dsl: "SplitPaneOverview",
    added: "2026-09-10",
    tags: &["new", "layout", "split", "panes", "resize", "minimum", "keyboard", "persist"],
    doc: "# SplitPane

Two panes and a bar, where the least each pane may be is a law rather than a courtesy extended to the hand.

`a` and `b` are the panes, `axis` says whether they sit side by side or stacked, `position` is the first pane's size in points, and `min_a` and `min_b` are the floors. There is no weighted or edge-relative align — *What it deliberately does not do* says why.

## The rule, and what it costs to have it

On the plain `Splitter`, described at the end of this note, a floor governs the drag and nothing else, and that is right there: an application closes a panel by writing a position of zero, so a floor applied to the layout would quietly reopen every panel that was ever closed. The cost is that the rule then lives in the host. Every caller that declares a floor and also writes positions — restoring a saved layout, snapping a sidebar to a preset, nudging a pane from a menu — has to apply the floor again itself, in each of those places.

Here folding is a state of its own, so nothing ever closes a pane by writing a position, so the floors have nothing left to fight. They hold for a drag, for `set_position`, for the keyboard, and for a window dragged narrower than the two panes together need.

## Two numbers, not one

`position` is what was **asked** for. `position()` is where the bar **stands**: the ask, brought inside the floors and the room that exists this pass. They are the same number almost always, and they part company exactly when the window is too small to honour the ask.

That is deliberate. If a squeeze wrote itself back into the ask, a window briefly dragged narrow would shrink a saved layout for good, and every application that persists a sidebar width would slowly lose it. So the squeeze moves the bar and leaves the ask alone, and opening the window out again puts the bar back where it was.

A hand on the bar is different: a drag past the floor is a request for the floor, and it is stored as the floor. Only a host may hold an ask the current window cannot honour, because only a host has a reason to — it is asking for a layout, not for a place on this screen.

## When neither floor fits

Drag a window narrower than `min_a + min_b + size` and there is no answer that keeps both floors. Both panes then miss their floor by the same share of it, rather than one of them absorbing the whole shortfall, so a two-hundred point sidebar beside a fifty point gutter stays four times the gutter all the way down. A layout being squeezed goes on looking like the layout.

## Keyboard

The bar is a focus stop. The arrows move it by `key_step`; left and up take room from the first pane whichever way the split runs. Home and End are the two clamps themselves, which nothing else reaches exactly. An arrow pressed while a pane is folded brings that pane back to where it was rather than stepping from the edge: the keyboard has no position of its own to follow, so the remembered one is the only answer that is not invented. A drag out of a fold does follow the hand, and the pane comes back at its floor and waits there until the finger has passed it.

## Persisting it

`settled` reports the number to write down when a gesture ends; `moved` reports every frame of one, for a readout. Put a saved number back with `set_position` and it arrives through the law, so a bar remembered from a wider window still comes back legal.

## What it deliberately does not do

No weighted or edge-relative align. A share of the window and a minimum size in points cannot both be honoured while the window shrinks, and when they disagree it is always the share that gives way — offering one would be offering a promise this widget cannot keep. It splits two panes and does not nest, tile or reorder them; three panes are two of these.

## The plain Splitter

`Splitter` is the other two-pane widget, and the one the dock builds its panels from.

`a` and `b` are the panes and `axis` works as it does above. Where the bar sits is an `align`: `Weighted(f)` keeps a fraction as the window resizes, `FromA(points)` and `FromB(points)` keep a fixed distance against one edge. A drag reports the axis and the align on every move, as `changed`. A bar left within 30 points of the middle keeps a `Weighted` align, and one left nearer an edge keeps its distance from that edge.

**The floor fields are named for the bar, not for the axis, and the two words are opposites.** A `Horizontal` axis splits horizontally, so the panes sit left and right and the bar between them is *vertical* — and it is `min_vertical` and `max_vertical` that bound it. A `Vertical` axis stacks the panes and uses the horizontal pair. `min_` is pane A's floor and `max_` is pane B's.

**A floor governs the hand, not the host.** The floors are applied where the drag is handled, so they bound what a person can drag to. They are *not* applied when the splitter lays itself out, and that is what lets an app close a panel: `collapse`, or `set_collapse` from code, folds a pane to nothing, and a splitter that clamped its own layout would reopen it to its floor every time. The layout clamps only into the room there actually is.

**Folding leaves the bar behind.** Whichever pane is folded, the bar is still drawn at that edge, because it is the only way to bring the pane back.",
    subject: "subject",
    feature: None,
    controls: &[
        Control { label: "Position", target: "subject", kind: ControlKind::Number { prop: "position", min: 0., max: 900., step: 10., default: 240. } },
        Control { label: "Least A", target: "subject", kind: ControlKind::Number { prop: "min_a", min: 0., max: 400., step: 10., default: 120. } },
        Control { label: "Least B", target: "subject", kind: ControlKind::Number { prop: "min_b", min: 0., max: 400., step: 10., default: 120. } },
        Control { label: "Bar", target: "subject", kind: ControlKind::Number { prop: "size", min: 2., max: 24., step: 1., default: 8. } },
        Control { label: "Key step", target: "subject", kind: ControlKind::Number { prop: "key_step", min: 1., max: 96., step: 1., default: 16. } },
    ],
    on_actions: Some(split_pane_page_actions),
}];
