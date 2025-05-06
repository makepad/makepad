/*
TeamTalk is a LAN (wired only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
This example shows using networking and audio IO
*/

use { 
    crate::{
        makepad_micro_serde::*,
        makepad_audio_graph::audio_stream::{AudioStreamSender},
        makepad_widgets::*,
        makepad_platform::live_atomic::*,
    },
    std::sync::Arc,
    std::net::UdpSocket,
    std::time::{Duration},
};

// We dont have a UI yet 

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    App = {{App}} {
        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill,
            window: {inner_size: vec2(400, 300)},
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#7, #3, self.pos.y);
                }
            }
            
            body = <View>{
                padding:20
                flow:Down
                global_volume = <Slider> {
                    padding: 0
                    height: Fit,
                    width: 125,
                    min: 0.0,
                    max: 20.0,
                    margin: {top: 1, left: 2}
                    text: "Out Volume"
                }
                min_volume = <Slider> {
                    padding: 0
                    height: Fit,
                    width: 125,
                    min: 0.0,
                    max: 1.0,
                    margin: {top: 1, left: 2}
                    text: "Min Volume"
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveAtomic, LiveHook, LiveRead, LiveRegister)]
#[live_ignore]
pub struct Store{
    #[live(11.0f64)] global_volume: f64a,
    #[live(0.000f64)] min_volume: f64a,
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] store: Arc<Store>,
    #[rust] volume_changed_by_ui: SignalFromUI,
    #[rust] volume_changed_by_network: SignalToUI,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        makepad_audio_graph::live_design(cx);
    }
}

impl App{
    pub fn store_to_widgets(&self, cx:&mut Cx){
        let db = DataBindingStore::from_nodes(self.store.live_read());
        Self::data_bind_map(db.data_to_widgets(cx, &self.ui));
    }
    
    pub fn data_bind_map(mut db: DataBindingMap) {
        db.bind(id!(global_volume), ids!(global_volume));
        db.bind(id!(min_volume), ids!(min_volume));
    }
}

impl MatchEvent for App{

    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        let mut db = DataBindingStore::new();
        db.data_bind(cx, actions, &self.ui, Self::data_bind_map);
        self.store.apply_over(cx, &db.nodes);
        if db.contains(id!(global_volume)){
            self.volume_changed_by_ui.set();
        }
    }
    
    fn handle_startup(&mut self,  cx: &mut Cx){
        self.start_network_stack(cx);
        self.store_to_widgets(cx);
    }
    
    fn handle_signal(&mut self, cx: &mut Cx){
        if self.volume_changed_by_network.check_and_clear(){
            self.store_to_widgets(cx);
        }
    }
    
    fn handle_audio_devices(&mut self, cx:& mut Cx, devices:&AudioDevicesEvent){
        for desc in &devices.descs{
            println!("{}", desc)
        }
        cx.use_audio_inputs(&devices.default_input());
        cx.use_audio_outputs(&devices.default_output());
    }
}
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
} 

// this is the protocol enum with 'micro-serde' binary serialise/deserialise macro on it.
#[derive(SerBin, DeBin, Debug)]
enum TeamTalkWire {
    //Volume{client_uid: u64, volume: f64},
    Silence {client_uid: u64, frame_count: u32},
    Audio {client_uid: u64, channel_count: u32, data: Vec<i16>},
}

impl App {
    pub fn start_network_stack(&mut self, cx: &mut Cx) {
        // not a very good uid, but it'l do.
        let my_client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).0;
        // Audiostream is an mpsc channel that buffers at the recv side
        // and allows arbitrary chunksized reads. Little utility struct
        let (mic_send, mut mic_recv) = AudioStreamSender::create_pair();
        let (mix_send, mut mix_recv) = AudioStreamSender::create_pair();
        
        // the UDP broadcast socket
        let write_audio = UdpSocket::bind("0.0.0.0:41531").unwrap();
        write_audio.set_read_timeout(Some(Duration::new(5, 0))).unwrap();
        write_audio.set_broadcast(true).unwrap();

