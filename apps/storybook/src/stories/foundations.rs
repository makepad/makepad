//! The foundations stories: the theme's tokens shown as what they are.
//!
//! The tables read `mod.theme` at draw time through the library's
//! reflection surface, so they list exactly what the running theme defines
//! and cannot drift from the theme files. The visual rows (the radius boxes,
//! the elevation cards, the type presets, the easing buttons) are DSL, since
//! text styles and easings are objects reflection does not reach. Status,
//! state, spacing and size put their tokens on real widgets above the table:
//! presence dots and placeholders, a matrix of controls held in each state,
//! cards and stacks at every step, and controls beside rulers.
use crate::makepad_widgets::animator::Ease;
use crate::makepad_widgets::file_tree::*;
use crate::makepad_widgets::makepad_script::trap::NoTrap;
use crate::makepad_widgets::reflect::{theme_values, ThemeVal};
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.TokenTableBase = #(TokenTable::register_widget(vm))
    /** Every theme token whose name starts with one of `prefixes`, in that order: a swatch or a bar, the name, the value and where the theme defines it. */
    mod.storybook.TokenTable = set_type_default() do mod.storybook.TokenTableBase{
        width: Fill
        height: Fit
        draw_swatch +: {
            color: #888
        }
        draw_text +: {
            text_style: theme.font_body_m
            color: theme.color_text
        }
        draw_meta +: {
            text_style: theme.font_body_s
            color: theme.color_text_meta
        }
    }

    mod.storybook.StillViewBase = #(StillView::register_widget(vm))
    /** A view that passes no event to anything inside it, so whatever state its children were built in is the state they keep. */
    mod.storybook.StillView = set_type_default() do mod.storybook.StillViewBase{
        width: Fill
        height: Fit
        flow: Down
    }

    mod.storybook.FoundationsFileTreeBase = #(FoundationsFileTree::register_widget(vm))
    /** A file tree of six fixed rows: two open folders and four files. */
    mod.storybook.FoundationsFileTree = set_type_default() do mod.storybook.FoundationsFileTreeBase{
        width: 240.
        height: 6. * theme.data_item_height
        file_tree: FileTree{}
    }

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    // One example over its caption.
    let Sample = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_2
        align: Align{x: 0.5 y: 0.0}
    }

    // The loaded content a placeholder stands in for: the same card as
    // PlaceholderCard, part for part, so the two can be compared side by side.
    let LoadedCard = RoundedView{
        width: 240.
        height: Fit
        flow: Down
        padding: theme.mspace_2
        spacing: theme.space_2
        show_bg: true
        draw_bg +: {
            color: theme.color_surface_container
            border_radius: theme.radius_m
        }
        Image{
            width: Fill
            height: 100.
            fit: ImageFit.CropToFill
            src: crate_resource("self:resources/photo_landscape.jpg")
            draw_bg +: {border_radius: theme.radius_m}
        }
        Pbold{width: Fill margin: 0. text: "Ridge route"}
        P{width: Fill margin: 0. text: "Nine kilometres, most of it above the treeline, and back along the lake."}
        Button{width: 96. height: theme.size_control_m text: "Details"}
    }

    // The loaded row a PlaceholderRow stands in for: an avatar beside two lines.
    let LoadedRow = View{
        width: 260.
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0 y: 0.5}
        Avatar{plate: 40. name: "Noor Haddad"}
        View{
            width: Fill
            height: Fit
            flow: Down
            spacing: theme.space_1
            Label{padding: 0. text: "Noor Haddad" draw_text +: {text_style: theme.font_label_l color: theme.color_on_surface}}
            Label{padding: 0. text: "Harbour office, second floor" draw_text +: {text_style: theme.font_body_s color: theme.color_on_surface_variant}}
        }
    }

    // A row of the state matrix: the control's name, then one cell per state.
    let StateLine = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0 y: 0.5}
    }

    let StateName = Label{
        width: 100.
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let StateHead = Label{
        draw_text +: {text_style: theme.font_bold{} color: theme.color_on_surface}
    }

    let StateCell = View{
        width: Fill
        height: Fit
        align: Align{x: 0.0 y: 0.5}
    }

    // A list row with a ground of its own, so its layer lies over a colour
    // rather than over nothing.
    let GroundedRow = ListItemOne{
        text: "Row"
        draw_bg +: {color: theme.color_surface_container_low}
    }

    // A column name and what that column shows.
    let LegendLine = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
    }

    let LegendName = Pbold{
        width: 100.
        margin: 0.
    }

    let LegendText = P{
        width: Fill
        margin: 0.
    }

    // Three bars in a card. Each instance sets padding and spacing to one step.
    let SpaceBar = RoundedView{
        width: 44.
        height: 8.
        draw_bg +: {color: theme.color_primary border_radius: theme.radius_xs}
    }

    let SpaceCard = RoundedView{
        width: Fit
        height: Fit
        flow: Down
        draw_bg +: {color: theme.color_surface_container_high border_radius: theme.radius_s}
        SpaceBar{}
        SpaceBar{}
        SpaceBar{width: 28.}
    }

    // A filled tag, so its edges show on every theme's ground.
    let StackTag = Tag{
        text: "tag"
        intent: Primary
        appearance: Filled
    }

    // A control and a bar as tall as the height it is built from, bottoms level.
    let Ruled = View{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_1
        align: Align{x: 0.0 y: 1.0}
    }

    // The same margin above and below as a button, so the bar and the face
    // start and end level.
    let Ruler = SolidView{
        width: 3.
        margin: theme.mspace_v_1
        draw_bg +: {color: theme.color_primary}
    }

    let Stripe = SolidView{
        width: Fill
        height: theme.data_item_height
        draw_bg +: {color: theme.color_primary}
    }

    let StripeGap = SolidView{
        width: Fill
        height: theme.data_item_height
        draw_bg +: {color: theme.color_surface_container_high}
    }


    mod.stories.FoundationsColorRoles = StoryPage{
        StoryNote{text: "The accent families and the four intents. Each has a base, the colour that reads on it, a container and the colour that reads on the container."}
        mod.storybook.TokenTable{prefixes: [
            "color_primary" "color_on_primary" "color_secondary" "color_on_secondary"
            "color_tertiary" "color_on_tertiary" "color_error" "color_on_error"
            "color_warning" "color_on_warning" "color_success" "color_on_success"
            "color_info" "color_on_info" "color_inverse_primary"
        ]}
    }

    mod.stories.FoundationsColorSurfaces = StoryPage{
        StoryNote{text: "The surface ladder, what reads on it, the outlines, the inverse pair and the scrim."}
        mod.storybook.TokenTable{prefixes: [
            "color_surface" "color_on_surface" "color_outline" "color_inverse_surface"
            "color_inverse_on_surface" "color_scrim" "color_elevation"
        ]}
    }

    mod.stories.FoundationsColorStatus = StoryPage{
        StoryNote{text: "The colours a status is drawn in: four for whether somebody is about, and two for content that has not arrived yet. A status never rests on colour alone, so every example here also carries a shape or a word."}

        StoryHeading{text: "Presence on a plate"}
        StoryNote{text: "The dot on an avatar gives every state a shape as well as a colour, so the state still reads for somebody who cannot tell the green from the amber, and in a screenshot turned grey."}
        StoryRow{
            spacing: theme.space_6
            align: Align{x: 0.0 y: 0.0}
            Sample{
                Avatar{plate: 48. name: "Mira Okafor" presence: Online}
                Caption{text: "online: a filled circle"}
            }
            Sample{
                Avatar{plate: 48. name: "Jun Park" presence: Away}
                Caption{text: "away: a triangle"}
            }
            Sample{
                Avatar{plate: 48. name: "Ada Sorensen" presence: Busy}
                Caption{text: "busy: a filled square"}
            }
            Sample{
                Avatar{plate: 48. name: "Tomas Ruiz" presence: Offline}
                Caption{text: "offline: a hollow ring"}
            }
        }

        StoryHeading{text: "Presence in a list"}
        StoryNote{text: "In a list the state is written out as well, so it can be read aloud, searched for and copied."}
        SolidView{
            width: 360.
            height: Fit
            flow: Down
            draw_bg +: {color: theme.color_surface_container_low}
            ListItemTwo{
                leading: Avatar{plate: 36. name: "Mira Okafor" presence: Online}
                text: "Mira Okafor"
                secondary: "Online"
                divider: ListItemDivider.Text
            }
            ListItemTwo{
                leading: Avatar{plate: 36. name: "Jun Park" presence: Away}
                text: "Jun Park"
                secondary: "Away since ten"
                divider: ListItemDivider.Text
            }
            ListItemTwo{
                leading: Avatar{plate: 36. name: "Ada Sorensen" presence: Busy}
                text: "Ada Sorensen"
                secondary: "Busy, in a meeting"
                divider: ListItemDivider.Text
            }
            ListItemTwo{
                leading: Avatar{plate: 36. name: "Tomas Ruiz" presence: Offline}
                text: "Tomas Ruiz"
                secondary: "Offline"
            }
        }

        StoryHeading{text: "Loading"}
        StoryNote{text: "A placeholder takes the shape of the content it stands for, so nothing on the page moves when that content arrives. Each one below sits beside what it stands for."}
        StoryRow{
            spacing: theme.space_6
            align: Align{x: 0.0 y: 0.0}
            PlaceholderCard{}
            LoadedCard{}
        }
        StoryRow{
            spacing: theme.space_6
            PlaceholderRow{width: 260.}
            LoadedRow{}
        }
        StoryNote{text: "The two colours, held still: the resting fill, and the highlight the moving band brings up."}
        StoryRow{
            spacing: theme.space_6
            align: Align{x: 0.0 y: 0.0}
            Sample{
                RoundedView{width: 120. height: 40. draw_bg +: {color: theme.color_placeholder border_radius: theme.radius_s}}
                Caption{text: "color_placeholder"}
            }
            Sample{
                RoundedView{width: 120. height: 40. draw_bg +: {color: theme.color_placeholder_hl border_radius: theme.radius_s}}
                Caption{text: "color_placeholder_hl"}
            }
        }

        mod.storybook.TokenTable{prefixes: ["color_presence_" "color_placeholder"]}
    }

    mod.stories.FoundationsColorPalette = StoryPage{
        StoryNote{text: "Every colour the theme defines, roles and legacy tokens alike, in the order the theme file declares them."}
        mod.storybook.TokenTable{prefixes: ["color_"]}
    }

    mod.stories.FoundationsTypeScale = StoryPage{
        StoryNote{text: "The nine presets built on the font size knob, so the whole scale moves with it."}
        StoryRow{flow: Down spacing: theme.space_1
            Label{text: "Title L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_l}}
            Label{text: "Title M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_m}}
            Label{text: "Title S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_title_s}}
            Label{text: "Body L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_l}}
            Label{text: "Body M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_m}}
            Label{text: "Body S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_body_s}}
            Label{text: "Label L: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_l}}
            Label{text: "Label M: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_m}}
            Label{text: "Label S: the quick brown fox jumps over the lazy dog" draw_text +: {text_style: theme.font_label_s}}
        }
        mod.storybook.TokenTable{prefixes: ["type_" "font_size_"]}
    }

    mod.stories.FoundationsSpacingScale = StoryPage{
        StoryNote{text: "The spacing ladder and the factor it is built from. The examples use one step at a time; in the table every bar is the token's length."}

        StoryHeading{text: "Padding and gap"}
        StoryNote{text: "The same card at every step. The step is both the room around the bars and the gap between them, so each card grows on every side as the ladder climbs."}
        StoryRow{
            spacing: theme.space_5
            align: Align{x: 0.0 y: 1.0}
            Sample{SpaceCard{padding: theme.space_1 spacing: theme.space_1} Caption{text: "space_1"}}
            Sample{SpaceCard{padding: theme.space_2 spacing: theme.space_2} Caption{text: "space_2"}}
            Sample{SpaceCard{padding: theme.space_3 spacing: theme.space_3} Caption{text: "space_3"}}
            Sample{SpaceCard{padding: theme.space_4 spacing: theme.space_4} Caption{text: "space_4"}}
            Sample{SpaceCard{padding: theme.space_5 spacing: theme.space_5} Caption{text: "space_5"}}
            Sample{SpaceCard{padding: theme.space_6 spacing: theme.space_6} Caption{text: "space_6"}}
        }

        StoryHeading{text: "Stacks"}
        StoryNote{text: "Three tags stacked at every step and lined up at the bottom. The tags are the same in every stack, so the staircase is made of the gaps alone."}
        StoryRow{
            spacing: theme.space_5
            align: Align{x: 0.0 y: 1.0}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_1 StackTag{} StackTag{} StackTag{}} Caption{text: "space_1"}}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_2 StackTag{} StackTag{} StackTag{}} Caption{text: "space_2"}}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_3 StackTag{} StackTag{} StackTag{}} Caption{text: "space_3"}}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_4 StackTag{} StackTag{} StackTag{}} Caption{text: "space_4"}}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_5 StackTag{} StackTag{} StackTag{}} Caption{text: "space_5"}}
            Sample{View{width: Fit height: Fit flow: Down spacing: theme.space_6 StackTag{} StackTag{} StackTag{}} Caption{text: "space_6"}}
        }

        mod.storybook.TokenTable{prefixes: ["space_"]}
    }

    mod.stories.FoundationsSizeScale = StoryPage{
        StoryNote{text: "Control heights, icon sizes, the touch target, the hairlines and the height of a data row. The examples put each token on something that uses it; in the table every bar is the token's length."}

        StoryHeading{text: "Control heights"}
        StoryNote{text: "The five button sizes, each beside a bar as tall as the height it is built from. The stock Button fits its label and comes out shorter than the small size, so the middle one here is given size_control_m."}
        StoryRow{
            spacing: theme.space_4
            align: Align{x: 0.0 y: 1.0}
            Sample{
                Ruled{ButtonXs{text: "XS"} Ruler{height: theme.size_control_s - theme.space_1}}
                Caption{text: "size_control_s - space_1"}
            }
            Sample{
                Ruled{ButtonSm{text: "S"} Ruler{height: theme.size_control_s}}
                Caption{text: "size_control_s"}
            }
            Sample{
                Ruled{Button{text: "M" height: theme.size_control_m} Ruler{height: theme.size_control_m}}
                Caption{text: "size_control_m"}
            }
            Sample{
                Ruled{ButtonLg{text: "L"} Ruler{height: theme.size_control_l}}
                Caption{text: "size_control_l"}
            }
            Sample{
                Ruled{ButtonXl{text: "XL"} Ruler{height: theme.size_control_l + theme.space_2}}
                Caption{text: "size_control_l + space_2"}
            }
        }

        StoryHeading{text: "Icons"}
        StoryNote{text: "One mark at the three icon sizes."}
        StoryRow{
            spacing: theme.space_6
            align: Align{x: 0.0 y: 1.0}
            Sample{
                Icon{size: theme.size_icon_s draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_on_surface}}
                Caption{text: "size_icon_s"}
            }
            Sample{
                Icon{size: theme.size_icon_m draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_on_surface}}
                Caption{text: "size_icon_m"}
            }
            Sample{
                Icon{size: theme.size_icon_l draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_on_surface}}
                Caption{text: "size_icon_l"}
            }
        }

        StoryHeading{text: "Touch target and hairline"}
        StoryNote{text: "A close button is 20 points across, smaller than a fingertip, so the frame around it shows the room size_touch_target asks for. The rule beside it is a Divider, drawn size_divider thick."}
        StoryRow{
            spacing: theme.space_6
            align: Align{x: 0.0 y: 1.0}
            Sample{
                RoundedView{
                    width: theme.size_touch_target
                    height: theme.size_touch_target
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {
                        color: #0000
                        border_size: 1.0
                        border_color: theme.color_outline
                        border_radius: theme.radius_s
                    }
                    CloseButton{}
                }
                Caption{text: "CloseButton in size_touch_target"}
            }
            Sample{
                Divider{width: 160.}
                Caption{text: "Divider at size_divider"}
            }
        }

        StoryHeading{text: "Data rows"}
        StoryNote{text: "A file tree's rows are data_item_height tall. The stripes on its left are that height too, one for each row, so every row lines up with a stripe."}
        StoryRow{
            spacing: 0.
            align: Align{x: 0.0 y: 0.0}
            View{
                width: 12.
                height: Fit
                flow: Down
                Stripe{}
                StripeGap{}
                Stripe{}
                StripeGap{}
                Stripe{}
                StripeGap{}
            }
            mod.storybook.FoundationsFileTree{}
        }

        mod.storybook.TokenTable{prefixes: ["size_" "data_"]}
    }

    mod.stories.FoundationsShapeRadius = StoryPage{
        StoryNote{text: "The corner ladder, from none to a full pill."}
        StoryRow{
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_none}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_xs}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_s}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_m}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_l}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_xl}}
            RoundedView{width: 64. height: 44. draw_bg +: {color: theme.color_primary_container border_radius: theme.radius_full}}
        }
        mod.storybook.TokenTable{prefixes: ["radius_"]}
    }

    mod.stories.FoundationsElevationLevels = StoryPage{
        StoryNote{text: "Five levels of lift. Each preset takes its blur, drop and shadow colour from the elevation tokens, so everything built on them agrees on how high it sits."}
        StoryRow{
            spacing: 28.
            padding: theme.mspace_3
            ElevatedView1{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "1"}}
            ElevatedView2{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "2"}}
            ElevatedView3{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "3"}}
            ElevatedView4{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "4"}}
            ElevatedView5{width: 88. height: 60. align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_surface_container border_radius: theme.radius_m} Label{text: "5"}}
        }
        mod.storybook.TokenTable{prefixes: ["elevation_" "color_elevation"]}
    }

    mod.stories.FoundationsStateLayers = StoryPage{
        StoryNote{text: "A state is shown by laying the content colour over a surface at a fixed opacity. The matrix holds three controls in every state, and the table under it lists the opacities."}

        StoryHeading{text: "Three controls in every state"}
        StoryNote{text: "Each cell is built in its state and kept there: the matrix passes no events to what is inside it, so a pointer over a cell cannot change it."}
        mod.storybook.StillView{
            spacing: theme.space_3
            StateLine{
                StateName{text: ""}
                StateCell{StateHead{text: "rest"}}
                StateCell{StateHead{text: "hover"}}
                StateCell{StateHead{text: "pressed"}}
                StateCell{StateHead{text: "focused"}}
                StateCell{StateHead{text: "dragged"}}
                StateCell{StateHead{text: "disabled"}}
            }
            StateLine{
                StateName{text: "ButtonOutline"}
                StateCell{ButtonOutline{text: "Save"}}
                StateCell{ButtonOutline{text: "Save" animator +: {hover: {default: @on}}}}
                StateCell{ButtonOutline{text: "Save" animator +: {hover: {default: @down}}}}
                StateCell{ButtonOutline{text: "Save" animator +: {focus: {default: @on}}}}
                StateCell{Caption{text: "none"}}
                StateCell{ButtonOutline{text: "Save" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Chip"}
                StateCell{Chip{text: "Chip"}}
                StateCell{Chip{text: "Chip" animator +: {hover: {default: @on}}}}
                StateCell{Chip{text: "Chip" animator +: {hover: {default: @down}}}}
                StateCell{Chip{text: "Chip" animator +: {hover: {default: @on}} draw_bg +: {hover_opacity: theme.state_focus_opacity}}}
                StateCell{Chip{text: "Chip" animator +: {hover: {default: @on}} draw_bg +: {hover_opacity: theme.state_drag_opacity}}}
                StateCell{Chip{text: "Chip" disabled: true}}
            }
            StateLine{
                StateName{text: "ListItem"}
                StateCell{GroundedRow{}}
                StateCell{GroundedRow{animator +: {hover: {default: @on}}}}
                StateCell{GroundedRow{animator +: {hover: {default: @down}}}}
                StateCell{GroundedRow{animator +: {hover: {default: @on}} draw_bg +: {hover_opacity: theme.state_focus_opacity}}}
                StateCell{GroundedRow{animator +: {hover: {default: @on}} draw_bg +: {hover_opacity: theme.state_drag_opacity}}}
                StateCell{GroundedRow{disabled: true}}
            }
        }
        StoryNote{text: "What each column shows:"}
        View{
            width: Fill
            height: Fit
            flow: Down
            spacing: theme.space_1
            LegendLine{LegendName{text: "rest"} LegendText{text: "No layer."}}
            LegendLine{LegendName{text: "hover"} LegendText{text: "The content colour over the face at state_hover_opacity."}}
            LegendLine{LegendName{text: "pressed"} LegendText{text: "The hover layer kept, with state_press_opacity added on top of it."}}
            LegendLine{LegendName{text: "focused"} LegendText{text: "The button draws its outline in the focus colour rather than a layer. The chip and the row have no focus layer of their own, so here their hover layer is set to state_focus_opacity."}}
            LegendLine{LegendName{text: "dragged"} LegendText{text: "A button is never dragged. The chip and the row show state_drag_opacity through their hover layer."}}
            LegendLine{LegendName{text: "disabled"} LegendText{text: "The button takes the theme's disabled colours. The chip and the row keep state_disabled_content_opacity of their ink and take no hover or press."}}
        }

        mod.storybook.TokenTable{prefixes: ["state_"]}
    }

    mod.stories.FoundationsMotionOverview = StoryPage{
        StoryNote{text: "Press an easing and the sixteen bars below run out from nothing, each over its own duration. The short ones are done before the long ones start to look like they are moving, which is the point of having sixteen of them. Spring goes past its length and comes back; bounce arrives, rebounds off it and settles without ever passing it. Hovering a button fades it over the long duration with the same easing, so the curve can be read twice."}
        StoryHeading{text: "Easings"}
        StoryRow{
            ease_standard := Button{text: "standard" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_standard_decelerate := Button{text: "standard decelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard_decelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_standard_accelerate := Button{text: "standard accelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_standard_accelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_emphasized_decelerate := Button{text: "emphasized decelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_emphasized_decelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
        }
        StoryRow{
            ease_emphasized_accelerate := Button{text: "emphasized accelerate" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_emphasized_accelerate apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_linear := Button{text: "linear" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_linear apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_spring := Button{text: "spring" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_spring apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
            ease_bounce := Button{text: "bounce" animator +: {hover: {on: AnimatorState{from: {all: Forward{duration: theme.motion_long_4}} ease: theme.motion_ease_bounce apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}}}}}
        }
        StoryHeading{text: "Durations"}
        // A wide bar column and a big scale: the bars are the thing on
        // this page that moves, so they are given room to move in. The
        // scale stops well short of the column because two of the
        // easings go PAST the value they are heading for, and a bar
        // clamped at the edge of its column would hide the overshoot
        // that is the whole point of them. Every other table leaves
        // both at their defaults.
        durations := mod.storybook.TokenTable{
            prefixes: ["motion_"]
            swatch_width: 300.
            bar_scale: 230.
        }
    }
}

/// One line of a table: what to show at the left, then the three texts.
struct Row {
    name: String,
    value: String,
    at: String,
    demo: Demo,
    /// How long this row's bar takes to run out, in seconds, when the
    /// table is asked to play. Only a duration token has one; every
    /// other row sits still.
    secs: Option<f64>,
}

enum Demo {
    /// A filled swatch.
    Color(u32),
    /// A bar this many points long.
    Bar(f64),
    /// The text colour at this opacity.
    Opacity(f64),
}

/// A run of the bars, from nothing to their full length.
struct Playing {
    /// When it started, on the same clock `Cx::seconds_since_app_start`
    /// hands out, so a frame that arrives late lands where it belongs
    /// rather than replaying from where the last one stopped.
    start: f64,
    ease: Ease,
    /// The longest row, so the run knows when every bar has arrived.
    until: f64,
    next: NextFrame,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TokenTable {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[live]
    draw_swatch: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_meta: DrawText,
    /// Only tokens whose name starts with one of these are listed, grouped in this order.
    #[live]
    prefixes: Vec<String>,
    /// Height of one line.
    #[live(30.0)]
    row_height: f64,
    /// Points per unit for a duration's bar. A duration is a fraction of
    /// a second, so it needs a big multiplier before it is a length worth
    /// looking at; every other bar is already in points and ignores this.
    #[live(60.0)]
    bar_scale: f64,
    /// Width of the swatch column.
    #[live(72.0)]
    swatch_width: f64,
    /// Width of the name column.
    #[live(260.0)]
    name_width: f64,
    /// Width of the value column.
    #[live(110.0)]
    value_width: f64,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    playing: Option<Playing>,
}

impl TokenTable {
    /// Run every duration bar out from nothing, each over its own
    /// duration, along this curve. Pressing again starts over.
    pub fn play(&mut self, cx: &mut Cx, ease: Ease) {
        if self.rows.is_empty() {
            self.rows = read_rows(cx, &self.prefixes, self.bar_scale);
        }
        let until = self.rows.iter().filter_map(|r| r.secs).fold(0.0, f64::max);
        if until <= 0.0 {
            return;
        }
        self.playing = Some(Playing {
            start: cx.seconds_since_app_start(),
            ease,
            until,
            next: cx.new_next_frame(),
        });
        self.area.redraw(cx);
    }

    /// How far along its own run this row's bar is, 0 to 1. One when
    /// nothing is playing, so a table at rest draws its bars full.
    fn reached(&self, row: &Row, now: f64) -> f64 {
        let (Some(play), Some(secs)) = (&self.playing, row.secs) else {
            return 1.0;
        };
        if secs <= 0.0 {
            return 1.0;
        }
        play.ease.map(((now - play.start) / secs).clamp(0.0, 1.0))
    }
}

impl TokenTableRef {
    pub fn play(&self, cx: &mut Cx, ease: Ease) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.play(cx, ease);
        }
    }
}

/// The easing a theme token names, read out of the running theme rather
/// than restated here. An easing is an object, so the reflection surface
/// that hands out the colours and the numbers does not carry it.
pub fn theme_ease(cx: &mut Cx, name: &str) -> Ease {
    let key = LiveId::from_str(name);
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        let value = vm.bx.heap.value(theme, key.into(), NoTrap);
        Ease::script_from_value(vm, value)
    })
}

fn read_rows(cx: &mut Cx, prefixes: &[String], bar_scale: f64) -> Vec<Row> {
    let mut rows: Vec<(usize, Row)> = Vec::new();
    for (name, _key, val, at) in theme_values(cx) {
        let Some(group) = prefixes.iter().position(|p| name.starts_with(p.as_str())) else {
            continue;
        };
        let (value, demo, secs) = match val {
            ThemeVal::Color(c) => (format!("#{c:08x}"), Demo::Color(c), None),
            ThemeVal::Num(v) => {
                // A duration is a length of TIME, so its bar is that time
                // scaled to points and it is also how long the bar takes
                // to get there. Everything else is already a length.
                let (demo, secs) = if name.starts_with("state_") {
                    (Demo::Opacity(v), None)
                } else if name.starts_with("motion_") {
                    (Demo::Bar(v * bar_scale), Some(v))
                } else {
                    (Demo::Bar(v), None)
                };
                (format!("{v}"), demo, secs)
            }
        };
        rows.push((group, Row { name, value, at, demo, secs }));
    }
    // Grouped by the prefix list, then by name, so a family reads base,
    // container, and the ladders count up.
    rows.sort_by(|(ga, a), (gb, b)| ga.cmp(gb).then_with(|| a.name.cmp(&b.name)));
    rows.into_iter().map(|(_, row)| row).collect()
}

fn color_vec(c: u32) -> Vec4f {
    Vec4f {
        x: ((c >> 24) & 0xFF) as f32 / 255.0,
        y: ((c >> 16) & 0xFF) as f32 / 255.0,
        z: ((c >> 8) & 0xFF) as f32 / 255.0,
        w: (c & 0xFF) as f32 / 255.0,
    }
}

impl Widget for TokenTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.rows.is_empty() {
            self.rows = read_rows(cx, &self.prefixes, self.bar_scale);
        }
        let now = cx.seconds_since_app_start();
        cx.begin_turtle(walk, Layout::flow_down());
        let h = self.row_height;
        let rows = std::mem::take(&mut self.rows);
        for row in &rows {
            let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(h)));
            let demo_top = rect.pos.y + 5.0;
            let demo_height = h - 10.0;
            match row.demo {
                Demo::Color(c) => {
                    self.draw_swatch.color = color_vec(c);
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top), size: dvec2(self.swatch_width, demo_height) },
                    );
                }
                Demo::Bar(len) => {
                    let mut c = self.draw_text.color;
                    c.w = 0.8;
                    self.draw_swatch.color = c;
                    // Floored at 2 so a bar part way out is still a bar
                    // and not a gap. A run therefore starts from a mark
                    // rather than from nothing, which also says which
                    // rows are about to move.
                    let len = (len * self.reached(row, now)).clamp(2.0, self.swatch_width);
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top + 4.0), size: dvec2(len, demo_height - 8.0) },
                    );
                }
                Demo::Opacity(a) => {
                    let mut c = self.draw_text.color;
                    c.w = a as f32;
                    self.draw_swatch.color = c;
                    self.draw_swatch.draw_abs(
                        cx,
                        Rect { pos: dvec2(rect.pos.x, demo_top), size: dvec2(self.swatch_width, demo_height) },
                    );
                }
            }
            let x = rect.pos.x + self.swatch_width + 12.0;
            let y = rect.pos.y + 6.0;
            self.draw_text.draw_abs(cx, dvec2(x, y), &row.name);
            self.draw_text.draw_abs(cx, dvec2(x + self.name_width, y), &row.value);
            self.draw_meta.draw_abs(cx, dvec2(x + self.name_width + self.value_width, y + 1.0), &row.at);
        }
        self.rows = rows;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // A theme switch arrives as a reload: read the tokens again.
        if let Event::LiveEdit = event {
            self.rows.clear();
        }
        // A run asks for the next frame until the longest bar has
        // arrived, then stops asking. Nothing here runs when nothing is
        // playing, so a table sitting on a page costs no frames.
        let Some(play) = &self.playing else {
            return;
        };
        if let Some(_) = play.next.is_event(event) {
            if cx.seconds_since_app_start() - play.start >= play.until {
                self.playing = None;
            } else if let Some(play) = &mut self.playing {
                play.next = cx.new_next_frame();
            }
            self.area.redraw(cx);
        }
    }
}

