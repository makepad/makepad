use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoRadioButton = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# RadioButton\n\nRadio buttons allow selecting one option from a group."}
        }
        demos +: {
            H4{text: "Default"}
            UIZooRowH{
                radios_demo_1 := View{
                    spacing: theme.space_2
                    width: Fit height: Fit
                    radio1 := RadioButton{text: "Option 1"}
                    radio2 := RadioButton{text: "Option 2"}
                    radio3 := RadioButton{text: "Option 3"}
                    radio4 := RadioButton{
                        text: "Option 4, disabled"
                        animator +: {
                            disabled: {
                                default: @on
                            }
                        }
                    }
                }
            }

            Hr{}
            H4{text: "RadioButtonFlat"}
            UIZooRowH{
                radios_demo_2 := View{
                    spacing: theme.space_2
                    width: Fit height: Fit
                    radio1 := RadioButtonFlat{text: "Option 1"}
                    radio2 := RadioButtonFlat{text: "Option 2"}
                    radio3 := RadioButtonFlat{text: "Option 3"}
                    radio4 := RadioButtonFlat{text: "Option 4"}
                }
            }

            Hr{}
            H4{text: "RadioButtonFlatter"}
            UIZooRowH{
                radios_demo_3 := View{
                    spacing: theme.space_2
                    width: Fit height: Fit
                    radio1 := RadioButtonFlatter{text: "Option 1"}
                    radio2 := RadioButtonFlatter{text: "Option 2"}
                    radio3 := RadioButtonFlatter{text: "Option 3"}
                    radio4 := RadioButtonFlatter{text: "Option 4"}
                }
            }

            Hr{}
            H4{text: "Button Group"}
            radios_demo_11 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                flow: Right
                radio1 := RadioButtonTab{text: "Option 1"}
                radio2 := RadioButtonTab{text: "Option 2"}
                radio3 := RadioButtonTab{text: "Option 3"}
                radio4 := RadioButtonTab{
                    text: "Option 4, disabled"
                    animator +: {
                        disabled: {
                            default: @on
                        }
                    }
                }
            }

            Hr{}
            H4{text: "Button Group Flat"}
            radios_demo_12 := View{
                spacing: theme.space_2
                width: Fit height: Fit
                flow: Right
                radio1 := RadioButtonTabFlat{text: "Option 1"}
                radio2 := RadioButtonTabFlat{text: "Option 2"}
                radio3 := RadioButtonTabFlat{text: "Option 3"}
                radio4 := RadioButtonTabFlat{text: "Option 4"}
            }
        }
    }
}
