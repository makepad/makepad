use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let SlugDemoCard = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {
            color: uniform(theme.color_inset)
            border_radius: uniform(theme.corner_radius)
            border_size: uniform(1.0)
            border_color: uniform(#fff2)
        }
    }

    let SlugDemoTitle = Pbold{
        width: Fill
        margin: 0.
    }

    let SlugDemoNote = P{
        width: Fill
        margin: 0.
    }

    let SlugProbeGrid = View{
        width: Fill
        height: Fit
        flow: Flow.Right{wrap: true}
        spacing: theme.space_2
    }

    let SlugProbeCard = RoundedView{
        width: 200.
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_2
        show_bg: true
        draw_bg +: {
            color: uniform(theme.color_inset_1)
            border_radius: uniform(theme.corner_radius)
            border_size: uniform(1.0)
            border_color: uniform(#fff1)
        }
    }

    let SlugLargeDefaultLabel = Label{
        draw_text +: {
            text_style +: {
                font_size: 160.
            }
        }
    }

    let SlugLargeLiteralLabel = Label{
        draw_text +: {
            color: #fff
            text_style +: {
                font_size: 160.
            }
        }
    }

    let SlugLargeThemeLabel = Label{
        draw_text +: {
            color: theme.color_text
            text_style +: {
                font_size: 160.
            }
        }
    }

    let SlugLargeAccentLabel = Label{
        draw_text +: {
            color: theme.color_makepad
            text_style +: {
                font_size: 160.
            }
        }
    }

    let SlugLargeGradientLabel = Label{
        draw_text +: {
            color: #x6CF
            color_2: #xFD6
            gradient_fill_horizontal: 1.0
            text_style +: {
                font_size: 160.
            }
        }
    }

    let SlugLargeCustomLiteralLabel = Label{
        draw_text +: {
            color: #xF75
            text_style +: {
                font_size: 160.
            }
            get_color: fn() -> vec4 {
                return mix(#xF75 #0000 self.pos.x)
            }
        }
    }

    let SlugLargeCustomThemeLabel = Label{
        draw_text +: {
            color: theme.color_makepad
            text_style +: {
                font_size: 160.
            }
            get_color: fn() -> vec4 {
                return mix(theme.color_makepad #0000 self.pos.x)
            }
        }
    }

    mod.widgets.DemoSlug = UIZooTabLayout_B{
        desc +: {
            Markdown{
                body: "# SLUG\n\nThese demos compare the same text widget below and above the current Linux SLUG cutoff.\n\nOnly the text size changes between the left and right columns. On Linux, the left column should stay on the raster/MSDF text path while the right column should switch to SLUG. On other platforms, both columns may already use SLUG.\n\nThe lower diagnostic section intentionally adds redundant large-text probes so we can tell whether Linux SLUG failures are tied to glyph shape, color source, or custom `get_color()` logic."
            }
        }
        demos +: {
            H4{text: "Plain Label"}
            P{text: "Same Label widget, same text, different font sizes."}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                SlugDemoCard{
                    SlugDemoTitle{text: "Below Linux cutoff"}
                    SlugDemoNote{text: "32 px Label text. Expected to stay on the normal text path on Linux."}
                    Label{
                        draw_text +: {
                            text_style +: {
                                font_size: 32.
                            }
                        }
                        text: "Ag"
                    }
                }
                SlugDemoCard{
                    SlugDemoTitle{text: "Above Linux cutoff"}
                    SlugDemoNote{text: "192 px Label text. Expected to use SLUG on Linux."}
                    Label{
                        draw_text +: {
                            text_style +: {
                                font_size: 192.
                            }
                        }
                        text: "Ag"
                    }
                }
            }

            Hr{}
            H4{text: "Gradient Label"}
            P{text: "Same gradient Label widget, so this is useful for checking that text styling still matches across both render paths."}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                SlugDemoCard{
                    SlugDemoTitle{text: "Gradient below cutoff"}
                    SlugDemoNote{text: "32 px LabelGradientX text."}
                    LabelGradientX{
                        draw_text +: {
                            color: #x6CF
                            color_2: #xFD6
                            text_style +: {
                                font_size: 32.
                            }
                        }
                        text: "SLUG"
                    }
                }
                SlugDemoCard{
                    SlugDemoTitle{text: "Gradient above cutoff"}
                    SlugDemoNote{text: "192 px LabelGradientX text."}
                    LabelGradientX{
                        draw_text +: {
                            color: #x6CF
                            color_2: #xFD6
                            text_style +: {
                                font_size: 192.
                            }
                        }
                        text: "SL"
                    }
                }
            }

            Hr{}
            H4{text: "Custom Text Shader"}
            P{text: "Both sides use the same Label shader override. This makes it easy to compare custom get_color logic across non-SLUG and SLUG rendering on Linux."}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                SlugDemoCard{
                    SlugDemoTitle{text: "Custom shader below cutoff"}
                    SlugDemoNote{text: "32 px Label with a custom get_color function."}
                    Label{
                        draw_text +: {
                            color: theme.color_makepad
                            text_style +: {
                                font_size: 32.
                            }
                            get_color: fn() -> vec4 {
                                return mix(theme.color_makepad #0000 self.pos.x)
                            }
                        }
                        text: "WAVE"
                    }
                }
                SlugDemoCard{
                    SlugDemoTitle{text: "Custom shader above cutoff"}
                    SlugDemoNote{text: "192 px Label with the same custom get_color function."}
                    Label{
                        draw_text +: {
                            color: theme.color_makepad
                            text_style +: {
                                font_size: 192.
                            }
                            get_color: fn() -> vec4 {
                                return mix(theme.color_makepad #0000 self.pos.x)
                            }
                        }
                        text: "W"
                    }
                }
            }

            Hr{}
            H4{text: "LinkLabel"}
            P{text: "This row uses the same interactive text widget at two sizes so you can compare the path switch on Linux with built-in link styling."}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                SlugDemoCard{
                    SlugDemoTitle{text: "LinkLabel below cutoff"}
                    SlugDemoNote{text: "28 px LinkLabel text."}
                    LinkLabel{
                        draw_text +: {
                            gradient_fill_horizontal: 1.0
                            color: #x8CF
                            color_2: #xFB8
                            text_style +: {
                                font_size: 28.
                            }
                        }
                        text: "Open docs"
                    }
                }
                SlugDemoCard{
                    SlugDemoTitle{text: "LinkLabel above cutoff"}
                    SlugDemoNote{text: "144 px LinkLabel text."}
                    LinkLabel{
                        draw_text +: {
                            gradient_fill_horizontal: 1.0
                            color: #x8CF
                            color_2: #xFB8
                            text_style +: {
                                font_size: 144.
                            }
                        }
                        text: "Go"
                    }
                }
            }

            Hr{}
            H4{text: "Diagnostic Matrix"}
            P{text: "These extra probes all stay above the Linux SLUG cutoff. They are meant to help isolate whether the disappearing text is tied to plain single-color Labels, inherited theme colors, custom get_color logic, or specific glyph shapes."}

            H4{text: "Color Source Probes"}
            P{text: "Same large Label widget, same text, different color sources. If only some of these disappear after SLUG promotion, that narrows the bug quickly."}
            SlugProbeGrid{
                SlugProbeCard{
                    SlugDemoTitle{text: "Default Label"}
                    SlugDemoNote{text: "No explicit color override."}
                    SlugLargeDefaultLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Literal White"}
                    SlugDemoNote{text: "Plain Label with color: #fff."}
                    SlugLargeLiteralLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Theme Text"}
                    SlugDemoNote{text: "Plain Label with theme.color_text."}
                    SlugLargeThemeLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Theme Accent"}
                    SlugDemoNote{text: "Plain Label with theme.color_makepad."}
                    SlugLargeAccentLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Plain Gradient"}
                    SlugDemoNote{text: "Plain Label using color and color_2."}
                    SlugLargeGradientLabel{text: "Ag"}
                }
            }

            Hr{}
            H4{text: "Glyph Shape Probes"}
            P{text: "All of these use the same large plain Label with a literal white color. This helps separate geometry-specific failures from color/state-specific failures."}
            SlugProbeGrid{
                SlugProbeCard{
                    SlugDemoTitle{text: "A"}
                    SlugDemoNote{text: "Uppercase with counters."}
                    SlugLargeLiteralLabel{text: "A"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "g"}
                    SlugDemoNote{text: "Lowercase descender."}
                    SlugLargeLiteralLabel{text: "g"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "W"}
                    SlugDemoNote{text: "Wide uppercase."}
                    SlugLargeLiteralLabel{text: "W"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "S"}
                    SlugDemoNote{text: "Curved single glyph."}
                    SlugLargeLiteralLabel{text: "S"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "L"}
                    SlugDemoNote{text: "Simple cornered glyph."}
                    SlugLargeLiteralLabel{text: "L"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "O"}
                    SlugDemoNote{text: "Closed loop counter."}
                    SlugLargeLiteralLabel{text: "O"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "8"}
                    SlugDemoNote{text: "Double counter."}
                    SlugLargeLiteralLabel{text: "8"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "y"}
                    SlugDemoNote{text: "Descender with simpler shape."}
                    SlugLargeLiteralLabel{text: "y"}
                }
            }

            Hr{}
            H4{text: "Custom Shader Probes"}
            P{text: "These all use the same basic fade-out get_color idea, but vary whether the base color is literal or theme-driven and whether the text is Ag or W."}
            SlugProbeGrid{
                SlugProbeCard{
                    SlugDemoTitle{text: "Custom Literal Ag"}
                    SlugDemoNote{text: "Literal base color, Ag text."}
                    SlugLargeCustomLiteralLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Custom Theme Ag"}
                    SlugDemoNote{text: "Theme base color, Ag text."}
                    SlugLargeCustomThemeLabel{text: "Ag"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Custom Literal W"}
                    SlugDemoNote{text: "Literal base color, W text."}
                    SlugLargeCustomLiteralLabel{text: "W"}
                }
                SlugProbeCard{
                    SlugDemoTitle{text: "Custom Theme W"}
                    SlugDemoNote{text: "Theme base color, W text."}
                    SlugLargeCustomThemeLabel{text: "W"}
                }
            }
        }
    }
}
