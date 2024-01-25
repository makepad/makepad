use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 

    ZooHeader = <View> {
        show_bg: true
        draw_bg: {color: #ddd}
        width: Fill,
        height: Fit
        flow: Down
         spacing: 10, padding: 15, margin:{bottom:10}
        title = <Label> {
            draw_text: {
                color: #9
                text_style: {
                    line_spacing:1.0
                    font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
                    font_size: 14
                }
            }
            text: "SlideTitle"
        }
    }

    ZooTitle = <View> {
        draw_bg: {color: #x1A}
        width: Fit,
        height: Fit
        flow: Down, spacing: 10, padding: 0, margin: 20
        title = <Label> {
            draw_text: {
                color: #2
                text_style: {
                    line_spacing:1.0
                    font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
                    font_size: 25
                }
            }
            text: "Makepad UI Zoo"
        }
    }


    ZooDesc = <Label> {
        margin: 0,
        padding: 0,
        spacing: 0,
        draw_text: {
            color: #3
            text_style: {
                line_spacing:1.5
                font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
                font_size: 13
            }
        }
        text: ""
    }


    ZooBlock = 
        <RoundedView> {
            
        show_bg: true;
        width: 50
        height: 50
        margin: 0,
        padding: 0,
        spacing: 0,

        draw_bg: {
          color: #ff0
             fn get_color(self) -> vec4 {
                //return #000
                return mix(self.color, self.color*0.7, self.pos.y);
            }
            radius: 5.0
        }
    }

    App = {{App}} {
        ui: <Window>{
            caption_bar = { margin: {left: -100}, visible: true, caption_label = {label = {text: "Makepad UI Zoo"}} },
            show_bg: true
            width: Fill,
            height: Fill
            draw_bg: {
                fn pixel(self) -> vec4 {                   
                    return #c;
                }
            }
            
            body = <View>{
                flow: Down,
                width: Fill, 
                height: Fill,
                spacing: 10,
                scroll_bars: <ScrollBars>{}
                
                <ZooTitle>{}
                
                <ZooHeader>
                {
                        title = {text:"View 1"}
                        <ZooDesc>{text:"This is a gray view with flow set to Right\nTo show the extend, the background has been enabled using show_bg and a gray pixelshader has been provided to draw_bg."}
                        <View>
                         {
                            show_bg: true,
                            draw_bg:{ 
                                
                                fn pixel(self) -> vec4{
                                return #bbb
                            }}           
                            padding: 10
                            spacing: 10
                            height: Fit
                            flow: Right,                    
                            <ZooBlock>{draw_bg:{color: #f00}}
                            <ZooBlock>{draw_bg:{color: #ff0}}
                            <ZooBlock>{draw_bg:{color: #00f}}
                         }

                         <ZooDesc>{text:"This is a view with flow set to Down"}
                         <View>
                          {
                             show_bg: true,
                             draw_bg:{ 
                                 
                                 fn pixel(self) -> vec4{
                                 return #bbb
                             }}           
                             padding: 10
                             spacing: 10
                             height: Fit
                             flow: Down,                    
                             <ZooBlock>{draw_bg:{color: #f00}}
                             <ZooBlock>{draw_bg:{color: #ff0}}
                             <ZooBlock>{draw_bg:{color: #00f}}
                          }
     
                }   


                <ZooHeader>{
                    title = {text:"RoundedView"}
                    <ZooDesc>{text:"This is a Rounded View. Please note that the radius has to be represented as a float value (with a decimal point) to work. Also note that instead of replacing the main pixel shader - you now replace get_color instead so the main shader can take care of rendering the radius."}
                    <RoundedView>
                     {
                        show_bg: true,
                        draw_bg:{ 
                            
                            radius: 10.0
                            fn get_color(self) -> vec4{
                            return #bbb
                        }}           
                        padding: 10
                        spacing: 10
                        height: Fit
                        flow: Right,                    
                        <ZooBlock>{draw_bg:{color: #f00}}
                        <ZooBlock>{draw_bg:{color: #ff0}}
                        <ZooBlock>{draw_bg:{color: #00f}}
                     }
                }   



                <ZooHeader>{
                    title = {text:"Button"}
                    <ZooDesc>{text:"A small clickable region"}
                   
                     <Button> {
                        text: "I can be clicked"
                    }
                     <IconButton> 
                     {
                            draw_icon: 
                            {
                                svg_file: (ICO_SEQ_SWEEP)
                            }
                            icon_walk: {width: 15.0, height: Fit}
                    }
                     <Button> {
                        text: "I can also be clicked, and I have an icon!"
                    }
                }   


                <ZooHeader>{
                    title = {text:"TextInput"}
                    <ZooDesc>{text:"Simple 1 line textbox"}
                   
                    <TextInput> {
                        width: 100, height: 30
                        text: "Click to count"
                    }
                }   


                <ZooHeader>{
                    title = {text:"Label"}
                    <ZooDesc>{text:"Simple 1 line textbox"}
                   
                    <Label> {
                        text: "This is a small line of text"
                        
                    }

                    <Label> {
                        draw_text: 
                        { 
                            color: #00c  
                            text_style:{font_size: 20
                            font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}}
                        },
                        text: "You can style text using colors and fonts"                        
                    }

                    <Label> {
                        draw_text: 
                        { 
                            fn get_color(self) ->vec4{
                                return mix(#f0f, #0f0, self.pos.x)
                            }
                            color: #ffc  
                            text_style:{font_size: 40
                            font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}}
                        },
                        text: "Or even some pixelshaders"                        
                    }

                }                
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            log!("BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}