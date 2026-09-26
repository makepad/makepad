//! The toolbar stories: the three columns of a bar, the same bar at the top
//! and the bottom of a window, and the block version at the top of a page.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::sync::Mutex;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    // Bars are drawn edge to edge, so each one gets a strip of its own.
    let BarStrip = View{
        width: Fill
        height: Fit
        flow: Down
    }

    mod.stories.ToolbarOverview = StoryPage{
        StoryNote{text: "Three columns: something at the leading edge, something at the trailing edge, and one thing between them that takes whatever room the two ends leave. The middle is reserved before the trailing slot is drawn and sized after it — laid out in order it would take the whole row, and the actions at the far end would be measured into nothing."}
        StoryHeading{text: "One toolbar, under the controls"}
        BarStrip{
            subject := AppBar{
                width: Fill
                title: "Settings"
                leading: ButtonFlat{text: "\u{2039} Back"}
                trailing: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonFlat{text: "Reset"}
                    Button{text: "Done"}
                }
            }
        }

        StoryHeading{text: "Two ends and a middle"}
        StoryNote{text: "The leading and trailing slots take what they ask for. The gap either side of the middle only exists when there is something on that side, so a bar with no navigation control starts its title at the padding rather than a gap in from it."}
        BarStrip{
            plain := Toolbar{
                width: Fill
                title: "Untitled document"
                leading: ButtonFlat{text: "\u{2039} Back"}
                trailing: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonFlat{text: "Share"}
                    Button{text: "Save"}
                }
            }
        }

        StoryHeading{text: "The bar at the top"}
        StoryNote{text: "`AppBar` is that row with a rule along its bottom edge and a taller bar. It is not another widget: two bars that disagree about how tall a bar is are how one application comes to look like two."}
        BarStrip{
            topbar := AppBar{
                width: Fill
                title: "Library"
                leading: ButtonFlat{text: "\u{2039} Back"}
                trailing: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    bar_share := ButtonFlat{text: "Share"}
                    ButtonFlat{text: "Filter"}
                }
            }
        }
        StoryRow{
            bar_note := Label{text: "nothing shared yet"}
        }

        StoryHeading{text: "Flat, and raised"}
        StoryNote{text: "A raised bar changes its own surface. It casts no shadow, because a shadow would have to be drawn over content the bar does not own and cannot redraw. Nothing here watches the scroll either — a bar cannot find the view underneath it, so the host reads its own offset and calls `set_raised_for_scroll`."}
        StoryNote{text: "Two thresholds, not one. This bar lifts at 8 points and does not settle again until 2, so step it down past 8 and back and watch it stay raised through the middle. With a single threshold the bar would change colour on every fraction of a point the content drifts by as it comes to rest."}
        BarStrip{
            lifting := AppBar{
                width: Fill
                title: "Inbox"
                leading: ButtonFlat{text: "\u{2039} Back"}
                trailing: ButtonFlat{text: "Edit"}
            }
        }
        StoryRow{
            scroll_down := ButtonFlat{text: "Scroll down 2"}
            scroll_up := ButtonFlat{text: "Scroll up 2"}
            scroll_note := Label{text: "0 points scrolled \u{2014} flat"}
        }

        StoryHeading{text: "The bar at the bottom"}
        StoryNote{text: "The same row upside down: the rule moves to the top edge, and one further slot past the actions carries the action the screen exists for."}
        BarStrip{
            bottombar := BottomAppBar{
                width: Fill
                leading: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonFlat{text: "Edit"}
                    ButtonFlat{text: "Copy"}
                    ButtonFlat{text: "Delete"}
                }
                floating: ButtonPrimary{text: "New"}
            }
        }
        StoryNote{text: "`float_lift` stands that action proud of the bar's middle. It stays inside the bar's own height, so a host that wants it over the edge gives the bar the height to do it in."}
        BarStrip{
            lifted := BottomAppBar{
                width: Fill
                float_lift: 10.
                leading: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonFlat{text: "Edit"}
                    ButtonFlat{text: "Copy"}
                }
                floating: ButtonPrimary{text: "New"}
            }
        }

        StoryHeading{text: "Something else in the middle"}
        StoryNote{text: "The centre slot takes the room the two ends leave, which is what a search box in a bar wants. It replaces the title rather than sharing the middle with it: two things measured against a width neither can be measured against is how a title comes to run out over the box beside it."}
        BarStrip{
            searching := Toolbar{
                width: Fill
                title: "This title stands down"
                leading: ButtonFlat{text: "Menu"}
                center: TextInput{
                    width: Fill
                    empty_text: "Search everything"
                }
                trailing: ButtonFlat{text: "Cancel"}
            }
        }

        StoryHeading{text: "Where the middle sits"}
        StoryNote{text: "`middle_align` is 0 at the leading edge and 0.5 in the centre. A centred title is a phone's bar; a leading one is a window's."}
        BarStrip{
            AppBar{
                width: Fill
                title: "Centred"
                middle_align: 0.5
                leading: ButtonFlat{text: "\u{2039} Back"}
                trailing: ButtonFlat{text: "Edit"}
            }
        }

        StoryHeading{text: "Which edge carries the rule"}
        StoryNote{text: "`ToolbarEdge.None`, `Top` and `Bottom`. A bar floating on a surface of its own needs no rule at all."}
        BarStrip{
            Toolbar{
                width: Fill
                title: "No rule"
                edge: ToolbarEdge.None
            }
        }
        BarStrip{
            Toolbar{
                width: Fill
                title: "Rule above"
                edge: ToolbarEdge.Top
            }
        }
        BarStrip{
            Toolbar{
                width: Fill
                title: "Rule below"
                edge: ToolbarEdge.Bottom
            }
        }
    }


    mod.stories.PageHeaderOverview = StoryPage{
        StoryNote{text: "The same idea as a block rather than a row, for the top of a page rather than the top of a window: a trail above, a title with an optional subtitle and a description, and the page's actions beside the words rather than under them."}

        StoryHeading{text: "The whole block"}
        StoryRow{
            width: Fill
            header := PageHeader{
                width: Fill
                title: "Quarterly report"
                subtitle: "Updated 10 minutes ago by the finance team"
                description: "Everything shipped between April and June, what it cost, and what is still open. The description wraps to whatever column the actions leave it."
                breadcrumb: Breadcrumb{trail: ["Home" "Reports" "Quarterly report"]}
                actions: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonOutline{text: "Export"}
                    ButtonPrimary{text: "Share"}
                }
            }
        }

        StoryHeading{text: "A title on its own"}
        StoryNote{text: "Every part but the title is optional, and a part that is not there costs no room: an empty subtitle leaves no gap where a subtitle would have been."}
        StoryRow{
            width: Fill
            PageHeader{
                width: Fill
                title: "Members"
            }
        }

        StoryHeading{text: "Words and actions"}
        StoryNote{text: "The words are the same deferred fill the toolbar's middle is. Taken in order they would have the whole row and the actions would land past the page's edge, so the actions are drawn first and the column is sized against what they left."}
        StoryRow{
            width: Fill
            PageHeader{
                width: Fill
                title: "Billing"
                description: "One long line, so the column the actions leave is the column the words wrap to rather than a width somebody guessed at."
                actions: View{
                    width: Fit height: Fit flow: Right
                    spacing: theme.space_1
                    align: Align{x: 0.5 y: 0.5}
                    ButtonOutline{text: "Download invoices"}
                    ButtonPrimary{text: "Change plan"}
                }
            }
        }
    }
}

