/*
This example loads pngs it receives from a websocket.
Allowing to use an apple tv as a slideshow viewer by sending it pngs over websocket
*/

use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {

        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill
            
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#5, #3, self.pos.y);
                }
            }
            
            body = <View>{
                flow: Overlay,
                align:{x:0.0,y:1.0}
                Logo = <Button> {
                    draw_icon: {
                        svg_file: dep("crate://self/resources/logo_makepad.svg"),
                        fn get_color(self) -> vec4 {
                            return (#8)
                        }
                    }
                    icon_walk: {margin:{left:200,bottom:100},width: 850.0, height: Fit}
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            return #0000
                        }
                    }
                }
                image = <ImageBlend> {
                    width: Fill,
                    height: Fill,
                    margin: 0
                    fn get_color(self) -> vec4 {
                        return self.get_color_scale_pan(self.image_scale, self.image_pan)*2.0
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
    #[rust] web_socket: Option<WebSocket>,
    #[rust] timer: Timer,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_startup(&mut self, cx: &mut Cx){
        // lets connect the websoc ket
        self.open_websocket(cx);
        self.timer = cx.start_interval(1.0);
        //self.ui.label(id!(time)).set_text_and_redraw(cx, "A");
    }
    
    fn handle_timer(&mut self, _cx:&mut Cx, _e:&TimerEvent){
        let _ = self.web_socket.as_mut().unwrap().send_binary("HI".into());
        //self.ui.label(id!(time)).set_text_and_redraw(cx, &format!("{:?}", e));
    }
    
    fn handle_signal(&mut self, cx: &mut Cx){
        if let Some(socket) = &mut self.web_socket{
            match socket.try_recv(){
                Ok(WebSocketMessage::Binary(data))=>{
                    self.ui.label(id!(time)).set_text(cx, &format!("GOT BINARY {}", data.len()));
                    // we got a new image. lets decode and show it
                    let img = self.ui.image_blend(id!(image));
                    let _ = img.load_png_from_data(cx, &data);
                    img.redraw(cx);
                }
                Ok(WebSocketMessage::Closed)=>{
                     println!("WEBSOCKET CLOSED");
                    self.open_websocket(cx);
                }
                Ok(WebSocketMessage::Error(_e))=>{
                    println!("WEBSOCKET ERROR {}",_e);
                    self.open_websocket(cx);
                }
                _=>()
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl App{
    fn open_websocket(&mut self, _cx:&mut Cx){
        let request = HttpRequest::new("http://10.0.0.105:8009".into(), HttpMethod::GET);
        self.web_socket = Some(WebSocket::open(request));
    }
}