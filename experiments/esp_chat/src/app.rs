use crate::makepad_live_id::*;
use makepad_widgets::*;
use std::{env, io, str, time::Duration, thread};

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    import crate::app_ui::AppUI;
    App = {{App}} { 
        ui: <Root> {
            <Window> {
                window: {inner_size: vec2(800, 600)},
                caption_bar = {visible: true, caption_label = {label = {text: "Esp chat"}}},
                hide_caption_on_fullscreen: true,
                body = <AppUI>{}
            }
        }
    }
}

app_main!(App);

 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] chat: Vec<(ChatMsg,String)>,
}

enum ChatMsg{
    Own,
    Other(String)
}

impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::app_ui::live_design(cx);
    }
}
     
impl App {
   fn connect_serial(&self){
       let mut send_port = serialport::new("/dev/cu.usbmodem1101", 115_200)
       .timeout(Duration::from_millis(1000000))
       .open()
       .unwrap();
       let mut recv_port = send_port.try_clone().unwrap();
       // Read from the serial port and print to the terminal
       thread::spawn(move || {
           loop {
               let mut buf = [0; 256];
               let len = recv_port.read(&mut buf).unwrap();
               print!("{}", str::from_utf8(&buf[..len]).unwrap());
           }
       });
       // Read from the terminal and send to the serial port
       loop {
           let message = "Hello world\n";
           send_port.write_all(message.as_bytes()).unwrap();
           std::thread::sleep(Duration::from_millis(1000));
       }
   }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx:&mut Cx){
        self.connect_serial();
    }
    
    fn handle_signal(&mut self, _cx: &mut Cx){
        
    }
            
    fn handle_draw_2d(&mut self, cx:&mut Cx2d){
        let chat = self.ui.portal_list(id!(chat));
                
        while let Some(next) = self.ui.draw(cx, &mut Scope::empty()).step() {
           if let Some(mut chat) = chat.has_widget(&next).borrow_mut() {
                chat.set_item_range(cx, 0, self.chat.len());
                while let Some(item_id) = chat.next_visible_item(cx) {
                    if item_id >= self.chat.len(){
                        continue
                    }
                    let (ty, msg) = &self.chat[item_id];
                    let template = match ty{
                        ChatMsg::Own=>live_id!(Own),
                        ChatMsg::Other(_)=>live_id!(Other),
                    };
                    let item = chat.item(cx, item_id, template).unwrap();
                    item.set_text(msg);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
    }
    
    
    fn handle_actions(&mut self, cx:&mut Cx, actions:&Actions){
        
        let message = self.ui.text_input(id!(message));
        if let Some(val) = message.returned(&actions){
            message.set_text_and_redraw(cx, "");
            message.set_cursor(0,0);
            self.chat.push((ChatMsg::Own, val));
            self.ui.widget(id!(chat)).redraw(cx);
        }
                  
        if self.ui.button(id!(send_button)).clicked(&actions) {
            
        }
        
        if self.ui.button(id!(clear_button)).clicked(&actions){
            self.chat.clear();
            self.ui.widget(id!(chat)).redraw(cx);
        }    
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.match_event_with_draw_2d(cx, event).is_ok(){
            return
        }
        
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
