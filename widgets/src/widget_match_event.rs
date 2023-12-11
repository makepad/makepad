
use crate::*;

pub trait WidgetMatchEvent{
    fn handle_next_frame(&mut self, _cx: &mut Cx, _e:&NextFrameEvent, _scope: &mut Scope){}
    fn handle_actions(&mut self, _cx: &mut Cx, _e:&Actions, _scope: &mut Scope){}
    fn handle_signal(&mut self, _cx: &mut Cx, _scope: &mut Scope){}
    fn handle_audio_devices(&mut self, _cx: &mut Cx, _e:&AudioDevicesEvent, _scope: &mut Scope){}
    fn handle_midi_ports(&mut self, _cx: &mut Cx, _e:&MidiPortsEvent, _scope: &mut Scope){}
    fn handle_video_inputs(&mut self, _cx: &mut Cx, _e:&VideoInputsEvent, _scope: &mut Scope){}
    fn handle_network_responses(&mut self, _cx: &mut Cx, _e:&NetworkResponsesEvent, _scope: &mut Scope){}
    fn widget_match_event(&mut self, cx:&mut Cx, event:&Event, scope: &mut Scope){
        match event{
            Event::NextFrame(e)=>self.handle_next_frame(cx, e, scope),
            Event::Actions(e)=>self.handle_actions(cx,e, scope),
            Event::AudioDevices(e)=>self.handle_audio_devices(cx, e, scope),
            Event::MidiPorts(e)=>self.handle_midi_ports(cx, e, scope),
            Event::VideoInputs(e)=>self.handle_video_inputs(cx, e, scope),
            Event::NetworkResponses(e)=>self.handle_network_responses(cx, e, scope),
            _=>()
        }
    }
}
