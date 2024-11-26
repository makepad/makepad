use makepad_widgets::*;

live_design!{
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_draw::shader::std::*;
    
    TEXT_BIG = 12.0
    TEXT_MONO = {
        font_size: 10.0,
        font: {path: dep("crate://makepad-widgets/resources/LiberationMono-Regular.ttf")}
    }
    COLOR_DOWN_2 = #x00000022
    
    Bubble =<TextInput> {
        ascii_only: true,
        width: Fill,
        height: Fill,
        margin: {top: 5.0, left: 50.0, bottom: .0, right: 0.0},
        text: ""
        draw_text: {
            text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}
        }
        draw_bg: {
            color: (#335)
        }
    }
    
    AppUI = <View dx:311.8 dy:124.8 dw:397.1 dh:300.0> {  
        flow: Overlay,
                            
                            
        width: Fill,
        height: Fill
        
        <View>{
            flow: Down
            <RoundedView>{
                draw_bg:{
                    color: (COLOR_DOWN_2)
                    border_width: 1.0
                    border_color: #x00000044
                }
                margin:{top:0, left:0, right: 0, bottom:5}
                align: {x:0.5},
                padding: 2
                width: Fill,
                height: Fill
                chat = <PortalList>{  
                    auto_tail:true,
                    width: Fill,
                    height: Fill,
                    margin: {top: 0},
                    Own = <Bubble>{margin:{left:0}}
                    Other = <Bubble>{margin:{left:100}}
                }
            }
            <View>{
                height: Fit
                name_input = <TextInput> {
                    height: Fit,
                    width: 100,
                    margin: {top: 0.0, left: 0.0, bottom: 0.0, right: 0.0},
                    margin: {bottom: 0}
                    empty_message: "Name"
                }
                message_input = <TextInput> {
                    height: Fit,
                    width: Fill,
                    margin: {top: 0.0, left: 0.0, bottom: 0.0, right: 0.0},
                    margin: {bottom: 0}
                    empty_message: "Talk here"
                }
                send_button = <Button> {
                    text: "Send"
                }
                clear_button = <Button> {
                    text: "Clear"
                }
                connect_serial = <Button> {
                    text: "Serial"
                }
            }
                
        }
    }
}
