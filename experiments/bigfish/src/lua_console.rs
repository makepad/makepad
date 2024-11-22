use crate::makepad_widgets::*;
//use rlua::{Function, Lua, MetaMethod, Result, UserData, UserDataMethods, Variadic};

live_design! {
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_widgets::base::*;
    use makepad_draw::shader::std::*;
    use crate::fish_theme::*;
    use crate::block_header_button::*;
    use crate::block_delete_button::*;

    LuaConsole = <View>
    {
        margin: 0
        width: 200
        height: Fit
        flow: Down
        optimize: DrawList

        title = <View>
        {
            show_bg: true
            flow: Down
            width: Fill
            height: Fit
            padding: 0
            draw_bg:
            {
                fn pixel(self) -> vec4
                {
                    return mix(vec4(1,1,0.6,1), vec4(1,1,0.5,1),self.pos.y);
                }
            },
            topbar = <View>
            {
                flow:Right,
                height: Fit,
                header = <BlockHeaderButton>
                {
                    text:"Lua Console"
                    draw_text:
                    {
                        color: #0
                        text_style: <H2_TEXT_BOLD> {}
                    }
                }

            }
        }
        body = <View>
        {
            show_bg: true
            width: Fill
            height: Fit
            flow: Down
            padding: {left: 30, right: 30, top: 4, bottom: 4}

            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1,1,0.9,1), vec4(1,1,0.8,1),self.pos.y);
                }
            }



            outputtext = <Label>
            {
                draw_text:
                {
                    fn pixel(self) -> vec4
                    {
                        return vec4(0,0,0,1);
                    }
                }
            }

             inputtext = <TextInput>
            {

            }

        }
    }


}
