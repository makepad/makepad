use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoDropdown = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# DropDown\n\nDropdowns allow selecting from a list of options."}
        }
        demos +: {
            H4{text: "Standard"}
            dropdown := DropDown{
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            }

            Hr{}
            H4{text: "Standard, disabled"}
            dropdown_disabled := DropDown{
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }

            Hr{}
            H4{text: "DropDownFlat"}
            dropdown_flat := DropDownFlat{
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            }

            Hr{}
            H4{text: "DropDownGradientX"}
            dropdown_gradient_x := DropDownGradientX{
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            }

            Hr{}
            H4{text: "DropDownGradientY"}
            dropdown_gradient_y := DropDownGradientY{
                labels: ["Value One" "Value Two" "Third" "Fourth Value" "Option E" "Hexagons"]
            }
        }
    }
}