/// The pretend scroll offset the raised story steps up and down. The bar
/// keeps no offset of its own — that is the point of `set_raised_for_scroll`
/// — so the page holds it, exactly as a host with a real scrolling view
/// would.
static SCROLL: Mutex<f64> = Mutex::new(0.0);

fn toolbar_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let down = root.button(cx, ids!(scroll_down)).clicked(actions);
    let up = root.button(cx, ids!(scroll_up)).clicked(actions);
    if down || up {
        let scroll = {
            let mut offset = SCROLL.lock().unwrap();
            *offset = if down { *offset + 2.0 } else { (*offset - 2.0).max(0.0) };
            *offset
        };
        let bar = root.toolbar(cx, ids!(lifting));
        let state = if bar.set_raised_for_scroll(cx, scroll) {
            "raised"
        } else {
            "flat"
        };
        root.label(cx, ids!(scroll_note))
            .set_text(cx, &format!("{scroll:.0} points scrolled \u{2014} {state}"));
    }
    // The slots are children under their own names, so a button dropped into
    // one is reachable by the id it was written with.
    if root.button(cx, ids!(bar_share)).clicked(actions) {
        let n = crate::stories::bump(live_id!(toolbar_share));
        root.label(cx, ids!(bar_note))
            .set_text(cx, &format!("shared {n} times"));
    }
}

