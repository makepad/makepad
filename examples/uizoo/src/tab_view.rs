use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoView = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# View\n\nViews are the basic layout containers."}
        }
        demos +: {
            H4{text: "View"}
            View{
                width: Fit height: Fit
                padding: theme.mspace_2
                align: Align{x: 0.5 y: 0.5}
                Label{text: "View"}
            }

            Hr{}
            H4{text: "Style Templates"}
            UIZooRowH{
                flow: Right

                SolidView{
                    width: Fit height: Fit
                    padding: theme.mspace_2
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {color: uniform(theme.color_inset)}
                    Label{text: "SolidView"}
                }

                RoundedView{
                    width: Fit height: Fit
                    padding: theme.mspace_2
                    align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {
                        color: uniform(theme.color_inset)
                        border_radius: uniform(5.)
                        border_size: uniform(2.0)
                        border_color: uniform(#xF)
                    }
                    Label{text: "RoundedView"}
                }

                ScrollXYView{
                    width: 100 height: 100
                    padding: theme.mspace_2
                    align: Align{x: 0. y: 0.}
                    show_bg: true
                    draw_bg +: {
                        color: uniform(theme.color_inset)
                    }
                    View{
                        width: 400. height: 400.
                        flow: Down
                        show_bg: true
                        draw_bg +: {color: uniform(theme.color_inset)}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                        Label{text: "ScrollXYView ScrollXYView ScrollXYView"}
                    }
                }

                ScrollYView{
                    width: 100 height: 100
                    padding: theme.mspace_2
                    align: Align{x: 0. y: 0.}
                    show_bg: true
                    draw_bg +: {
                        color: uniform(theme.color_inset)
                    }
                    View{
                        width: 400. height: 400.
                        flow: Down
                        show_bg: true
                        draw_bg +: {color: uniform(theme.color_inset)}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                        Label{text: "ScrollYView ScrollYView ScrollYView"}
                    }
                }
            }

            Hr{}
            H4{text: "Special functions"}
            UIZooRowH{
                CachedView{
                    width: Fit height: Fit
                    padding: theme.mspace_2
                    align: Align{x: 0.5 y: 0.5}
                    View{
                        width: Fit height: Fit
                        show_bg: true
                        draw_bg +: {color: uniform(theme.color_inset)}
                        Label{text: "CachedView"}
                    }
                }
            }
        }
    }
}
