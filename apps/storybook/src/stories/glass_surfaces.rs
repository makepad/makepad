//! The glass surfaces gallery: the lensing backing and every preset of it,
//! over something worth bending.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let Cap = Label{draw_text +: {color: #ffffffcc}}

    mod.stories.GlassSurfacesOverview = StoryPage{
        StoryNote{text: "The lensing surface the glass family is built on, and the presets of it that each control uses. They all bend what is behind them, so they are shown over the same colourful ground — on a flat one they collapse to an outline."}

        StoryHeading{text: "The surfaces"}
        StoryNote{text: "LensSurface is the base every control's backing derives from. The rest are the same surface tuned for what sits on it: a button, a prominent button, a chip, an icon, an input, a radio."}
        // Each band is taller than the one it replaces by the 42 points
        // the shared stage's clearance costs over the old 9-point inset.
        GlassStage{
            height: 250.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.LensSurface{width: 110. height: 44.}
                    Cap{text: "LensSurface"}
                    mod.widgets.glass.ButtonSurface{width: 110. height: 44.}
                    Cap{text: "ButtonSurface"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.ProminentButtonSurface{width: 110. height: 44.}
                    Cap{text: "ProminentButtonSurface"}
                    mod.widgets.glass.ChipSurface{width: 90. height: 32.}
                    Cap{text: "ChipSurface"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.IconSurface{width: 44. height: 44.}
                    Cap{text: "IconSurface"}
                    mod.widgets.glass.InputSurface{width: 140. height: 36.}
                    Cap{text: "InputSurface"}
                    mod.widgets.glass.RadioSurface{width: 44. height: 26.}
                    Cap{text: "RadioSurface"}
                }
            }
        }

        StoryHeading{text: "The controls made from them"}
        StoryNote{text: "A lens button and a lens chip are those same surfaces with a padding and a centring — despite the names they are not buttons. They carry no text and no press: the label goes inside them, and if you want something that answers a click, that is GlassButton on the glass controls page."}
        GlassStage{
            height: 240.
            body +: {
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    // No text on any of these: they are surfaces with a
                    // padding and a centring, so the label goes inside.
                    mod.widgets.glass.LensButton{Cap{text: "LensButton"}}
                    mod.widgets.glass.LensButtonProminent{Cap{text: "LensButtonProminent"}}
                    mod.widgets.glass.LensChip{Cap{text: "LensChip"}}
                }
                mod.widgets.glass.ClearPanel{
                    width: Fill height: Fit
                    padding: theme.mspace_2
                    Cap{text: "ClearPanel"}
                }
                View{
                    width: Fill height: Fit flow: Right spacing: theme.space_2
                    align: Align{y: 0.5}
                    mod.widgets.glass.CutButton{text: "CutButton"}
                    mod.widgets.glass.ProminentButton{text: "ProminentButton"}
                    mod.widgets.glass.IconButton{text: "Icon"}
                    mod.widgets.glass.Body{text: "glass.Body"}
                    mod.widgets.glass.ButtonLabel{text: "glass.ButtonLabel"}
                }
                mod.widgets.glass.List{
                    width: Fill height: Fit
                    mod.widgets.glass.ListRow{Cap{text: "a glass list row"}}
                    mod.widgets.glass.ListRow{Cap{text: "and another"}}
                }
            }
        }

        StoryHeading{text: "The two rounded surfaces underneath"}
        StoryNote{text: "LensedRoundedView is the tuned preset of GaussRoundedView the whole family sits on, and GaussGradientRoundedView is the version that also lays a gradient over the blur."}
        rounded := GlassStage{
            height: 150.
            // The stage's slot flows Down, which is what a column of demos
            // wants; this band is one row, so it says so. The body already
            // centres what it holds on the cross axis.
            body +: {
                flow: Right
                LensedRoundedView{width: 150. height: 70.}
                Cap{text: "LensedRoundedView"}
                GaussGradientRoundedView{width: 150. height: 70.}
                Cap{text: "GaussGradientRoundedView"}
            }
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "containers/glasssurfaces/overview",
    category: "Containers",
    component: "GlassSurfaces",
    also: &[
        "LensSurface", "ButtonSurface", "ProminentButtonSurface", "ChipSurface",
        "IconSurface", "InputSurface", "RadioSurface",
        "LensButton", "LensButtonProminent", "LensChip", "ClearPanel",
        "List", "ListRow", "CutButton", "ProminentButton", "IconButton", "Body", "ButtonLabel",
        "LensedRoundedView", "GaussGradientRoundedView",
    ],
    name: "Surfaces",
    dsl: "GlassSurfacesOverview",
    added: "2026-02-12",
    tags: &[],
    doc: "# Glass surfaces

The lensing backing the glass family is built on, and every preset of it the library ships.

`GaussRoundedView` is the raw surface: it samples the scene behind itself through a chain of mip textures and blurs it. `LensedRoundedView` is the tuned preset the family actually sits on, and `GaussGradientRoundedView` lays a gradient over the blur as well.

`LensSurface` derives from that, and everything else derives from `LensSurface` — the same surface adjusted for what sits on top of it. `ButtonSurface` and `ProminentButtonSurface` for buttons, `ChipSurface` for a chip, `IconSurface` for a square icon target, `InputSurface` for a field, `RadioSurface` for a toggle. **`LensButton`, `LensButtonProminent` and `LensChip` are not buttons.** They are the same surfaces with a padding and a centring, and they carry neither a label nor a press — writing `text:` on one is rejected at runtime, where only the log can see it. Put the label inside. The thing that answers a click is `GlassButton`, on the glass controls page.

`ClearPanel` is the plain sheet, and `List` with `ListRow` are the family's own rows — note that these live under `mod.widgets.glass` and are *not* general-purpose list views, which is a mistake worth making only once.

**They are shown here over a coloured ground on purpose.** Every one of them draws what is behind it, so on a flat background the whole family collapses to a faint outline. If the page you are putting one on has nothing worth refracting, this is not the family you want — see the glass controls page for the same comparison made side by side.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// The page is built from the DSL, which the Rust compiler never reads,
    /// and a shader that fails to compile is not an error anywhere — the
    /// draw is skipped and the widget paints nothing. All three bands moved
    /// onto a shared stage that did not exist before, and the third one is
    /// a ROW where the stage's slot flows Down, so the flow override is
    /// checked as well as the build.
    #[test]
    fn the_page_builds_and_the_row_band_still_flows_right() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
        });
        let story = &STORIES[0];
        let page = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
            assert!(value.as_object().is_some(), "no template {}", story.dsl);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!page.is_empty(), "{} built no widget", story.key);
        assert_eq!(
            makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // The last band is one row of four. The stage hands out a slot that
        // flows Down, so a merge that silently failed would stack them.
        let stage = page.widget(&cx, &[live_id!(rounded)]);
        assert!(!stage.is_empty(), "the last band is named on the page");
        let slot = stage
            .borrow::<View>()
            .expect("a stage is a View")
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(body))
            .map(|(_, w)| w.clone())
            .expect("the stage has a body");
        assert!(
            matches!(
                slot.borrow::<View>().expect("the slot is a View").layout.flow,
                Flow::Right { .. }
            ),
            "the row band overrode the stage's Down flow"
        );
    }
}
