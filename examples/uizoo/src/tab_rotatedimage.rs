use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoRotatedImage = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# RotatedImage\n\nRotatedImage displays rotated images."}
        }
        demos +: {
            H4{text: "RotatedImage"}
            P{text: "RotatedImage widget is not available in the new widget system."}
        }
    }
}
