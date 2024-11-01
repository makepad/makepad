
use crate::*;

pub trait WidgetMatchEvent{
    fn handle_next_frame(&mut self, _cx: &mut Cx, _e:&NextFrameEvent, _scope: &mut Scope){}
    fn handle_actions(&mut self, _cx: &mut Cx, _e:&Actions, _scope: &mut Scope){}
    fn handle_signal(&mut self, _cx: &mut Cx, _scope: &mut Scope){}
    fn handle_audio_devices(&mut self, _cx: &mut Cx, _e:&AudioDevicesEvent, _scope: &mut Scope){}
    fn handle_midi_ports(&mut self, _cx: &mut Cx, _e:&MidiPortsEvent, _scope: &mut Scope){}
    fn handle_video_inputs(&mut self, _cx: &mut Cx, _e:&VideoInputsEvent, _scope: &mut Scope){}
    
    fn handle_http_response(&mut self, _cx:&mut Cx, _request_id:LiveId, _response:&HttpResponse, _scope: &mut Scope){}
    fn handle_http_request_error(&mut self, _cx:&mut Cx, _request_id:LiveId, _err:&HttpError, _scope: &mut Scope){}
    fn handle_http_progress(&mut self, _cx:&mut Cx, _request_id:LiveId, _progress:&HttpProgress, _scope: &mut Scope){}
    fn handle_http_stream(&mut self, _cx:&mut Cx, _request_id:LiveId, _data:&HttpResponse, _scope: &mut Scope){}
    fn handle_http_stream_complete(&mut self, _cx:&mut Cx, _request_id:LiveId, _data:&HttpResponse, _scope: &mut Scope){}
        
    fn handle_network_responses(&mut self, cx: &mut Cx, e:&NetworkResponsesEvent, scope: &mut Scope){
        for e in e{
            match &e.response{
                NetworkResponse::HttpRequestError(err)=>{
                    self.handle_http_request_error(cx, e.request_id, err, scope);
                }
                NetworkResponse::HttpResponse(res)=>{
                    self.handle_http_response(cx, e.request_id, res, scope);
                }
                NetworkResponse::HttpProgress(progress)=>{
                    self.handle_http_progress(cx, e.request_id, progress, scope);
                }
                NetworkResponse::HttpStreamResponse(data)=>{
                    self.handle_http_stream(cx, e.request_id, data, scope);
                }
                NetworkResponse::HttpStreamComplete(res)=>{
                    self.handle_http_stream_complete(cx, e.request_id, res, scope);
                }
            }
        }
    }
    
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
