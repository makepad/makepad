#![allow(unused_variables)]
use {
    crate::{
        register_as_audio_component,
        audio_registry::*,
        audio_graph::*,
        makepad_platform::*,
        makepad_platform::platform::apple::{
            audio_unit::*,
            core_midi::*,
        },
    },
};

live_register!{
    PluginMusicDevice: {{PluginMusicDevice}} {
        plugin:"FM8"
    }
}

enum ToUI {
    NewDevice(AudioDevice)
}

enum FromUI {
    NewDevice(AudioDeviceClone)
}

#[derive(Live)]
#[live_register( | cx: &mut Cx | {register_as_audio_component!(cx, PluginMusicDevice)})]
struct PluginMusicDevice {
    plugin:String,
    preset_data:String,

    #[rust] audio_device: Option<AudioDevice>,
    #[rust(FromUISender::new())] from_ui: FromUISender<FromUI>,
    #[rust(ToUIReceiver::new(cx))] to_ui: ToUIReceiver<ToUI>,
}

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    audio_device: Option<AudioDeviceClone>
}

impl AudioGraphNode for Node{
    fn handle_midi_1_data(&mut self, data:Midi1Data){
        if let Some(audio_device) = &self.audio_device{
            audio_device.handle_midi_1_data(data);
        }
    }
    
    fn render_to_audio_buffer(&mut self, buffer: &mut AudioBuffer){
        while let Ok(msg) = self.from_ui.try_recv(){
            match msg{
                FromUI::NewDevice(device)=>{
                    self.audio_device = Some(device);
                }
            }
        }
        if let Some(audio_device) = &self.audio_device{
            audio_device.render_to_audio_buffer(buffer);
        }
    }
}

impl LiveHook for PluginMusicDevice {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.load_audio_device();
    }
}

impl PluginMusicDevice{
    fn load_audio_device(&mut self){
        // alright lets create an audio device 
        let list = Audio::query_devices(AudioDeviceType::MusicDevice);
        let sender = self.to_ui.sender();
        if let Some(info) = list.iter().find( | item | item.name == self.plugin) {
            Audio::new_device(info, move | result | {
                match result {
                    Ok(device) => {
                        sender.send(ToUI::NewDevice(device)).unwrap()
                    }
                    Err(err) => println!("Error {:?}", err)
                }
            })
        }        
    }
}

impl AudioComponent for PluginMusicDevice {
    fn type_id(&self) -> LiveType {LiveType::of::<Self>()}

    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node{
            from_ui: self.from_ui.receiver(),
            audio_device: if let Some(device) = &self.audio_device{Some(device.clone())}else{None}
        })
    }
    
    fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
        match event {
            Event::Signal(se) => while let Ok(to_ui) = self.to_ui.try_recv(se) {
                match to_ui{
                    ToUI::NewDevice(device)=>{
                        self.from_ui.send(FromUI::NewDevice(device.clone())).unwrap();
                        self.audio_device = Some(device);
                    }
                }
            }
            _ => ()
        }
    }
}


