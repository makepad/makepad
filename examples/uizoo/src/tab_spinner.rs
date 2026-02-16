use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoSpinner = UIZooTabLayout_B{
        desc +: {}
        demos +: {
            H4{text: "Default"}
            LoadingSpinner{}
        }
    }
}