/// The seven easing buttons run the bars below them.
fn motion_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    const BUTTONS: &[(&[LiveId], &str)] = &[
        (ids!(ease_standard), "motion_ease_standard"),
        (ids!(ease_standard_decelerate), "motion_ease_standard_decelerate"),
        (ids!(ease_standard_accelerate), "motion_ease_standard_accelerate"),
        (ids!(ease_emphasized_decelerate), "motion_ease_emphasized_decelerate"),
        (ids!(ease_emphasized_accelerate), "motion_ease_emphasized_accelerate"),
        (ids!(ease_linear), "motion_ease_linear"),
        (ids!(ease_spring), "motion_ease_spring"),
        (ids!(ease_bounce), "motion_ease_bounce"),
    ];
    for (id, token) in BUTTONS {
        if root.button(cx, id).clicked(actions) {
            let ease = theme_ease(cx, token);
            root.token_table(cx, ids!(durations)).play(cx, ease);
            return;
        }
    }
}

/// A view that hands no event to its children. The state matrix builds each
/// control in the state it shows, and a pointer passing over one would play
/// that control's hover off and leave the cell showing rest.
#[derive(Script, ScriptHook, Widget)]
pub struct StillView {
    #[deref]
    view: View,
}

impl Widget for StillView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// A file tree with six rows that never change: two folders, both open, and
/// four files. A file tree is drawn by its host every pass rather than
/// declared, so the rows are written out here.
#[derive(Script, ScriptHook, Widget)]
pub struct FoundationsFileTree {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[find]
    #[live]
    pub file_tree: FileTree,
}

