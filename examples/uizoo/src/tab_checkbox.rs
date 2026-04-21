use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoCheckBox = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# CheckBox\n\nCheckboxes allow toggling options on/off."}
        }
        demos +: {
            H4{text: "Checkbox"}
            CheckBox{text: "CheckBox"}

            Hr{}
            H4{text: "Checkbox, disabled"}
            CheckBox{
                text: "CheckBox"
                animator +: {
                    disabled: {
                        default: @on
                    }
                }
            }

            Hr{}
            H4{text: "CheckBoxFlat"}
            CheckBoxFlat{text: "CheckBoxFlat"}

            Hr{}
            H4{text: "Toggle"}
            UIZooRowH{
                Toggle{text: "Toggle"}
            }

            Hr{}
            H4{text: "ToggleFlat"}
            UIZooRowH{
                ToggleFlat{text: "ToggleFlat"}
            }

            Hr{}
            H4{text: "Output demo"}
            UIZooRowH{
                height: Fit
                flow: Right
                align: Align{x: 0.0 y: 0.5}
                simplecheckbox := CheckBox{text: "CheckBox"}
                simplecheckbox_output := Label{text: ""}
            }

            Hr{}
            H4{text: "Custom Checkbox"}
            UIZooRowH{
                CheckBoxCustom{
                    text: "CheckBoxCustom"
                    align: Align{x: 0. y: 0.5}
                    padding: Inset{top: 0. left: 0. bottom: 0. right: 0.}
                    margin: Inset{top: 0. left: 0. bottom: 0. right: 0.}

                    label_walk: Walk{
                        width: Fit height: Fit
                        margin: theme.mspace_h_1{left: 5.5}
                    }

                    draw_icon +: {
                        svg: crate_resource("self:resources/Icon_Favorite.svg")
                    }

                    icon_walk: Walk{
                        width: 13.0
                        height: Fit
                    }
                }
            }
        }
    }
}
