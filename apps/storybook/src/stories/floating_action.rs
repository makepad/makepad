//! The floating action stories: the one action a screen is for and the few
//! behind it, the same button laid out in a bar or pinned to the whole
//! window, and the eight places it can be pinned with a set of any size.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryActionCountBase = #(StoryActionCount::register_widget(vm))
    /** A surface holding one floating action that shows only the first
     * `count` of the actions declared on it. */
    mod.storybook.StoryActionCount = set_type_default() do mod.storybook.StoryActionCountBase{
        width: Fit
        height: Fit
    }

    // A phone-sized surface: the button pins to this, not to the page.
    let Screen = RoundedView{
        width: 360.
        height: 520.
        flow: Overlay
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    // One cell of the anchor grid: a small surface a button pins to.
    let Cell = RoundedView{
        width: 220.
        height: 180.
        flow: Overlay
        show_bg: true
        draw_bg +: {color: theme.color_surface_container_low}
    }

    // Three short actions, the same in every cell, so what differs between
    // the cells is only where the button is.
    let CellAction = FloatingActionItem{}

    mod.stories.FloatingActionOverview = StoryPage{
        StoryNote{text: "The one action a screen exists for, as a round button pinned to a corner or an edge of its container. When that action is really a small set, pressing the button brings the set out toward the middle of the screen, and the plus on the button turns into a cross, so the place that brought the set out also puts it away."}

        StoryHeading{text: "One action, and the few behind it"}
        StoryNote{text: "The button sits on the screen, not on the list, so it stays put while the list scrolls under it. Press it and the plus turns into a cross while the actions come out toward the middle, one after another. Press and drag to an action and let go, and that one is picked in a single gesture. A press anywhere else puts the set away and does nothing else."}
        StoryRow{
            Screen{
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    scroll_bars: ScrollBars{
                        show_scroll_x: false
                        show_scroll_y: true
                        scroll_bar_y.drag_scrolling: true
                    }
                    ListItem{text: "Quarterly figures" secondary: "The draft is ready for a look"}
                    ListItem{text: "Garden plans" secondary: "Three layouts to choose from"}
                    ListItem{text: "Kitchen tiles" secondary: "Samples arrive on Tuesday"}
                    ListItem{text: "Reading list" secondary: "Twelve books, four started"}
                    ListItem{text: "Trip notes" secondary: "Trains, rooms and a map"}
                    ListItem{text: "Recipes" secondary: "The soup that worked"}
                    ListItem{text: "Budget" secondary: "Numbers for the spring"}
                    ListItem{text: "Birthday ideas" secondary: "Nothing decided yet"}
                    ListItem{text: "Repairs" secondary: "The gate and the gutter"}
                    ListItem{text: "Meeting notes" secondary: "Who said what, roughly"}
                    ListItem{text: "Photos to sort" secondary: "Two hundred from the coast"}
                    ListItem{text: "Letters" secondary: "Two to write back"}
                    ListItem{text: "Running log" secondary: "Five weeks in a row"}
                    ListItem{text: "Seeds" secondary: "Sow after the last frost"}
                    ListItem{text: "Insurance" secondary: "Renews next month"}
                    ListItem{text: "Film list" secondary: "Watched three of ten"}
                    ListItem{text: "Bike service" secondary: "Chain and brakes"}
                    ListItem{text: "Lessons" secondary: "Thursday at six"}
                    ListItem{text: "Paint colours" secondary: "Warm grey or cool white"}
                    ListItem{text: "Library" secondary: "Two books due"}
                    ListItem{text: "Taxes" secondary: "Receipts in the blue folder"}
                    ListItem{text: "Music practice" secondary: "Scales, then the slow piece"}
                    ListItem{text: "Moving boxes" secondary: "Labelled by room"}
                    ListItem{text: "Plants" secondary: "Water the fern on Sundays"}
                    ListItem{text: "Car" secondary: "Tyres before winter"}
                    ListItem{text: "Neighbours" secondary: "Return the ladder"}
                    ListItem{text: "Holiday dates" secondary: "Two weeks in August"}
                    ListItem{text: "Shelves" secondary: "Measure the alcove"}
                    ListItem{text: "Old phone" secondary: "Wipe it and give it away"}
                    ListItem{text: "Ideas" secondary: "Anything that does not fit elsewhere"}
                }
                subject := FloatingAction{
                    anchor: FloatingAnchor.BottomRight
                    dial: SpeedDialLayout.Vertical
                    star := FloatingActionItem{
                        label: "Favourite"
                        button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
                    }
                    share := FloatingActionItem{
                        label: "Share"
                        button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}
                    }
                    done := FloatingActionItem{
                        label: "Mark done"
                        button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}
                    }
                }
            }
        }

        StoryHeading{text: "What came back"}
        StoryNote{text: "A pick reports the id the action was declared under, and nothing else."}
        StoryRow{
            picked := Label{text: "nothing picked yet"}
        }

        StoryHeading{text: "Motion"}
        StoryNote{text: "The actions come out one after another on the theme's emphasized decelerate curve and go back the other way round, the last out first, on its standard accelerate curve; the plus turns on the same curve at the same time. The Enter ease and Exit ease controls give the set any of the theme's eight easings, the curves Foundations > Motion plays side by side: pick one and open the set again, and lengthen the times to see it. On the spring each action swings past its place and back, but a press still lands on where it comes to rest. Opened again on its way back, every action turns round from where it has got to. With Reduced motion on, everything lands at once."}

        StoryHeading{text: "The keyboard"}
        StoryNote{text: "Tab to the button and press Return: the set comes out and the first action takes the focus. The arrows follow the direction the actions went, so on the column above Up moves out along it and Down comes back; on an arc they walk the actions in order, round the inner arc and then the outer one. Home and End go to the first and the last. Return picks, and Escape puts the set away and gives the focus back to the button."}

        StoryHeading{text: "In a bar"}
        StoryNote{text: "Laid out inside a bar, the button keeps the room the bar gives it, moves with the bar's own alignment, and still sends its actions up and inward. Only the actions float. Float lift raises the button within the bar, which is made tall enough here to raise it by the whole of the control's range, and the actions follow it on the next frame."}
        StoryRow{
            RoundedView{
                width: 480.
                height: 320.
                flow: Down
                show_bg: true
                draw_bg +: {color: theme.color_surface_container_low}
                View{
                    width: Fill
                    height: Fill
                    padding: theme.mspace_3
                    flow: Down
                    spacing: theme.space_2
                    H4{text: "Inbox"}
                    P{width: Fill text: "Nothing new since this morning. The bar below carries the screen's own action at its trailing end."}
                }
                bar := BottomAppBar{
                    width: Fill
                    // A bar keeps its floating action inside itself, so the
                    // lift needs room above a 56 point button: 20 either side.
                    bar_height: 96.
                    title: "Inbox"
                    leading: View{
                        width: Fit height: Fit flow: Right
                        spacing: theme.space_1
                        align: Align{x: 0.5 y: 0.5}
                        ButtonFlat{text: "Archive"}
                        ButtonFlat{text: "Search"}
                    }
                    // A slot takes a value, and a value has no id to look it up
                    // by, so the action stands in a view of its own inside it.
                    floating: View{
                        width: Fit
                        height: Fit
                        bar_action := FloatingAction{
                            inline: true
                            anchor: FloatingAnchor.BottomRight
                            dial: SpeedDialLayout.Vertical
                            compose := FloatingActionItem{
                                label: "Write"
                                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}
                            }
                            flag := FloatingActionItem{
                                label: "Flag"
                                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
                            }
                            tidy := FloatingActionItem{
                                label: "Mark all read"
                                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}
                            }
                        }
                    }
                }
            }
        }
        StoryRow{
            bar_picked := Label{text: "nothing picked yet"}
        }

        StoryHeading{text: "Pinned to the window"}
        StoryNote{text: "Pinned to the window rather than to a container, the button sits at the bottom of the window over everything and stays there however the page is scrolled. Drawn on top is not heard first, though: it takes a press only where nothing that hears events before it lies underneath. This one is declared inside the page, so it sits in the middle of the bottom edge, over the page rather than the panels beside it; in an application it is declared last in the window's body, where it hears every press first."}
        StoryRow{
            window_toggle := CheckBox{text: "Pin one to the window"}
        }
        window_action := FloatingAction{
            pin_to: FloatingPin.Window
            anchor: FloatingAnchor.BottomCenter
            visible: false
            up := FloatingActionItem{
                label: "Back to top"
                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}
            }
            keep := FloatingActionItem{
                label: "Keep this page"
                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}
            }
            seen := FloatingActionItem{
                label: "Mark as read"
                button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}
            }
        }

        StoryHeading{text: "Eight behind one button"}
        StoryNote{text: "A set is not always three. The arc grows before two actions would touch, and in a window too small for it the actions go on round a second arc further out, the inner one filled first. Only the chip of the action under the pointer shows, straight out past the arcs. Later is switched off and is drawn dimmed. The anchors page has a set whose size goes from one to twelve."}
        StoryRow{
            Screen{
                height: 400.
                eight := FloatingAction{
                    anchor: FloatingAnchor.BottomRight
                    dial: SpeedDialLayout.Radial
                    copy := FloatingActionItem{label: "Copy" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                    move_to := FloatingActionItem{label: "Move" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                    print := FloatingActionItem{label: "Print" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    mail := FloatingActionItem{label: "Send" button +: {draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}}
                    pin := FloatingActionItem{label: "Pin" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                    tag := FloatingActionItem{label: "Tag" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                    archive := FloatingActionItem{label: "Archive" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    later := FloatingActionItem{label: "Later" enabled: false button +: {draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}}
                }
            }
        }

        StoryHeading{text: "With nothing behind it"}
        StoryNote{text: "A floating action with no set is just the action: it reports a press and never turns."}
        StoryRow{
            Screen{
                height: 200.
                single := FloatingAction{
                    anchor: FloatingAnchor.BottomCenter
                }
            }
        }
    }

    mod.stories.FloatingActionAnchors = StoryPage{
        StoryNote{text: "Every set opens toward the middle of its container. From a corner it fans a quarter circle, from the middle of an edge a half circle. A row or a column that has no way to go inward along its own axis is centred on the button, one step in. These eight are pinned: out from the start and kept out, so all of them can be compared at once."}
        StoryRow{
            to_radial := Button{text: "Arc"}
            to_row := Button{text: "Row"}
            to_column := Button{text: "Column"}
        }
        View{
            width: Fit
            height: Fit
            flow: Down
            spacing: theme.space_2
            View{
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                Cell{
                    top_left := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.TopLeft
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
                Cell{
                    top_center := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.TopCenter
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
                Cell{
                    top_right := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.TopRight
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
            }
            View{
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                Cell{
                    center_left := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.CenterLeft
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
                Cell{
                    padding: theme.mspace_3
                    flow: Down
                    StoryNote{width: Fill text: "Each cell pins one button to one of eight places. The middle has no place of its own: a button in the middle of a screen covers exactly what the screen is for."}
                }
                Cell{
                    center_right := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.CenterRight
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
            }
            View{
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_2
                Cell{
                    bottom_left := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.BottomLeft
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
                Cell{
                    bottom_center := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.BottomCenter
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
                Cell{
                    bottom_right := FloatingActionSm{
                        pinned: true
                        anchor: FloatingAnchor.BottomRight
                        a := CellAction{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        b := CellAction{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        c := CellAction{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                    }
                }
            }
        }
        StoryRow{
            anchors_picked := Label{text: "nothing picked yet"}
        }

        StoryHeading{text: "Any number of actions"}
        StoryNote{text: "Twelve actions are declared on this set and Count shows the first so many; a hidden action leaves no gap. Pick an anchor and a layout, raise the count and make the window smaller. An arc that would leave the window goes on round a second arc further out, filling the inner one first, and a row or a column folds into a second line one step further in. Nothing scrolls and nothing shrinks. Once a line has folded, a chip shows only for the action under the pointer or with the focus, and it is carried out past the other lines so it covers none of them."}
        StoryRow{
            // By its full name: the glob at the top was taken before this
            // file declared it.
            count_host := mod.storybook.StoryActionCount{
                count: 8.
                // The counter only counts; the surface the set pins to is
                // an ordinary one inside it.
                RoundedView{
                    width: 520.
                    height: 420.
                    flow: Overlay
                    show_bg: true
                    draw_bg +: {color: theme.color_surface_container_low}
                    driven := FloatingAction{
                        pinned: true
                        anchor: FloatingAnchor.BottomRight
                        dial: SpeedDialLayout.Radial
                        one := FloatingActionItem{label: "Note" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        two := FloatingActionItem{label: "Photo" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        three := FloatingActionItem{label: "Link" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                        four := FloatingActionItem{label: "Keep" button +: {draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}}
                        five := FloatingActionItem{label: "Share" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        six := FloatingActionItem{label: "Done" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                        seven := FloatingActionItem{label: "Copy" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        eight := FloatingActionItem{label: "Move" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                        nine := FloatingActionItem{label: "Print" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_check.svg")}}}
                        ten := FloatingActionItem{label: "Send" button +: {draw_icon +: {svg: crate_resource("self:resources/Icon_Favorite.svg")}}}
                        eleven := FloatingActionItem{label: "Pin" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_star.svg")}}}
                        twelve := FloatingActionItem{label: "Tag" button +: {draw_icon +: {svg: crate_resource("self:resources/mark_arrow.svg")}}}
                    }
                }
            }
        }
    }
}

/// The twelve actions of the counted set, in the order they are declared.
const DRIVEN: [LiveId; 12] = [
    live_id!(one),
    live_id!(two),
    live_id!(three),
    live_id!(four),
    live_id!(five),
    live_id!(six),
    live_id!(seven),
    live_id!(eight),
    live_id!(nine),
    live_id!(ten),
    live_id!(eleven),
    live_id!(twelve),
];

/// A surface whose floating action shows only the first `count` of the
/// actions declared on it. The count is a property of the surface rather
/// than of the set because a set has no business knowing how many of its
/// actions a page wants to show; each action already knows whether it is
/// shown.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryActionCount {
    #[deref]
    view: View,
    /// How many of the declared actions the set shows.
    #[live(12.0)]
    count: f64,
    /// The count last handed to the actions.
    #[rust]
    shown: Option<usize>,
}

impl StoryActionCount {
    /// Show the first `count` actions and hide the rest, when that changed.
    fn apply_count(&mut self, cx: &mut Cx) {
        let count = self.count.round().max(0.0) as usize;
        if self.shown == Some(count) {
            return;
        }
        self.shown = Some(count);
        let set = self.view.widget(cx, ids!(driven));
        for (k, id) in DRIVEN.iter().enumerate() {
            set.widget(cx, &[*id]).set_visible(cx, k < count);
        }
    }
}

impl Widget for StoryActionCount {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Before the set is drawn, so it lays out the actions it now has.
        self.apply_count(cx);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// The words a pick is reported with on the page: the id, as declared.
fn id_word(id: LiveId) -> &'static str {
    [
        (live_id!(star), "star"),
        (live_id!(share), "share"),
        (live_id!(done), "done"),
        (live_id!(copy), "copy"),
        (live_id!(move_to), "move_to"),
        (live_id!(print), "print"),
        (live_id!(mail), "mail"),
        (live_id!(pin), "pin"),
        (live_id!(tag), "tag"),
        (live_id!(archive), "archive"),
        (live_id!(later), "later"),
        (live_id!(a), "a"),
        (live_id!(b), "b"),
        (live_id!(c), "c"),
        (live_id!(one), "one"),
        (live_id!(two), "two"),
        (live_id!(three), "three"),
        (live_id!(four), "four"),
        (live_id!(five), "five"),
        (live_id!(six), "six"),
        (live_id!(seven), "seven"),
        (live_id!(eight), "eight"),
        (live_id!(nine), "nine"),
        (live_id!(ten), "ten"),
        (live_id!(eleven), "eleven"),
        (live_id!(twelve), "twelve"),
        (live_id!(compose), "compose"),
        (live_id!(flag), "flag"),
        (live_id!(tidy), "tidy"),
        (live_id!(up), "up"),
        (live_id!(keep), "keep"),
        (live_id!(seen), "seen"),
    ]
    .into_iter()
    .find(|(known, _)| *known == id)
    .map(|(_, word)| word)
    .unwrap_or("?")
}

fn overview_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    for set in [ids!(subject), ids!(eight)] {
        if let Some(id) = root.floating_action(cx, set).picked(actions) {
            root.label(cx, ids!(picked)).set_text(cx, id_word(id));
        }
    }
    if root.floating_action(cx, ids!(single)).pressed(actions) {
        root.label(cx, ids!(picked)).set_text(cx, "the action itself");
    }
    for set in [ids!(bar_action), ids!(window_action)] {
        if let Some(id) = root.floating_action(cx, set).picked(actions) {
            root.label(cx, ids!(bar_picked)).set_text(cx, id_word(id));
        }
    }
    if let Some(on) = root.check_box(cx, ids!(window_toggle)).changed(actions) {
        root.widget(cx, ids!(window_action)).set_visible(cx, on);
    }
}

/// The eight cells of the anchor grid, by id.
const CELLS: [&[LiveId]; 8] = [
    ids!(top_left),
    ids!(top_center),
    ids!(top_right),
    ids!(center_left),
    ids!(center_right),
    ids!(bottom_left),
    ids!(bottom_center),
    ids!(bottom_right),
];

fn anchors_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let dial = if root.button(cx, ids!(to_radial)).clicked(actions) {
        Some(SpeedDialLayout::Radial)
    } else if root.button(cx, ids!(to_row)).clicked(actions) {
        Some(SpeedDialLayout::Horizontal)
    } else if root.button(cx, ids!(to_column)).clicked(actions) {
        Some(SpeedDialLayout::Vertical)
    } else {
        None
    };
    for cell in CELLS {
        let action = root.floating_action(cx, cell);
        if let Some(dial) = dial {
            action.set_dial(cx, dial);
        }
        if let Some(id) = action.picked(actions) {
            root.label(cx, ids!(anchors_picked)).set_text(cx, id_word(id));
        }
    }
    if let Some(id) = root.floating_action(cx, ids!(driven)).picked(actions) {
        root.label(cx, ids!(anchors_picked)).set_text(cx, id_word(id));
    }
}

const ALSO: &[&str] = &["ButtonFloating", "ButtonFloatingSm", "FloatingActionItem", "FloatingActionSm", "FloatingActionLg"];

/// The theme's easings by token, in the order Foundations > Motion plays
/// them. Written as the DSL names them, so the control applies the token
/// itself and follows the theme rather than a copy of its curves.
const EASES: &[&str] = &[
    "theme.motion_ease_standard",
    "theme.motion_ease_standard_decelerate",
    "theme.motion_ease_standard_accelerate",
    "theme.motion_ease_emphasized_decelerate",
    "theme.motion_ease_emphasized_accelerate",
    "theme.motion_ease_linear",
    "theme.motion_ease_spring",
    "theme.motion_ease_bounce",
];

const ANCHORS: &[&str] = &[
    "FloatingAnchor.TopLeft",
    "FloatingAnchor.TopCenter",
    "FloatingAnchor.TopRight",
    "FloatingAnchor.CenterLeft",
    "FloatingAnchor.CenterRight",
    "FloatingAnchor.BottomLeft",
    "FloatingAnchor.BottomCenter",
    "FloatingAnchor.BottomRight",
];

const LAYOUTS: &[&str] = &["SpeedDialLayout.Radial", "SpeedDialLayout.Horizontal", "SpeedDialLayout.Vertical"];

const LABELS: &[&str] = &["SpeedDialLabels.Auto", "SpeedDialLabels.Always", "SpeedDialLabels.Hot", "SpeedDialLabels.Never"];

pub const STORIES: &[Story] = &[
    Story {
        key: "actions/floating-action/overview",
        category: "Actions",
        component: "FloatingAction",
        also: ALSO,
        name: "Overview",
        dsl: "FloatingActionOverview",
        added: "2026-09-13",
        tags: &["new"],
        doc: "# FloatingAction

The one action a screen exists for, as a round button pinned to a corner or an edge of its container or of the window. When that action is really a small set, pressing the button brings the set out toward the middle of the screen: along an arc, a row or a column. Each action is a small round button with an optional label chip, and the plus on the button turns into a cross, so the place that brought the set out also puts it away.

`anchor` picks one of eight places, and the same choice decides which way the set opens: always into the container, away from the edges the button touches. `edge_margin` is read only on those edges. `pin_to` is the container the button is declared in or the whole window.

`dial` lays the set out. `Radial` fans a quarter circle from a corner and a half circle from an edge, and the arc grows before two actions would touch. `Horizontal` and `Vertical` run from the button inward along their own axis; one with no inward run on its axis is centred on the button, one step in.

A set can hold any number of actions, and none leaves the window. An arc that would leave it is already as wide as its anchor lets it open, so the actions go on round a second arc further out, the inner arc filled first; a row or a column folds into a second line one step further in. Nothing scrolls and nothing shrinks. Only a window too small for the whole set is overrun.

`labels` decides the chips. `Auto` shows every chip beside a column, where they line up, and only the hovered or focused one on an arc or a row, where they would run into each other. Chips go on the side facing the interior and are kept inside the window. On an arc they sit straight out past the outermost arc; once a row or a column has folded, only the aimed-at chip shows whatever `labels` asks for, and it is carried out past the other lines, or for lines centred on the button to the nearer side of the set, so it covers none of them. A chip the window's edge would push over its own action goes beside the action instead.

Actions are children declared `name := FloatingActionItem{label: \"...\"}`, in the order they come out; a pick reports the name. An action with `visible: false` is left out and the set closes up. With no children, or none shown, the button is just the action and reports a press.

The button opens the set on the press rather than the click, so pressing, dragging to an action and letting go picks it in one gesture. While the set is out it takes the pointer: a press outside puts it away and reaches nothing underneath. Escape and the window losing focus put it away too.

**Motion comes from the theme.** The actions come out over `enter_secs` along `enter_ease` (defaults `theme.motion_short_4` and `theme.motion_ease_emphasized_decelerate`) and go back over `exit_secs` along `exit_ease` (`theme.motion_short_3` and `theme.motion_ease_standard_accelerate`), the last out first. Each runs its own clock, `stagger_secs` after the one before, and the stagger shrinks for a large set so the last is out no more than 0.15 s after one action would be. The plus turns on the same eases and clock. Any of the theme's eight easings fits; Foundations > Motion plays them side by side. On a curve that overshoots, such as the spring, an action swings past its place and back, stopping at the window's edge rather than being cut by it, while presses and the arrow keys aim at where it comes to rest. Opened again on its way back, the set turns round from where each action has got to. `reduced_motion` lands everything at once.

Keyboard: Return or Space on the button opens the set and moves focus to the first action. The arrows follow the direction the actions went: along a row or a column, and across to the next line of a folded one; on an arc they walk the actions in order, the inner arc and then the outer. Home and End go to the ends, Return picks, Tab stays inside the set, and Escape puts it away with focus back on the button.

`inline: true` lays the button out in its parent's flow instead of pinning it, which is what a bar's floating slot wants: the button takes the room the bar gives it and moves with the bar's alignment, and `float_lift` raises it like anything else in that slot, within the bar's own height, so a bar meant to show a lift is given the height for it. `pin_to` and `edge_margin` are not read; `anchor` still picks the direction the set opens. The actions are laid out around where the button really ended up, which is known a frame after the bar is laid out, so after the bar moves the actions follow on the next frame.

`pin_to: FloatingPin.Window` pins to the window instead of the container, over everything, however the page is scrolled. Drawn on top is not heard first: a closed button takes a press only where nothing that hears events before it lies underneath, so a button pinned to the window is declared last in the window's body, and one pinned to a container is declared last in it.",
        subject: "subject",
        feature: None,
        controls: &[
            Control { label: "Anchor", target: "subject", kind: ControlKind::Choice { prop: "anchor", options: ANCHORS, default: 7 } },
            Control { label: "Layout", target: "subject", kind: ControlKind::Choice { prop: "dial", options: LAYOUTS, default: 2 } },
            Control { label: "Labels", target: "subject", kind: ControlKind::Choice { prop: "labels", options: LABELS, default: 0 } },
            Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "size", min: 40., max: 96., step: 4., default: 56. } },
            Control { label: "Item size", target: "subject", kind: ControlKind::Number { prop: "item_size", min: 24., max: 64., step: 2., default: 40. } },
            Control { label: "Gap", target: "subject", kind: ControlKind::Number { prop: "item_gap", min: 0., max: 32., step: 1., default: 12. } },
            Control { label: "Enter ease", target: "subject", kind: ControlKind::Choice { prop: "enter_ease", options: EASES, default: 3 } },
            Control { label: "Enter time", target: "subject", kind: ControlKind::Number { prop: "enter_secs", min: 0., max: 0.6, step: 0.01, default: 0.2 } },
            Control { label: "Exit ease", target: "subject", kind: ControlKind::Choice { prop: "exit_ease", options: EASES, default: 2 } },
            Control { label: "Exit time", target: "subject", kind: ControlKind::Number { prop: "exit_secs", min: 0., max: 0.6, step: 0.01, default: 0.15 } },
            Control { label: "Reduced motion", target: "subject", kind: ControlKind::Bool { prop: "reduced_motion", default: false } },
            Control { label: "Float lift", target: "bar", kind: ControlKind::Number { prop: "float_lift", min: 0., max: 20., step: 1., default: 0. } },
        ],
        on_actions: Some(overview_actions),
    },
    Story {
        key: "actions/floating-action/anchors",
        category: "Actions",
        component: "FloatingAction",
        also: ALSO,
        name: "Anchors and layouts",
        dsl: "FloatingActionAnchors",
        added: "2026-09-13",
        tags: &["new"],
        doc: "# Anchors and layouts

Eight containers, one button pinned in each, every set held out with `pinned: true` so all eight can be compared at once.

Every set opens toward the middle of its container. From a corner the arc is a quarter circle whose ends lie along the two edges; from the middle of an edge it is a half circle. A row from a corner or a side runs inward along the row; a row from the middle of the top or bottom edge has no inward run along its own axis, so it is centred on the button one step in. Columns work the same way turned a quarter.

The buttons above switch all eight between an arc, a row and a column. The chips follow: beside a column on its inner side, above or below a row, and straight out past each action on an arc, where only the hovered one shows.

A pinned set holds no pointer grab and ignores Escape and presses outside, so it can sit on a surface like any other control; a pick is still reported.

## Any number of actions

The set below declares twelve actions; Count shows the first so many by setting `visible` on the rest, and the set closes up around a hidden one. Anchor, Layout, Labels and Gap drive the same set. The layout keeps every action inside the window and no two actions' squares overlapping: an arc grows by the chord rule, and past it until no two squares meet, and once it would leave the window goes on round a second arc a step further out with the inner arc filled first, and a row or a column folds into lines a step further in, the lines from the middle of a side centred on the button. Make the window small to see both. On an arc the arrows walk the actions in order, round the inner arc and then the outer; on folded lines they move along a line and across to the next.",
        subject: "driven",
        feature: None,
        controls: &[
            Control { label: "Count", target: "count_host", kind: ControlKind::Number { prop: "count", min: 1., max: 12., step: 1., default: 8. } },
            Control { label: "Anchor", target: "driven", kind: ControlKind::Choice { prop: "anchor", options: ANCHORS, default: 7 } },
            Control { label: "Layout", target: "driven", kind: ControlKind::Choice { prop: "dial", options: LAYOUTS, default: 0 } },
            Control { label: "Labels", target: "driven", kind: ControlKind::Choice { prop: "labels", options: LABELS, default: 0 } },
            Control { label: "Gap", target: "driven", kind: ControlKind::Number { prop: "item_gap", min: 0., max: 32., step: 1., default: 12. } },
        ],
        on_actions: Some(anchors_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::id_path;

    fn load(cx: &mut Cx) {
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
        });
    }

    fn build(cx: &mut Cx, dsl: &str) -> WidgetRef {
        cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(dsl).into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The pages are markup the compiler never reads: the widgets they show
    /// are reached by name, the enum values by qualified name, and every
    /// action by an id the page's own handler looks up. Building each page
    /// is what turns a mistake in any of that into a failed test rather
    /// than an empty screen in the catalogue.
    #[test]
    fn every_page_builds_and_everything_it_addresses_is_there() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        load(&mut cx);
        let _ = makepad_platform::shader_error::take();
        let extra: [&[&str]; 2] = [
            &["picked", "single", "star", "eight", "later", "bar", "bar_action", "bar_picked", "window_toggle", "window_action", "compose"],
            &["anchors_picked", "to_radial", "to_row", "to_column", "count_host", "driven", "twelve"],
        ];
        assert_eq!(STORIES.len(), extra.len());
        for (story, extra) in STORIES.iter().zip(extra) {
            let page = cx.with_vm(|vm| {
                let stories = vm.module(id!(stories));
                let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
                assert!(value.as_object().is_some(), "no template {}", story.dsl);
                WidgetRef::script_from_value(vm, value)
            });
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert_eq!(makepad_platform::shader_error::take(), None, "a draw shader failed to compile on {}", story.key);
            for target in [story.subject]
                .into_iter()
                .chain(story.controls.iter().map(|control| control.target))
                .chain(extra.iter().copied())
                .filter(|target| !target.is_empty())
            {
                assert!(!page.widget(&cx, &id_path(target)).is_empty(), "{}: no widget at {target}", story.key);
            }
            assert!(story.key.starts_with("actions/floating-action/"), "{}", story.key);
            assert_eq!(story.category, "Actions");
            assert_eq!(story.added, "2026-09-13");
        }
    }

    /// What the pages say about each button is what the buttons hold: the
    /// overview's set has three actions and the next one eight, the bar's
    /// is laid out inline and the window's pinned to the window and hidden
    /// until asked for; the grid's eight are pinned at the anchor their id
    /// names with three each, and the counted set declares twelve.
    #[test]
    fn the_buttons_are_set_the_way_the_pages_say() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        load(&mut cx);
        let actions_of = |page: &WidgetRef, cx: &Cx, id: &str| -> usize {
            let widget = page.widget(cx, &id_path(id));
            let set = widget.borrow::<FloatingAction>().unwrap_or_else(|| panic!("{id} is not a floating action"));
            let mut count = 0;
            set.children(&mut |_, child| {
                if child.borrow::<FloatingActionItem>().is_some() {
                    count += 1;
                }
            });
            count
        };

        let overview = build(&mut cx, "FloatingActionOverview");
        {
            let subject = overview.widget(&cx, ids!(subject));
            let subject = subject.borrow::<FloatingAction>().expect("the subject is a floating action");
            assert_eq!(subject.anchor, FloatingAnchor::BottomRight);
            assert_eq!(subject.dial, SpeedDialLayout::Vertical);
            assert!(!subject.inline && !subject.pinned);
        }
        assert_eq!(actions_of(&overview, &cx, "subject"), 3, "one dial of three");
        assert_eq!(actions_of(&overview, &cx, "eight"), 8, "and one of eight");
        let later = overview.widget(&cx, ids!(later));
        assert!(!later.borrow::<FloatingActionItem>().expect("an action").enabled, "Later is switched off");
        {
            let inline = overview.widget(&cx, ids!(bar_action));
            assert!(inline.borrow::<FloatingAction>().expect("the bar's action").inline);
            let window = overview.widget(&cx, ids!(window_action));
            let window = window.borrow::<FloatingAction>().expect("the window's action");
            assert_eq!(window.pin_to, FloatingPin::Window);
            assert_eq!(window.anchor, FloatingAnchor::BottomCenter);
        }
        assert!(!overview.widget(&cx, ids!(window_action)).visible(), "hidden until the box is ticked");

        let anchors = build(&mut cx, "FloatingActionAnchors");
        for (cell, anchor) in CELLS.iter().zip([
            FloatingAnchor::TopLeft,
            FloatingAnchor::TopCenter,
            FloatingAnchor::TopRight,
            FloatingAnchor::CenterLeft,
            FloatingAnchor::CenterRight,
            FloatingAnchor::BottomLeft,
            FloatingAnchor::BottomCenter,
            FloatingAnchor::BottomRight,
        ]) {
            let widget = anchors.widget(&cx, cell);
            let action = widget.borrow::<FloatingAction>().unwrap_or_else(|| panic!("{cell:?} is not a floating action"));
            assert_eq!(action.anchor, anchor);
            assert!(action.pinned, "{anchor:?} is pinned");
            assert_eq!(action.size, 40.0, "{anchor:?} is the small preset");
        }
        assert_eq!(actions_of(&anchors, &cx, "bottom_right"), 3, "three per cell");
        assert_eq!(actions_of(&anchors, &cx, "driven"), 12);
    }

    /// Count shows the first so many of the counted set's actions and hides
    /// the rest, and a count raised again shows them again.
    #[test]
    fn count_shows_the_first_so_many() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        load(&mut cx);
        let anchors = build(&mut cx, "FloatingActionAnchors");
        let shown = |cx: &Cx| -> Vec<bool> { DRIVEN.iter().map(|id| anchors.widget(cx, &[*id]).visible()).collect() };
        let host = anchors.widget(&cx, ids!(count_host));
        for count in [8usize, 1, 12, 5] {
            {
                let mut inner = host.borrow_mut::<StoryActionCount>().expect("the host counts");
                inner.count = count as f64;
                inner.apply_count(&mut cx);
            }
            let want: Vec<bool> = (0..12).map(|k| k < count).collect();
            assert_eq!(shown(&cx), want, "count {count}");
        }
    }

    /// The Ease controls offer the theme's eight easings by token and start
    /// on the ones the set already has, so the panel never names a curve
    /// the set is not on; the time controls start on the set's own times.
    #[test]
    fn the_motion_controls_start_where_the_set_is() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        load(&mut cx);
        let overview = build(&mut cx, "FloatingActionOverview");
        let story = &STORIES[0];
        for (label, prop) in [("Enter ease", "enter_ease"), ("Exit ease", "exit_ease")] {
            let control = story.controls.iter().find(|control| control.label == label).unwrap_or_else(|| panic!("no {label} control"));
            let ControlKind::Choice { prop: written, options, default } = &control.kind else {
                panic!("{label} is a choice");
            };
            assert_eq!(*written, prop);
            assert_eq!(options.len(), 8, "every theme easing");
            assert!(options.iter().all(|option| option.starts_with("theme.motion_ease_")));
            let token = options[*default].trim_start_matches("theme.");
            let named = crate::stories::foundations::theme_ease(&mut cx, token);
            let set = overview.widget(&cx, &id_path(control.target));
            let set = set.borrow::<FloatingAction>().expect("the target is a floating action");
            let own = if prop == "enter_ease" { set.enter_ease } else { set.exit_ease };
            assert_eq!(own, named, "{label} starts on the set's own easing");
        }
        for (label, prop) in [("Enter time", "enter_secs"), ("Exit time", "exit_secs")] {
            let control = story.controls.iter().find(|control| control.label == label).unwrap_or_else(|| panic!("no {label} control"));
            let ControlKind::Number { prop: written, default, .. } = &control.kind else {
                panic!("{label} is a number");
            };
            assert_eq!(*written, prop);
            let set = overview.widget(&cx, ids!(subject));
            let set = set.borrow::<FloatingAction>().expect("the subject");
            let own = if prop == "enter_secs" { set.enter_secs } else { set.exit_secs };
            assert!((own - default).abs() < 1e-9, "{label} starts on the set's own time");
        }
    }
}
