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

            Hr{}
            H4{text: "Multiline TextInput (fixed height, scrollable)"}
            P{text: "A multiline text input with a fixed height. Try typing or pasting enough text to overflow, then scroll with mouse wheel or drag the scrollbar."}
            multiline_textinput := TextInput{
                empty_text: "Type multiple lines here..."
                is_multiline: true
                height: 150.0
                width: Fill
            }

            Hr{}
            H4{text: "Multiline TextInput (taller, with pre-filled content)"}
            P{text: "A taller multiline text input pre-filled with content that overflows."}
            multiline_prefilled := TextInput{
                empty_text: "Prefilled multiline"
                is_multiline: true
                height: 200.0
                width: Fill
                text: "Line 1: The quick brown fox jumps over the lazy dog.\nLine 2: Pack my box with five dozen liquor jugs.\nLine 3: How vexingly quick daft zebras jump!\nLine 4: The five boxing wizards jump quickly.\nLine 5: Sphinx of black quartz, judge my vow.\nLine 6: Two driven jocks help fax my big quiz.\nLine 7: Crazy Frederick bought many very exquisite opal jewels.\nLine 8: We promptly judged antique ivory buckles for the next prize.\nLine 9: A mad boxer shot a quick, gloved jab to the jaw of his dizzy opponent.\nLine 10: Jaded zombies acted quaintly but kept driving their oxen forward."
            }

            Hr{}
            H4{text: "Multiline TextInput (read-only)"}
            P{text: "A read-only multiline text input. You can scroll and select text, but cannot edit."}
            multiline_readonly := TextInput{
                empty_text: "Read-only multiline"
                is_multiline: true
                is_read_only: true
                height: 120.0
                width: Fill
                text: "This is a read-only multiline text input.\nYou can scroll through the content and select text,\nbut you cannot modify it.\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10"
            }

            Hr{}
            H4{text: "Multiline TextInput (Fit height with max bound)"}
            P{text: "A multiline text input with Fit height that caps at 100px. The widget grows with content until hitting the max, then scrolls."}
            TextInput{
                empty_text: "Type several lines to grow this TextInput, then scroll..."
                is_multiline: true
                height: Fit{max: FitBound.Abs(100)}
                width: Fill
            }

            Hr{}
            H4{text: "Multiline TextInput (Fit height with relative max bound)"}
            P{text: "A multiline text input with Fit height capped at 30% of the parent height. Uses FitBound.Rel with Base.Full."}
            TextInput{
                empty_text: "Type several lines to test relative max height..."
                is_multiline: true
                height: Fit{max: FitBound.Rel{base: Base.Full, factor: 0.3}}
                width: Fill
            }

            Hr{}
            H4{text: "Toggle Multiline"}
            P{text: "Use the checkbox to toggle this text input between single-line and multiline mode."}
            UIZooRowH{
                multiline_toggle := CheckBox{text: "Multiline"}
            }
            multiline_toggleable := TextInput{
                text: "Toggle me between single-line and multiline..."
                height: Fit
                width: Fill
            }
        }
    }
}
