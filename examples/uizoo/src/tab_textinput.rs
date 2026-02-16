use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoTextInput = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# TextInput\n\nText inputs allow users to enter text."}
        }
        demos +: {
            H4{text: "TextInput"}
            UIZooRowH{
                simpletextinput := TextInput{}
                simpletextinput_outputbox := P{
                    text: "Output"
                }
            }

            Hr{}
            H4{text: "TextInput, Disabled"}
            TextInput{
                empty_text: "Inline Label"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }

            Hr{}
            H4{text: "TextInput Inline Label"}
            TextInput{empty_text: "Inline Label"}

            Hr{}
            H4{text: "TextInput with content"}
            TextInput{empty_text: "Some text"}

            Hr{}
            H4{text: "TextInputFlat"}
            TextInputFlat{empty_text: "Inline Label"}

            Hr{}
            H4{text: "TextInputGradientX"}
            TextInputGradientX{empty_text: "Inline Label"}

            Hr{}
            H4{text: "TextInputGradientY"}
            TextInputGradientY{empty_text: "Inline Label"}
        }
    }
}
