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

    let RedBox = Box{ draw_bg +: { color: #xD8483C } }
    let GreenBox = Box{ draw_bg +: { color: #x3CA83C } }
    let BlueBox = Box{ draw_bg +: { color: #x4A6ED8 } }

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

            Hr{}
            H4{text: "Align x of an under-filling Fill child"}
            // A `width: Fill{max}` child that caps below the row width leaves slack.
            // align.x must position that slack. These three should read left / center /
            // right; before the Flow::Right deferred-fill fix they all anchored left.
            Pbold{text: "flow: Right, align: {x: 0.},  child width: Fill{max: 120.}"}
            UIZooRowH{
                align: Align{x: 0. y: 0.5}
                RedBox{height: 50 width: Fill{max: 120.}
                    BoxLabel{text: "Fill{max: 120.}"}
                }
            }
            Pbold{text: "flow: Right, align: {x: 0.5}, child width: Fill{max: 120.}"}
            UIZooRowH{
                align: Align{x: 0.5 y: 0.5}
                GreenBox{height: 50 width: Fill{max: 120.}
                    BoxLabel{text: "Fill{max: 120.}"}
                }
            }
            Pbold{text: "flow: Right, align: {x: 1.0}, child width: Fill{max: 120.}"}
            UIZooRowH{
                align: Align{x: 1.0 y: 0.5}
                BlueBox{height: 50 width: Fill{max: 120.}
                    BoxLabel{text: "Fill{max: 120.}"}
                }
            }

            Hr{}
            H4{text: "Align y of an under-filling Fill child"}
            // The Flow::Down mirror: a `height: Fill{max}` child caps below the column
            // height, and align.y positions the slack. These read top / center / bottom.
            // The faint outer box is the full column; the colored box is the Fill child.
            UIZooRowH{
                height: 180.
                spacing: 20.
                Box{width: 140. height: Fill flow: Down align: Align{y: 0.}
                    RedBox{width: Fill height: Fill{max: 60.}
                        BoxLabel{text: "align y: 0."}
                    }
                }
                Box{width: 140. height: Fill flow: Down align: Align{y: 0.5}
                    GreenBox{width: Fill height: Fill{max: 60.}
                        BoxLabel{text: "align y: 0.5"}
                    }
                }
                Box{width: 140. height: Fill flow: Down align: Align{y: 1.0}
                    BlueBox{width: Fill height: Fill{max: 60.}
                        BoxLabel{text: "align y: 1.0"}
                    }
                }
            }
        }
    }
}
