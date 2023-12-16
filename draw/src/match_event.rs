use makepad_platform::*;
use crate::Cx2d;

pub trait MatchEvent{
    fn handle_startup(&mut self, _cx: &mut Cx){}
    fn handle_shutdown(&mut self, _cx: &mut Cx){}
    fn handle_pause(&mut self, _cx: &mut Cx){}
    fn handle_resume(&mut self, _cx: &mut Cx){}
    fn handle_app_got_focus(&mut self, _cx: &mut Cx){}
    fn handle_app_lost_focus(&mut self, _cx: &mut Cx){}
    fn handle_next_frame(&mut self, _cx: &mut Cx, _e:&NextFrameEvent){}
    fn handle_action(&mut self, _cx: &mut Cx, _e:&Action){}
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        for action in actions{
            self.handle_action(cx, action);
        }
    }
    fn handle_signal(&mut self, _cx: &mut Cx){}
    fn handle_audio_devices(&mut self, _cx: &mut Cx, _e:&AudioDevicesEvent){}
    fn handle_midi_ports(&mut self, _cx: &mut Cx, _e:&MidiPortsEvent){}
    fn handle_video_inputs(&mut self, _cx: &mut Cx, _e:&VideoInputsEvent){}
    fn handle_network_responses(&mut self, _cx: &mut Cx, _e:&NetworkResponsesEvent ){}
    fn handle_draw(&mut self, _cx: &mut Cx, _e:&DrawEvent){}
    fn handle_draw_2d(&mut self, _cx: &mut Cx2d){}
    fn handle_key_down(&mut self, _cx: &mut Cx, _e:&KeyEvent){}
    fn handle_key_up(&mut self, _cx: &mut Cx, _e:&KeyEvent){}
    fn match_event(&mut self, cx:&mut Cx, event:&Event){
        match event{
            Event::Startup=>self.handle_startup(cx),
            Event::Shutdown=>self.handle_shutdown(cx),
            Event::Pause=>self.handle_pause(cx),
            Event::Resume=>self.handle_resume(cx),
            Event::AppGotFocus=>self.handle_app_got_focus(cx),
            Event::AppLostFocus=>self.handle_app_lost_focus(cx),
            Event::NextFrame(e)=>self.handle_next_frame(cx, e),
            Event::Actions(e)=>self.handle_actions(cx,e),
            Event::Draw(e)=>self.handle_draw(cx, e),
            Event::AudioDevices(e)=>self.handle_audio_devices(cx, e),
            Event::MidiPorts(e)=>self.handle_midi_ports(cx, e),
            Event::VideoInputs(e)=>self.handle_video_inputs(cx, e),
            Event::NetworkResponses(e)=>self.handle_network_responses(cx, e),
            Event::KeyDown(e)=>self.handle_key_down(cx, e),
            Event::KeyUp(e)=>self.handle_key_up(cx, e),
            _=>()
        }
    }
    
    fn match_event_with_draw_2d(&mut self, cx:&mut Cx, event:&Event)->Result<(),()>{
        match event{
            Event::Draw(e)=>{
                let cx = &mut Cx2d::new(cx, e);
                self.handle_draw_2d(cx);
                Ok(())
            }
            e=>{
                self.match_event(cx, e);
                Err(())
            }
        }
    }
}