impl Widget for FoundationsFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            let tree = &mut self.file_tree;
            tree.set_folder_is_open(cx, live_id!(foundations_src), true, Animate::No);
            tree.set_folder_is_open(cx, live_id!(foundations_resources), true, Animate::No);
            if tree.begin_folder(cx, live_id!(foundations_src), "src").is_ok() {
                tree.file(cx, live_id!(foundations_main), "main.rs");
                tree.file(cx, live_id!(foundations_theme), "theme.rs");
                tree.end_folder();
            }
            if tree.begin_folder(cx, live_id!(foundations_resources), "resources").is_ok() {
                tree.file(cx, live_id!(foundations_photo), "photo.jpg");
                tree.end_folder();
            }
            tree.file(cx, live_id!(foundations_readme), "README.md");
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
    }
}

const fn story(
    key: &'static str,
    component: &'static str,
    name: &'static str,
    dsl: &'static str,
    doc: &'static str,
    also: &'static [&'static str],
) -> Story {
    Story {
        key,
        category: "Foundations",
        component,
        also,
        name,
        dsl,
        added: "2026-09-05",
        tags: &["tokens", "new"],
        doc,
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    }
}

/// In the order the pages are read: colour, type, spacing and size, then
/// shape, elevation, state and motion.
pub const STORIES: &[Story] = &[
    story(
        "foundations/colour/roles",
        "Colour",
        "Roles",
        "FoundationsColorRoles",
        "# Colour roles\n\nSeven accent families: primary, secondary, tertiary and the four intents. Each family is four tokens: the base, `color_on_<family>` for what reads on it, a container, and what reads on the container. The values are generated from the house seed by the rule in the token registry; a unit test regenerates them and diffs against the theme files.",
        &[],
    ),
    story(
        "foundations/colour/surfaces",
        "Colour",
        "Surfaces",
        "FoundationsColorSurfaces",
        "# Surfaces\n\nThe surface ladder sits on the opaque ladder the legacy tokens already define: `color_surface` is the app background, the containers step up or down from it. Outlines are translucent tints, surfaces never are. The scrim and the inverse pair are here too.",
        &[],
    ),
    story(
        "foundations/colour/status",
        "Colour",
        "Status",
        "FoundationsColorStatus",
        "# Status colours\n\nFour presence colours and two placeholder colours. A status is never shown by colour alone.\n\n**Presence.** The dot on an `Avatar` gives each state a shape as well as a colour: online is a filled circle, away a triangle, busy a filled square and offline a hollow ring. A list row says the state in words too, so it can be read aloud and searched for.\n\n**Loading.** `color_placeholder` is the resting fill of a placeholder and `color_placeholder_hl` is the highlight its moving band brings up. A placeholder takes the shape of the content it stands for, which is why the page shows `PlaceholderCard` and `PlaceholderRow` beside the card and the row they stand in for: when the content arrives, nothing around it moves.",
        &[],
    ),
    story(
        "foundations/colour/palette",
        "Colour",
        "Palette",
        "FoundationsColorPalette",
        "# Palette\n\nEvery colour token in the running theme, roles and legacy alike, in the order the theme file declares them. This is what the design overlay's palette strip reads.",
        &[],
    ),
    story(
        "foundations/type/scale",
        "Type",
        "Scale",
        "FoundationsTypeScale",
        "# Type scale\n\nTitle, body and label in three sizes each. The sizes are expressed on `font_size_base` and `font_size_contrast`, so the scale follows the theme's knob; `font_size_5` and `font_size_6` fill the gap between the headings and the paragraph size.",
        &[],
    ),
    story(
        "foundations/spacing/scale",
        "Spacing",
        "Scale",
        "FoundationsSpacingScale",
        "# Spacing\n\nSix steps built on `space_factor`: `space_1` to `space_3` are half, one and one and a half times it, and `space_4` to `space_6` are two, three and four times it. The insets (`mspace_*`) are objects and are not listed in the table.\n\nThe cards use one step as both their padding and the gap between their bars. The stacks put the same three tags at each step and line them up at the bottom, so two stacks differ only by their gaps.",
        &[],
    ),
    story(
        "foundations/size/scale",
        "Size",
        "Scale",
        "FoundationsSizeScale",
        "# Sizes\n\nThree control heights, three icon sizes, the touch target, the hairline widths and the measures of a data row.\n\n**Controls.** The button sizes are built on the control heights: `ButtonXs` is `size_control_s` less `space_1`, `ButtonSm` is `size_control_s`, `ButtonLg` is `size_control_l` and `ButtonXl` is `size_control_l` plus `space_2`. The stock `Button` fits its label and comes out shorter than `ButtonSm`, so the page gives it `size_control_m`.\n\n**Touch and hairlines.** `size_touch_target` is the least room anything a finger has to hit should be given; the page draws that room around a 20 point `CloseButton`. `Divider` draws its rule `size_divider` thick.\n\n**Data rows.** A `FileTree` row is `data_item_height` tall, and the stripes beside the tree are the same height.\n\nNothing in the library reads `size_icon_m`, `size_icon_l` or `size_touch_target` yet, so this page is the first place they are used.",
        &[],
    ),
    story(
        "foundations/shape/radius",
        "Shape",
        "Radius",
        "FoundationsShapeRadius",
        "# Radius\n\nSeven corner sizes from none to a full pill. Widgets take one of these rather than a number of their own.",
        &[],
    ),
    story(
        "foundations/elevation/levels",
        "Elevation",
        "Levels",
        "FoundationsElevationLevels",
        "# Elevation\n\nFive levels, each a blur radius, a vertical drop and a shadow colour. `ElevatedView1` to `ElevatedView5` apply them to a rounded shadow view.",
        &["ElevatedView1", "ElevatedView2", "ElevatedView3", "ElevatedView4", "ElevatedView5"],
    ),
    story(
        "foundations/state/layers",
        "State",
        "Layers",
        "FoundationsStateLayers",
        "# State layers\n\nA state is shown by laying the content colour over the surface at a fixed opacity. Hover and press are the two layers the controls draw: hover at `state_hover_opacity`, and a pressed control keeps its hover layer and adds `state_press_opacity` on top.\n\nThe matrix holds `ButtonOutline`, `Chip` and a `ListItem` in each state. A focused button draws its outline in the focus colour instead of adding a layer. The chip and the row have no focus or drag layer of their own, so the matrix sets their hover layer to `state_focus_opacity` and `state_drag_opacity` to show how much those weigh. Disabled takes the theme's disabled colours on the button and `state_disabled_content_opacity` on the chip and the row; the row's ground keeps `state_disabled_container_opacity` of itself.\n\nThe matrix passes no events to what is inside it, so a pointer over it cannot change a cell, and the design overlay cannot pick a control inside it.",
        &[],
    ),
    Story {
        on_actions: Some(motion_actions),
        ..story(
            "foundations/motion/overview",
            "Motion",
            "Overview",
            "FoundationsMotionOverview",
            "# Motion

Sixteen durations in four bands and seven easings.

An easing token is an `Ease` object, not a number, so the reflection surface that lists the theme's colours and numbers does not carry it and the table below has none of them. An animator state names one directly: `ease: theme.motion_ease_standard`, which is what the seven buttons do to their own hover.

Press one and the bars run out from nothing, each over its own duration and along that curve. Two of them do not simply arrive: `spring` overshoots its length and settles back onto it, and `bounce` rebounds off it two or three times without ever going past. The bar column is wider than the longest bar so there is somewhere for an overshoot to go; clamp the two together and the difference between those two curves disappears. They arrive at different times because they are different lengths of time, which is the only thing sixteen durations are for: `short_1` is over before `extra_long_4` has visibly started. The bar never goes to nothing, it floors at two points, so a row that is about to move still says where it is.",
            &[],
        )
    },
];
