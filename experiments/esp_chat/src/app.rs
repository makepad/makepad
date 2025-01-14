use crate::makepad_live_id::*;
use makepad_widgets::*;
use std::{env, str, time::Duration, thread};
use serialport::SerialPort;

live_design!{
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_draw::shader::std::*;
    use crate::app_ui::AppUI;
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
    #[rust] send_port: Option<Box<dyn SerialPort>>,
    #[rust] receiver: ToUIReceiver<String>
}

enum ChatMsg{
    Own,
    Other
}

impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::app_ui::live_design(cx);
    }
}
     
impl App {
    fn send_message(&mut self, cx:&mut Cx){
        let message_input = self.ui.text_input(id!(message_input));
        let name_input = self.ui.text_input(id!(name_input));
        let message = message_input.text();
        let name = name_input.text();
        message_input.set_text(cx, "");
        message_input.set_cursor(0,0);
        // lets send it on the serial port
        if let Some(send_port) = &mut self.send_port{
            let msg = format!("{}:{}\n", name, message);
            send_port.write_all(msg.as_bytes()).unwrap();
        }
        // push it in
        self.chat.push((ChatMsg::Own, message));
        self.ui.widget(id!(chat)).redraw(cx);
    }
    
    fn connect_serial(&mut self){
        return;
       let send_port = serialport::new("/dev/cu.usbmodem101", 115_200)
       .timeout(Duration::from_millis(1000000))
       .open()
       .unwrap();
       
       let mut recv_port = send_port.try_clone().unwrap();
       self.send_port = Some(send_port);
       let sender = self.receiver.sender();
       // Read from the serial port and print to the terminal
       thread::spawn(move || {
           // rest
           let mut rest = String::new();
           loop {
               let mut buf = [0; 1024];
               let len = recv_port.read(&mut buf).unwrap();
               // properly group by newlines
               let s = str::from_utf8(&buf[..len]).unwrap();
               let line:Vec<&str> = s.split("\n").collect();
               // always append the first part
               rest.push_str(line[0]);
               // if there is more than one part, we have a newline
               if line.len() > 1{
                   // send the rest first
                   sender.send(rest).ok();
                   // store the last bit in the rest
                   rest = line.last().unwrap().to_string();
                   // now send the middle chunks
                   
                   for i in 1..line.len() - 1{
                       sender.send(line[i].into()).ok();
                   }
               }
           }
       });
   }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx:&mut Cx){
        self.connect_serial();
    }
    
    fn handle_signal(&mut self, cx: &mut Cx){
        while let Ok(line) = self.receiver.try_recv(){
            self.chat.push((ChatMsg::Other, line));
            self.ui.portal_list(id!(chat)).redraw(cx);
        }
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
                        ChatMsg::Other=>live_id!(Other),
                    };
                    let item = chat.item(cx, item_id, template).unwrap();
                    item.set_text(msg);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
    }
    
    
    fn handle_actions(&mut self, cx:&mut Cx, actions:&Actions){
        let message_input = self.ui.text_input(id!(message_input));
        if  message_input.returned(&actions).is_some(){
            self.send_message(cx);
        }
                  
        if self.ui.button(id!(send_button)).clicked(&actions) {
            self.send_message(cx);
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
