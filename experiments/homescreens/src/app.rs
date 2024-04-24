use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;

    import makepad_experiments_homescreens::my_widget::MyWidget;
    import makepad_experiments_homescreens::iconbutton::IconButton;
    
    import makepad_experiments_homescreens::diffuse::DiffuseThing;
    ContainerStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform shadowopacity:  0.2,
            uniform shadowx: 4.0,
            uniform shadowy: 4.0,
            uniform shadowcolor: vec3(0.01,0.01,0.02),
            varying o0: vec2,
            varying oShadow: vec2,
            
            fn vertex(self) -> vec4 {                
                let dpi = self.dpi_factor;                               
                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                self.o0 = self.pos;
                self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi )*0.001;
                return pos;
            }

            fn pixel(self) -> vec4 {

                let shadow = sample2d_rt(self.image, self.oShadow + vec2(cos(self.time*3.+self.o0.y*10.)*0.0013, cos(self.time+self.o0.x*100.)*0.0013));
                let main = sample2d_rt(self.image, self.o0);
                let col =  (vec4(self.shadowcolor.xyz,self.shadowopacity)  * shadow.a ) * ( 1 - main.a) + main;
                return col;
            }
        }
    }

    ReflectorStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform shadowopacity:  0.2,
            uniform shadowx: 4.0,
            uniform shadowy: 4.0,
            uniform shadowcolor: vec3(0.01,0.01,0.02),
            varying o0: vec2,
            varying oShadow: vec2,
            
            fn vertex(self) -> vec4 {                
                let dpi = self.dpi_factor;                               
                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                self.o0 = self.pos;
                self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi )*0.001;
                return pos;
            }

            fn pixel(self) -> vec4 {

    
                let main = sample2d_rt(self.image, self.o0);
                let uv = self.o0  - vec2(0.03,0.07);
                uv.y +=sin(uv.y*140.)*0.02+ (cos((uv.y + (self.time * 0.04)) * 45.0) * 0.0019) + (cos((uv.y + (self.time * 0.1)) * 10.0) * 0.002);
                uv.x += sin(uv.y*420.)*0.02+ (sin((uv.y + (self.time * 0.07)) * 15.0) * 0.0029) + (sin((uv.y + (self.time * 0.1)) * 15.0) * 0.002);
                let flect = sample2d_rt(self.image, uv);
                let col =  vec4(flect.xyz*0.1, flect.w * 0.1) * ( 1 - main.a) + main;
                return col;
            }
        }
    }

    IconSet = <View>
    {
            width: Fill,
            height: Fill,
            flow: Down
            spacing: 5
            padding: 5
        <View>{
            spacing: 5
            width: Fill,
            height: Fill,
            align: {x:0., y:0.5}
            flow: Right,
            <IconButton>{button={text:"Amaze-on"},width: Fill,image={source: dep("crate://self/resources/Icon1.png")}}
            <IconButton>{button={text:"Pearstore"},width: Fill,image={source: dep("crate://self/resources/Icon2.png")}}
            <IconButton>{button={text:"Tao's Tacos"},width: Fill,image={source: dep("crate://self/resources/Icon3.png")}}
            <IconButton>{button={text:"Floof"},width: Fill,image={source: dep("crate://self/resources/Icon4.png")}}
            
        }
        <View>{
            width: Fill,
            height: Fill,
            flow: Right,
            
            <IconButton>{button={text:"JackyYes"},width: Fill,image={source: dep("crate://self/resources/Icon5.png")}}
            <IconButton>{button={text:"MangoTime"},width: Fill,image={source: dep("crate://self/resources/Icon6.png")}}
            <IconButton>{button={text:"Browser"},width: Fill,image={source: dep("crate://self/resources/Icon7.png")}}
            <IconButton>{button={text:"Game-Royale"},width: Fill,image={source: dep("crate://self/resources/Icon8.png")}}
                                
        }
        <View>{
            width: Fill,
            height: Fill,
            flow: Right,
         
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon9.png")}, button={text: "P-Express"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon10.png")}, button={text: "Fishness"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon11.png")}, button={text: "ZenTea"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon12.png")}, button={text: "Fishness"}}
        }
        <View>{
            width: Fill,
            height: Fill,
            flow: Right,
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon13.png")}, button={ text: "Diwe"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon14.png")}, button={ text: "Wubi"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon15.png")}, button={text: "RideHyper"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon16.png")}, button={text: "TrustyBank"}}
        }
    
    }

    App = {{App}} {        
        ui: <Window>{
            
            window: {
                inner_size: vec2(640,1024)
            }
            show_bg: true
            width: Fill,
            height: Fill
            padding : 0,
            spacing : 0,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#7,#4,self.pos.y);
                }
            }

            body = <View>{
                width: Fill,
                height: Fill,
                flow: Down,
                padding: 0,
                spacing: 0,
                
                <View>{
                    width: Fill, height: 28, draw_bg:
                    {
                        fn pixel(self) ->  vec4{return #ff0;}
                    }
                }
                
                <Dock>{
                    width: Fill,
                    height: Fill,
                    padding: 0,
                    spacing: 0,

                    root = Tabs{tabs:[screen1tab, screen2tab, screen3tab], selected:0}

                    screen1tab = Tab{
                        name: "Screen 1"
                        kind: screen1
                    }

                    screen2tab = Tab{
                        name: "Screen 2"
                        kind: screen2
                    }

                    screen3tab = Tab{
                        name: "Screen 3"
                        kind: screen3
                    }
    
                    screen1 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                
                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                fn pixel(self) -> vec4 
                                {
                                
                                    let time = self.time * .015+23.0;
                                    let uv = self.pos*0.1;
                                    let p = mod(uv*6.283, 6.283)-250.0;
                                    let i = vec2(p);
                                    let c = 1.0;
                                    let inten = .005;
                                    let n = 0;
                                    for _n in 0..4 
                                    {
                                        let t = time * (1.0 - (3.5 / (float(n) +1.0)));
                                        i = p + vec2(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
                                        c += 1.0/length(vec2(p.x / (sin(i.x+t)/inten),p.y / (cos(i.y+t)/inten)));
                                        n = n + 1;
                                    }
                                    c /= float(5);
                                    c = 1.17-pow(c, 1.4);
                                    let colour = vec3(pow(abs(c), 8.0));
                                    colour = clamp(colour*.8 + vec3(0.70, 0.35, 0.5), 0.0, 1.0);
                                    return vec4(colour, 1.0);
                                }
                            }
                        }
                         <ContainerStage>{   
                            <DiffuseThing>{}
                            <IconSet> {}              
                         }
                
                        
                    }   
                    screen3 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },

                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                // this example shader is ported from kishimisu's tutorial
                                fn pixel(self) -> vec4 {
                                // let uv = self.pos - 0.5;
                                //  let uv0 = uv;


                               
                                
                                let fragColor = mix( vec4(.8,0.8,.8, 1.),vec4(0.0,0.1,0.3, 1.0), self.pos.y);



                                    //let finalColor = vec3(0.3+0.01*sin(uv.x*6.283*4));
                                    return fragColor;
                                }
                            }
                        }
                        <ReflectorStage>{   
                            <IconSet>{}
                        }
                    }
                    screen2 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                
                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                // this example shader is ported from kishimisu's tutorial
                                fn pixel(self) -> vec4 {
                                // let uv = self.pos - 0.5;
                                //  let uv0 = uv;


                                let time = self.time * .15+23.0;
                                // uv should be the 0-1 uv of texture...
                                let uv = self.pos;
                                
                                let p = mod(uv*6.283, 6.283)-250.0;
                                let i = vec2(p);
                                let c = 1.0;
                                let inten = .005;
                                let n = 0;
                                for _n in 0..4 
                                {
                                    let t = time * (1.0 - (3.5 / (float(n) +1.0)));
                                    i = p + vec2(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
                                    c += 1.0/length(vec2(p.x / (sin(i.x+t)/inten),p.y / (cos(i.y+t)/inten)));
                                    n = n + 1;
                                }
                                c /= float(5);
                                c = 1.17-pow(c, 1.4);
                                let colour = vec3(pow(abs(c), 8.0));
                                colour = clamp(colour*.8 + vec3(0.0, 0.35, 0.5), 0.0, 1.0);

                                
                                let fragColor = vec4(colour, 1.0);



                                    //let finalColor = vec3(0.3+0.01*sin(uv.x*6.283*4));
                                    return fragColor;
                                }
                            }
                        }
                        <ContainerStage>{   
                            <IconSet> {}              
                         }
                
                        
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
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::my_widget::live_design(cx);
        crate::iconbutton::live_design(cx);
        crate::diffuse::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}