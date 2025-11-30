use makepad_draw2::*;

app_main!(App); 
 
#[derive(Script, ScriptHook)]
pub struct App {
}
 
impl MatchEvent for App{
    fn handle_startup(&mut self, _cx:&mut Cx){
        log!("STARTUP")
    }
        
    fn handle_actions(&mut self, _cx: &mut Cx, _actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
    }
}