pub const STORIES: &[Story] = &[
    Story {
        key: "navigation/toolbar/overview",
        category: "Navigation",
        component: "Toolbar",
        also: &["AppBar", "BottomAppBar", "ToolbarEdge"],
        name: "Overview",
        dsl: "ToolbarOverview",
        added: "2026-09-10",
        tags: &["new", "bar", "app bar", "top bar", "bottom bar", "actions", "title"],
        doc: "# Toolbar\n\nA row of controls with two fixed ends and a middle that takes what is left.\n\nEvery bar in an application is the same three columns: `leading`, the middle, and `trailing`. Written by hand the middle is where it goes wrong — a `Fill` child in a row takes the **whole** row, so the trailing actions are laid out into nothing and never painted, silently, because a widget with no width is still a widget. Here the middle is a deferred fill: reserved before the trailing slot is drawn and sized after it, which is the only arrangement where both ends are certain to get their room.\n\n## Which page\n\nThis folder holds every bar in the library. **Toolbar**, this page, is a row of controls along the edge of a window or a panel. **Page header** is the same idea as a block at the top of a page: a trail, a title, a description, and the page's actions beside them. **Window chrome** is the menu bar and the caption buttons, which belong to the window rather than to anything in it. **Drop controls** are chips small enough for a crowded bar that keep their dragging in a popover, where it cannot be mistaken for dragging the window.\n\n## The bars are one bar\n\n`AppBar` is this row at the top of a window: a title, a navigation control before it, actions after it, and a rule along the bottom edge. `BottomAppBar` is the same row at the bottom, with the rule on the top edge and a `floating` slot past the actions for the one action the screen exists for. Neither is a separate widget — the day two of them disagree about how tall a bar is is the day one application looks like two.\n\n## The slots\n\n| Slot | What goes in it |\n|---|---|\n| `leading` | a back control, a menu button, a mark |\n| `center` | whatever wants the room the ends leave — a search box |\n| `trailing` | the bar's actions |\n| `floating` | past the actions: the one action the screen is for |\n\nA slot takes a **value** — `leading: Button{...}`. Writing `leading := Button{}` makes a named child and leaves the slot empty. A slot sizes itself, so a `Fill` widget dropped into one has nothing to fill; wrap several controls in a `Fit` row instead.\n\n`title` is drawn in the middle only when `center` is empty. The two would have to share a width neither can be measured against, and a title quietly overrunning the box beside it is worse than a title that stands down.\n\n## Flat, and raised\n\nA raised bar changes its own **surface** and casts no shadow: a shadow would have to be drawn over content the bar neither owns nor can redraw. Nothing here watches the scroll — a bar cannot find the view underneath it — so the host reads its own offset and calls `set_raised_for_scroll`, which switches on two thresholds rather than one. `lift_at` is where a flat bar lifts and `settle_at`, below it, is where a raised one settles. Content does not come to rest on a number, and with one threshold the bar changes colour on every fraction the content drifts by.\n\n## What it does not do\n\nIt does not collapse, fold or scroll what is in it: a row too narrow for its controls overruns, because a bar that quietly drops controls drops them where nobody will look. It is not the application menu bar and it does not own the window's caption buttons; both are on Window chrome.",
        subject: "plain",
        feature: None,
        controls: &[
            Control { label: "Title", target: "subject", kind: ControlKind::Text { prop: "title", default: "Settings" } },
            Control { label: "Middle align", target: "subject", kind: ControlKind::Number { prop: "middle_align", min: 0., max: 1., step: 0.5, default: 0. } },
            Control { label: "Bar height", target: "subject", kind: ControlKind::Number { prop: "bar_height", min: 32., max: 96., step: 2., default: 56. } },
            Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "gap", min: 0., max: 40., step: 2., default: 6. } },
            Control { label: "Lift at", target: "subject", kind: ControlKind::Number { prop: "lift_at", min: 0., max: 80., step: 2., default: 8. } },
            Control { label: "Settle at", target: "subject", kind: ControlKind::Number { prop: "settle_at", min: 0., max: 80., step: 2., default: 2. } },
        ],
        on_actions: Some(toolbar_actions),
    },
    
    Story {
        key: "navigation/toolbar/page-header",
        category: "Navigation",
        component: "Toolbar",
        also: &["PageHeader"],
        name: "Page header",
        dsl: "PageHeaderOverview",
        added: "2026-09-10",
        tags: &["new", "header", "page title", "subtitle", "breadcrumb"],
        doc: "# PageHeader\n\nThe block at the top of a page: a trail above, a title with an optional subtitle and a description, and the page's actions beside the words rather than under them.\n\n`title`, `subtitle` and `description` are three properties, not one block of text, because they are three different sizes of ink and a part that is not there must cost no room — an empty subtitle leaves no gap where a subtitle would have been.\n\n`breadcrumb` and `actions` are slots. The trail is a `Breadcrumb` by convention and anything at all by contract; the actions are a `Fit` row, because a slot sizes itself.\n\nThe words are the same deferred fill the toolbar's middle is: taken in order they would have the whole row and the actions would land past the page's edge, so the actions are drawn first and the column the words wrap to is what those actions left. That is the difference between a description that wraps to the page and one that wraps to a width somebody guessed at.\n\nIt has no face of its own — the ground is transparent — so it takes the colour of whatever page it is dropped on.",
        subject: "header",
        feature: None,
        controls: &[
            Control { label: "Title", target: "header", kind: ControlKind::Text { prop: "title", default: "Quarterly report" } },
            Control {
                label: "Subtitle",
                target: "header",
                kind: ControlKind::Text { prop: "subtitle", default: "Updated 10 minutes ago by the finance team" },
            },
            Control { label: "Gap", target: "header", kind: ControlKind::Number { prop: "gap", min: 0., max: 48., step: 1., default: 9. } },
            Control { label: "Text spacing", target: "header", kind: ControlKind::Number { prop: "text_spacing", min: 0., max: 24., step: 1., default: 3. } },
        ],
        on_actions: None,
    },
];
