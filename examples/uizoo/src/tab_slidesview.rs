use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoSlidesView = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# SlidesView\n\nSlidesView displays presentation slides."}
        }
        demos +: {
            SlidesView{
                width: Fill height: Fill

                SlideChapter{
                    title := H1{text: "Hey!"}
                    SlideBody{text: "This is the 1st slide. Use your right\ncursor key to show the next slide."}
                }

                Slide{
                    title := H1{text: "Second slide"}
                    SlideBody{text: "This is the 2nd slide. Use your left\ncursor key to show the previous slide."}
                }
            }
        }
    }
}
