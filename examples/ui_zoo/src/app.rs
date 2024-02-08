use makepad_widgets::*;
use makepad_platform::live_atomic::*;
 

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    import makepad_draw::shader::std::*;
    import makepad_example_ui_zoo::demofiletree::*;

    ZooHeader = <View> {
        show_bg: true
        draw_bg: {color: #ddd}
        width: Fill,
        height: Fit
        flow: Down
         spacing: 10, padding: 15, margin:{bottom:10}
         divider = <View>
         {
            width: Fill, height: 2
            show_bg: true
            draw_bg: {color: #ccc}
        }
        title = <Label> {
            draw_text: {
                color: #f50
                text_style: {
                    line_spacing:1.0
                    font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
                    font_size: 14
                }
            }
            text: "Header"
        }
    }
    ZooGroup = <View>{ 
        
            height: Fit,
            width: Fill,
            flow: Right,
            padding: 10,

            draw_bg: {
                fn pixel(self) -> vec4{
                    return #aaa
                }
            }

           show_bg: true;
    }

    ZooTitle = <View> {
        draw_bg: {color: #x1A}
        width: Fit,
        height: Fit
        flow: Down, spacing: 10, padding: 0, margin: 20
        title = <Label> {
            draw_text: {
                color: #1
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
        margin: {top: 10},
        padding: 0,
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
                    return #d;
                }
            }
            
            body = <View>{


                flow:Right
                
                width: Fill, 
                height: Fill,
                margin: 0 
                padding: 0
                spacing: 0
                <View>{
                    <FileTree>{
                        <FileTreeNode>{text:"bleh"} 
                        <Label>{text: "item"}
                    }
                    margin: 0
                    width: 200
                    show_bg: true
                }
                <View>{
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
                            height: Fit,
                             padding: 10
                             spacing: 10
                             flow: Down,                    
                             <ZooBlock>{draw_bg:{color: #f00}}
                             <ZooBlock>{draw_bg:{color: #ff0}}
                             <ZooBlock>{draw_bg:{color: #00f}}
                            

                          }

                          <ZooDesc>{text:"This view is bigger on the inside"}
                          <View>
                           {
                            scroll_bars: <ScrollBars>{}
                            width: 150
                            height: 150
                            flow: Right,
                              show_bg: true,
                              draw_bg:{ 
                                  
                                  fn pixel(self) -> vec4{
                                  return #bbb
                              }
                            }         
                              
                            <View>{  
                                show_bg: false,
                                width: Fit,
                                height: Fit,
                                padding: 0
                                spacing: 10
                                flow: Down,                    
                                <ZooBlock>{draw_bg:{color: #f00}}
                                <ZooBlock>{draw_bg:{color: #ff0}}
                                <ZooBlock>{draw_bg:{color: #00f}}
                                <ZooBlock>{draw_bg:{color: #0f0}}
                              } 
                              <View>{  
                                show_bg: false,
                                width: Fit,
                                height: Fit,
                                padding: 0
                                spacing: 10
                                flow: Down,                    
                                <ZooBlock>{draw_bg:{color: #f00}}
                                <ZooBlock>{draw_bg:{color: #ff0}}
                                <ZooBlock>{draw_bg:{color: #00f}}
                                <ZooBlock>{draw_bg:{color: #0f0}}
                              } 
                              <View>{  
                                show_bg: false,
                                width: Fit,
                                height: Fit,
                                padding: 0
                                spacing: 10
                                flow: Down,                    
                                <ZooBlock>{draw_bg:{color: #f00}}
                                <ZooBlock>{draw_bg:{color: #ff0}}
                                <ZooBlock>{draw_bg:{color: #00f}}
                                <ZooBlock>{draw_bg:{color: #0f0}}

                              }
                              
                              <View>{  
                                show_bg: false,
                                width: Fit,
                                height: Fit,
                                padding: 0
                                spacing: 10
                                flow: Down,                    
                                <ZooBlock>{draw_bg:{color: #f00}}
                                <ZooBlock>{draw_bg:{color: #ff0}}
                                <ZooBlock>{draw_bg:{color: #00f}}
                                <ZooBlock>{draw_bg:{color: #0f0}}
                              }
                              <View>{  
                                show_bg: false,
                                width: Fit,
                                height: Fit,
                              padding: 0
                              spacing: 10
                              flow: Down,                    
                              <ZooBlock>{draw_bg:{color: #f00}}
                              <ZooBlock>{draw_bg:{color: #ff0}}
                              <ZooBlock>{draw_bg:{color: #00f}}
                              }
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
                            }
                        }           
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
                    
                        basicbutton = <Button> {
                            text: "I can be clicked"
                        }
                        
                        iconbutton = <Button> {
                            draw_icon: {
                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                color: #000;
                                brightness: 0.8;
                            }
                            text:"I can have a lovely icon!"

                             icon_walk: {width: 30, margin:14, height: Fit}
                        }

                        styledbutton = <Button> {
                            draw_bg: {
                                fn pixel(self) -> vec4 {                                    
                                    return #f40 + self.pressed * vec4(1,1,1,1)
                                }
                            }
                            draw_text: {
                                fn get_color(self) -> vec4 {                                    
                                    return #fff - vec4(0,.1,.4,0) *self.hover - self.pressed * vec4(1,1,1,0);
                                }                             
                            }                            
                            text: "I can be styled!"
                        }
                    }

                    <ZooHeader>{
                        title = {text:"TextInput"}                        
                        <ZooDesc>{text:"Simple 1 line textbox"}                        
                        <ZooGroup>{
                            simpletextinput= <TextInput> {
                                width: 100
                                text: "This is inside a textbox!"
                            }
                            
                            simpletextinput_outputbox = <Label> {
                                text: "Output"
                            }
                        }
                    }

                    <ZooHeader>{
                        title = {text:"Label"}
                        <ZooDesc>
                        {
                            text:"Simple 1 line textbox"
                        }
                        <ZooGroup>{
                        <Label> 
                        {
                            text: "This is a small line of text"                        
                        }
                    }
                    <ZooGroup>{
                        <Label> {
                            draw_text: 
                            { 
                                color: #00c  
                                text_style:{font_size: 20
                                font:{path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}}
                            },
                            text: "You can style text using colors and fonts"                        
                        }
                    }
                    <ZooGroup>{
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

                    <ZooHeader>{
                        title = {text:"Slider"}
                        <ZooDesc>
                        {
                            text:"A parameter dragger"
                        }
                        <ZooGroup> {
                            <Slider> 
                            {
                                width: 100,
                                height: 30, 
                                draw_slider:{
                                slider_type: Horizontal
                                }
                                text: "param"                        
                            }          
                        }      
                     
                    }              

                    <ZooHeader>{title = {text:"KeyboardView"} <ZooDesc>{text:"KeyboardView ?"}<ZooGroup> {<KeyboardView>{}}  }              
                    <ZooHeader>{title = {text:"CheckBox"} <ZooDesc>{text:"Checkbox ?"}<ZooGroup> 
                    {
                        flow: Down
                        simplecheckbox = <CheckBox>{text:"Check me out!"}
                        simplecheckbox_output = <Label>{text:"hmm"}
                }  
            }              
                    <ZooHeader>{title = {text:"RadioButton"} <ZooDesc>{text:"RadioButton ?"}<ZooGroup>
                     {
                        <RadioButtonSet>{
                        <RadioButton>{text:"Option 1: yey"}
                        <RadioButton>{text:"Option 2: hah"}
                        <RadioButton>{text:"Option 3: hmm"}
                        <RadioButton>{text:"Option 4: all of the above"}
                        }
                    }
                }               
                    <ZooHeader>{title = {text:"DesktopButton"} <ZooDesc>{text:"Desktop Button ?"}<ZooGroup> {<DesktopButton>{}}  }              
                    <ZooHeader>{title = {text:"DropDown"} <ZooDesc>{text:"DropDown ?"}<ZooGroup> 
                    {
                    dropdown = <DropDown>{
                        height: 30,
                        width: 200
                        labels: ["ValueOne", "ValueTwo","Thrice","FourthValue","OptionE","Hexagons"],
                        values: [  ValueOne,ValueTwo,Thrice,FourthValue,OptionE,Hexagons]

                    }}  }              
                       
                    
                    <ZooHeader>{title = {text:"DemoFileTree"} <ZooDesc>{text:"DemoFileTree ?"}<ZooGroup> {<DemoFileTree>{width: Fill, height: 100}}  }     
                    <ZooHeader>{title = {text:"StackViewHeader"} <ZooDesc>{text:"StackViewHeader ?"}<ZooGroup> {<StackViewHeader>{}}  }     
                    <ZooHeader>{title = {text:"FoldHeader"} <ZooDesc>{text:"Fold header ?"}<ZooGroup> {
                        <FoldButton>{text:"Origami"}
                        <FoldHeader>{
                            text:"Origami"
                        }}  }     


                    <ZooHeader>{title = {text:"Image"} <ZooDesc>{text:"A static inline image from a resource."}
                        <ZooGroup> {
                            <Image> {
                                source: dep("crate://self/resources/ducky.png" ),
                                width: (1000 * 0.175),
                                height: (1000 * 0.175),
                                margin: 0
                            }

                        }  
                    }     
                }
            }
        }
    }
}

app_main!(App);


#[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum DropDownEnum {
    #[pick]
    ValueOne,
    ValueTwo,
    Thrice,
    FourthValue,
    OptionE,
    Hexagons,
}

#[derive(Live, LiveHook, LiveRead, LiveRegister)]
pub struct DataBindingsForApp {
    #[live] fnumber: f32,
    #[live] inumber: i32,
    #[live] dropdown: DropDownEnum
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
    #[rust(DataBindingsForApp::new(cx))] bindings: DataBindingsForApp
 }

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::demofiletree::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){

        if let Some(txt) = self.ui.text_input(id!(simpletextinput)).changed(&actions){

            log!("TEXTBOX CHANGED {}", self.counter); 
            self.counter += 1;
            let lbl = self.ui.label(id!(simpletextinput_outputbox));
            lbl.set_text_and_redraw(cx,&format!("{} {}" , self.counter, txt));
        }

        if self.ui.button(id!(basicbutton)).clicked(&actions) {
            log!("BASIC BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let btn = self.ui.button(id!(basicbutton));
            btn.set_text_and_redraw(cx,&format!("Clicky clicky! {}", self.counter));
        }

        if self.ui.button(id!(styledbutton)).clicked(&actions) {
            log!("STYLED BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let btn = self.ui.button(id!(styledbutton));
            btn.set_text_and_redraw(cx,&format!("Styled button clicked: {}", self.counter));        
        }

        if self.ui.button(id!(iconbutton)).clicked(&actions) {
            log!("ICON BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let btn = self.ui.button(id!(iconbutton));
            btn.set_text_and_redraw(cx,&format!("Icon button clicked: {}", self.counter));
        }       


        if let Some(check) = self.ui.check_box(id!(simplecheckbox)).changed(actions) {
            log!("CHECK BUTTON CLICKED {} {}", self.counter, check); 
            self.counter += 1;                  
            let lbl = self.ui.label(id!(simplecheckbox_output));
            lbl.set_text_and_redraw(cx,&format!("{} {}" , self.counter, check));            
        }

        let mut db = DataBindingStore::new();
        db.data_bind(cx, actions, &self.ui, Self::data_bind);        
        self.bindings.apply_over(cx, &db.nodes);

    }

    fn handle_startup(&mut self, cx: &mut Cx) {

        let ui = self.ui.clone();
        let db = DataBindingStore::from_nodes(self.bindings.live_read());
        Self::data_bind(db.data_to_widgets(cx, &ui));     
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl App{
    pub fn data_bind(mut db: DataBindingMap) {
        // sequencer
        db.bind(id!(dropdown), ids!(dropdown));



    }
}