use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Box = RoundedView{
        show_bg: true
        draw_bg +: {
            color: uniform(#x0F02)
            border_size: uniform(1.)
            border_radius: uniform(0.)
            border_color: uniform(#xfff8)
        }
        padding: 3.
        align: Align{x: 0.5 y: 0.5}
    }

    let BoxLabel = P{
        width: Fit
        align: Align{x: 0.5}
    }

    mod.widgets.DemoLayout = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Layout\n\nLayout demos show width, height, margin, padding, spacing, flow, and alignment."}
        }
        demos +: {
            H4{text: "Width & Height"}
            UIZooRowH{
                flow: Right
                height: 100.
                Box{
                    width: 100. height: 60.
                    BoxLabel{text: "width: 100.\nheight: 60"}
                }
                Box{
                    width: 100. height: Fill
                    BoxLabel{text: "width: 100.\nheight: Fill"}
                }
                Box{
                    width: 150. height: Fit
                    BoxLabel{text: "width: 150.\nheight: Fit"}
                }
            }

            Hr{}
            H4{text: "Margin"}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                flow: Right
                spacing: 0.
                Box{
                    width: Fit height: Fit
                    margin: 0.
                    BoxLabel{text: "margin: 0."}
                }
                Box{
                    width: Fit height: Fit
                    margin: 0.
                    BoxLabel{text: "margin: 0."}
                }
                Box{
                    width: Fit height: Fit
                    margin: 10.
                    BoxLabel{text: "margin: 10."}
                }
                Box{
                    width: Fit height: Fit
                    margin: Inset{top: 0. left: 40 right: 0 bottom: 0.}
                    BoxLabel{text: "margin: {left: 40}"}
                }
            }

            Hr{}
            H4{text: "Padding"}
            UIZooRowH{
                Box{
                    width: Fit height: Fit
                    padding: 20.
                    BoxLabel{text: "padding: 20."}
                }
                Box{
                    width: Fit height: Fit
                    padding: Inset{left: 40. right: 10.}
                    BoxLabel{text: "padding: {left: 40., right: 10.}"}
                }
            }

            Hr{}
            H4{text: "Spacing"}
            Pbold{text: "spacing: 10."}
            UIZooRowH{
                spacing: 10.
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "spacing: 30."}
            UIZooRowH{
                spacing: 30.
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
            }

            Hr{}
            H4{text: "Flow Direction"}
            Pbold{text: "flow: Right"}
            UIZooRowH{
                spacing: 10.
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "flow: Down"}
            UIZooRowH{
                flow: Down
                spacing: 10.
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
                Box{height: 50 width: 50.}
            }

            Hr{}
            H4{text: "Align"}
            Pbold{text: "align: {x: 0., y: 0.}"}
            UIZooRowH{
                align: Align{x: 0. y: 0.}
                Box{height: 100 width: 50.}
                Box{height: 20 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "align: {x: 0.0, y: 0.5}"}
            UIZooRowH{
                align: Align{x: 0.0 y: 0.5}
                Box{height: 100 width: 50.}
                Box{height: 20 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "align: {x: 0., y: 1.}"}
            UIZooRowH{
                align: Align{x: 0.0 y: 1.0}
                Box{height: 100 width: 50.}
                Box{height: 20 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "align: {x: 0.5, y: 0.}"}
            UIZooRowH{
                align: Align{x: 0.5 y: 0.}
                Box{height: 100 width: 50.}
                Box{height: 20 width: 50.}
                Box{height: 50 width: 50.}
            }
            Pbold{text: "align: {x: 1.0, y: 1.}"}
            UIZooRowH{
                align: Align{x: 1.0 y: 1.}
                Box{height: 100 width: 50.}
                Box{height: 20 width: 50.}
                Box{height: 50 width: 50.}
            }
        }
    }
}