        let read_audio = write_audio.try_clone().unwrap();
        /*let volume_changed_by_ui = self.volume_changed_by_ui.clone();
        */
        let store = self.store.clone();
        // our microphone broadcast network thread
        std::thread::spawn(move || {
            let mut wire_data = Vec::new();
            let mut output_buffer = AudioBuffer::new_with_size(640, 1);
            loop {
                // fill the mic stream recv side buffers, and block if nothing
                mic_recv.recv_stream();
                loop {
                    if mic_recv.read_buffer(0, &mut output_buffer, 1, 255) == 0 {
                        break;
                    }
                    let buf = output_buffer.channel(0);
                    // do a quick volume check so we can send 1 byte packets if silent
                    let mut sum = 0.0;
                    for v in buf {
                        sum += v.abs();
                    }
                    let peak = sum / buf.len() as f32;
                    //println!("{}", peak);
                    /*if volume_changed_by_ui.check_and_clear(){
                        /*wire_data.clear();*/
                        /*TeamTalkWire::Volume{client_uid:my_client_uid, volume: store.global_volume.get()}.ser_bin(&mut wire_data);
                        write_audio.send_to(&wire_data, "255.255.255.255:41531").unwrap();*/
                    }*/
                    let wire_packet = if peak > store.min_volume.get() as f32 *0.01 {
                        TeamTalkWire::Audio {client_uid:my_client_uid, channel_count: 1, data: output_buffer.to_i16()}
                    }
                    else {
                        TeamTalkWire::Silence {client_uid:my_client_uid, frame_count: output_buffer.frame_count() as u32}
                    };
                    // serialise the packet enum for sending over the wire
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    // send to all peers
                    write_audio.send_to(&wire_data, "10.0.0.255:41531").unwrap();
                };
            }
        });
        //let volume_changed_by_network = self.volume_changed_by_network.clone();
        /*let store = self.store.clone();*/
        // the network audio receiving thread
        std::thread::spawn(move || {
            let mut read_buf = [0u8; 4096];
            
            while let Ok((len, _addr)) = read_audio.recv_from(&mut read_buf) {
                let read_buf = &read_buf[0..len];
                
                let packet = TeamTalkWire::deserialize_bin(&read_buf).unwrap();
                
                // create an audiobuffer from the data
                let (client_uid, buffer, _silence) = match packet {
                    TeamTalkWire::Audio {client_uid, channel_count, data} => {
                        (client_uid, AudioBuffer::from_i16(&data, channel_count as usize), false)
                    }
                    TeamTalkWire::Silence {client_uid, frame_count} => {
                        (client_uid, AudioBuffer::new_with_size(frame_count as usize, 1), true)
                    }
                    /*TeamTalkWire::Volume{client_uid, volume}=>{
                        if client_uid != my_client_uid{
                            store.global_volume.set(volume);
                            volume_changed_by_network.set();
                        }
                        continue
                    }*/
                };
                
                if client_uid != my_client_uid{
                    mix_send.write_buffer(client_uid, buffer).unwrap();
                }
            }
        });
        
        cx.audio_input(0, move | _info, input_buffer | {
            let mut input_buffer = input_buffer.clone();
            input_buffer.make_single_channel();
            mic_send.write_buffer(0, input_buffer).unwrap();
        });
        let store = self.store.clone();
        cx.audio_output(0, move | _info, output_buffer | {
            //println!("buffer {:?}",_time);
            output_buffer.zero();
            // fill our read buffers on the audiostream without blocking
            mix_recv.try_recv_stream();
            let volume = store.global_volume.get() as f32;
            let mut chan = AudioBuffer::new_like(output_buffer);
            for i in 0..mix_recv.num_routes() {
                if mix_recv.read_buffer(i, &mut chan, 1,4) != 0 {
                    for i in 0..chan.data.len() {
                        output_buffer.data[i] += chan.data[i]*volume;
                    }
                }
            }
        });
    }
    
}