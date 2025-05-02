
use makepad_widgets::*;
use makepad_xr_net::xr_net::*;
use makepad_widgets::xr_hands::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                
                body = <View>{
                    flow: Down,
                    spacing: 10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    show_bg: true,
                    draw_bg:{
                        fn pixel(self) -> vec4 {
                            let center = vec2(0.5, 0.5);
                            let uv = self.pos - center;
                            let radius = length(uv);
                            let angle = atan(uv.y, uv.x);
                            let color1 = mix(#f00, #00f, 0.5 + 10.5 * cos(angle + self.time));
                            let color2 = mix(#0f0, #ff0, 0.5 + 0.5 * sin(angle + self.time));
                            let color = mix(color1, color2, radius);
                            
                            return depth_clip(self.world, color, self.depth_clip);
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
    #[rust(XrNetNode::new(cx))] xr_net:XrNetNode,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) { 
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_startup(&mut self, cx:&mut Cx){
        cx.xr_start_presenting();
    }
    
    fn handle_actions(&mut self, _cx: &mut Cx, _actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::XrUpdate(e) = event{
            self.xr_net.send_state((*e.state).clone());
            /*
            use makepad_platform::makepad_micro_serde::*;
            let data = (*e.state).serialize_bin();
            let compr = makepad_miniz::compress_to_vec(&data,10);
            log!("{:?} {:?}", data.len(), compr.len());
            */
            if let Some(mut xr_hands) = self.ui.xr_hands(id!(xr_hands)).borrow_mut(){
                while let Ok(msg) = self.xr_net.incoming_receiver.try_recv(){
                    match msg{
                        XrNetIncoming::Join{state,peer}=>xr_hands.update_peer(cx, peer.to_live_id(), state, e),
                        XrNetIncoming::Leave{peer}=>xr_hands.leave_peer(cx, peer.to_live_id()),
                        XrNetIncoming::Update{state,peer}=>xr_hands.update_peer(cx, peer.to_live_id(), state, e),
                    }
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}