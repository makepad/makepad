/*
TeamTalk is a LAN (only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
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
    std::collections::HashMap
};

// We dont have a UI yet 

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
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
                global_volume = <Slider> {
                    padding: 0
                    height: Fit,
                    width: 125,
                    margin: {top: 1, left: 2}
                    text: "1344"
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveAtomic, LiveHook, LiveRead, LiveRegister)]
#[live_ignore]
pub struct Store{
    #[live(0.5f64)] global_volume: f64a,
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] store: Arc<Store>,
    #[rust] volume_changed_by_ui: SignalFromUI,
    #[rust] volume_changed_by_network: SignalToUI,
    #[rust] hue_light_change: ToUIReceiver<(usize,HueLight)>,
    #[rust] hue_light_last: HashMap<usize, HueLight>,
    #[rust] hue_light_set: HashMap<usize, HueLight>,
    #[rust] hue_poll: Timer,
}

#[derive(Debug, Clone, PartialEq)]
enum HueLight{
    Switch{on: bool},
    Color{on: bool, hue: f32, sat: f32, val: f32}
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
    }
}

impl MatchEvent for App{
    fn handle_midi_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        cx.use_midi_inputs(&ports.all_inputs());
    }
    
    fn handle_http_response(&mut self, cx:&mut Cx, request_id:LiveId, res:&HttpResponse){
        if request_id == live_id!(hue_fetch){
            self.handle_hue_lights(cx, res);
        }
    }
    
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
        self.start_artnet_client(cx);
        self.store_to_widgets(cx);
        self.fetch_hue_lights(cx);
        self.start_imu_forward(cx);
        self.start_forza_forward(cx);
        self.hue_poll = cx.start_interval(0.1);
    }
    
    fn handle_timer(&mut self, cx:&mut Cx, e:&TimerEvent){
        // lets remove ids out of the hue light set one at a time
        if self.hue_poll.is_timer(e).is_some(){
            if let Some(key) = self.hue_light_set.keys().next(){
                let key = key.clone();
                let light = self.hue_light_set.remove(&key).unwrap();
                // lets set the light
                let url = format!("https://{}/api/{}/lights/{}/state", HUE_BRIDGE, HUE_KEY, key);
                let mut request = HttpRequest::new(url, HttpMethod::PUT);
                request.set_header("Content-Type".to_string(), "application/json".to_string());
                match light{
                    HueLight::Color{on, hue, sat, val}=>{
                        let ws = format!("{{\"on\":{}, \"sat\":{}, \"bri\":{},\"hue\":{}}}",
                            on,
                            (sat*255.0) as u32,
                            (val*255.0) as u32,
                            (hue*65535.0) as u32
                        );
                        request.set_body(ws.as_bytes().to_vec());
                    }
                    HueLight::Switch{on}=>{
                        let ws = format!("{{\"on\":{}}}",
                        on,
                    );
                    request.set_body(ws.as_bytes().to_vec());
                    }
                }
                request.set_ignore_ssl_cert();
                cx.http_request(live_id!(hue_set), request);
            }
        };
    }
    
    fn handle_signal(&mut self, cx: &mut Cx){
        if self.volume_changed_by_network.check_and_clear(){
            self.store_to_widgets(cx);
        }
        // lets fetch the latest hue IDs
        while let Ok((id,data)) = self.hue_light_change.try_recv(){
            if self.hue_light_last.get(&id) != Some(&data){
                self.hue_light_set.insert(id, data.clone());
            }
            self.hue_light_last.insert(id, data);
        }
    }
    
    fn handle_audio_devices(&mut self, cx:& mut Cx, devices:&AudioDevicesEvent){
        for _desc in &devices.descs{
            //println!("{}", desc)
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
    Volume{client_uid: u64, volume: f64},
    Silence {client_uid: u64, frame_count: u32},
    Audio {client_uid: u64, channel_count: u32, data: Vec<i16>},
}

pub const DMXOUTPUT_HEADER: [u8;18] = [
    b'A',b'r',b't',b'-',b'N',b'e',b't',b'\0', 
    0, // opcode hi 
    0x50, // opcode lo = output
    0, // proto hi
    0xe, // proto lo = 14
    0, // sequence
    0, // physical 
    0,  // sub uni
    0,  // net
    2,  // buffer hi size (512)
    0   // buffer lo
];

// get a HUE key from here: https://developers.meethue.com/develop/get-started-2/
const HUE_KEY:&'static str = "Ay0O7saTTq3FNogyKhDwB8WWY7MdIyzeFzzsydRz";
const HUE_BRIDGE:&'static str = "10.0.0.104";

#[allow(unused)]
#[derive(DeBin)]
struct ForzaTelemetryDash{
    // = 1 when race is on. = 0 when in menus/race stopped …
    is_race_on: i32,
    // Can overflow to 0 eventually
    time_stamp_ms: u32,
    
    engine_max_rpm: f32,
    engine_idle_rpm: f32,
    current_engine: f32,
    // In the car's local space; X = right, Y = up, Z = forward
    acceleration_x: f32,
    acceleration_y: f32,
    acceleration_z: f32,
    // In the car's local space; X = right, Y = up, Z = forward
    velocity_x: f32,
    velocity_y: f32,
    velocity_z: f32,
    // In the car's local space; X = pitch, Y = yaw, Z = roll
    angular_velocity_x: f32,
    angular_velocity_y: f32,
    angular_velocity_z: f32,
    yaw: f32,
    pitch: f32,
    roll: f32,
    // Suspension travel normalized: 0.0f = max stretch; 1.0 = max compression
    normalized_suspension_travel_front_left: f32,
    normalized_suspension_travel_front_right: f32,
    normalized_suspension_travel_rear_left: f32,
    normalized_suspension_travel_rear_right: f32,
    // Tire normalized slip ratio, = 0 means 100% grip and |ratio| > 1.0 means loss of grip.
    tire_slip_ratio_front_left: f32,
    tire_slip_ratio_front_right: f32,
    tire_slip_ratio_rear_left: f32,
    tire_slip_ratio_rear_right: f32,
    // Wheels rotation speed radians/sec. 
    wheel_rotation_speed_front_left: f32,        
    wheel_rotation_speed_front_right: f32,        
    wheel_rotation_speed_rear_left: f32,        
    wheel_rotation_speed_rear_right: f32,       
    // = 1 when wheel is on rumble strip, = 0 when off.
    wheel_on_rumble_strip_front_left: i32, 
    wheel_on_rumble_strip_front_right: i32, 
    wheel_on_rumble_strip_rear_left:  i32, 
    wheel_on_rumble_strip_rear_right: i32, 
    // = from 0 to 1, where 1 is the deepest puddle
    wheel_in_puddle_depth_front_left: f32,
    wheel_in_puddle_depth_front_right: f32,
    wheel_in_puddle_depth_rear_left: f32,
    wheel_in_puddle_depth_rear_right: f32,
    // Non-dimensional surface rumble values passed to controller force feedback
    surface_rumble_front_left: f32,
    surface_rumble_front_right: f32,
    surface_rumble_rear_left: f32,
    surface_rumble_rear_right: f32,
    // Tire normalized slip angle, = 0 means 100% grip and |angle| > 1.0 means loss of grip.
    tire_slip_angle_front_left: f32,
    tire_slip_angle_front_right: f32,
    tire_slip_angle_rear_left: f32,
    tire_slip_angle_rear_right: f32,
    // Tire normalized combined slip, = 0 means 100% grip and |slip| > 1.0 means loss of grip.
    tire_combind_slip_front_left: f32,
    tire_combind_slip_front_right: f32,
    tire_combind_slip_rear_left: f32,
    tire_combind_slip_rear_right: f32,
    // Actual suspension travel in meters
    suspension_travel_meters_front_left: f32,
    suspension_travel_meters_front_right: f32,
    suspension_travel_meters_rear_left: f32,
    suspension_travel_meters_rear_right: f32,
    // Unique ID of the car make/model
    car_ordinal: i32,
    // Between 0 (D -- worst cars) and 7 (X class -- best cars) inclusive         
    car_class: i32,
    // Between 100 (worst car) and 999 (best car) inclusive
    car_performance_index: i32,
    // 0 = FWD, 1 = RWD, 2 = AWD
    drive_train_type: i32,
    // Number of cylinders in the engine
    num_cylinders: i32,
    position_x: f32,
    position_y: f32,
    position_z: f32,
    speed: f32,
    power: f32,
    torque: f32,
    tire_temp_front_left: f32,
    tire_temp_front_right: f32,
    tire_temp_rear_left: f32,
    tire_temp_rear_right: f32,
    boost: f32,
    fuel: f32,
    distance_traveled: f32,
    best_lap: f32,
    last_lap: f32,
    current_lap: f32,
    current_race_time: f32,
    
    lap_number: u16,
    race_position: u8,
    accel: u8,
    
    brake: u8,
    clutch: u8,
    hand_brake: u8,
    gear: u8,
    
    //steer: i8,
    normalizedf_driving_line: i8,
    normalized_ai_brake_difference: i8,
    
    tire_wear_front_left: f32,
    tire_wear_front_right: f32,
    tire_wear_rear_left: f32,
    //tire_wear_rear_right: f32,
    
    //track_ordinal: i32
}

impl App {
    pub fn start_imu_forward(&mut self, _cx:&mut Cx){
        // open up port udp X and forward packets to both wind + platform
        let imu_recv = UdpSocket::bind("0.0.0.0:44442").unwrap();
        std::thread::spawn(move || {
            let mut buffer = [0u8;25];
            while let Ok((length, _addr)) = imu_recv.recv_from(&mut buffer){
                 log!("IMU {:x?}",&buffer[0..length]);
            } 
        });
    }
    
    pub fn start_forza_forward(&mut self, _cx:&mut Cx){
        let wind_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let wind_send_addr = "10.0.0.202:44443";
        let platform_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let platform_send_addr = "10.0.0.126:51010";
                
        // open up port udp X and forward packets to both wind + platform
        let forca_recv = UdpSocket::bind("0.0.0.0:51010").unwrap();
        let buf = [0 as u8,]; 
        let _ = wind_socket.send_to(&buf, wind_send_addr);
        std::thread::spawn(move || {
            
            let mut buffer = [0u8;1024];
            while let Ok((length, _addr)) = forca_recv.recv_from(&mut buffer){
                let forza = ForzaTelemetryDash::deserialize_bin(&buffer[0..length]).unwrap();
                let speed = (forza.velocity_x*forza.velocity_x+forza.velocity_y*forza.velocity_y+forza.velocity_z*forza.velocity_z).sqrt();
                // ok so speed is 20.0 at 40mph
                // max fan is 127.0
                // lets say 100mph = 60 = 127.
                let buf = [(speed*2.2).min(255.0) as u8,];
                let _ = wind_socket.send_to(&buf, wind_send_addr);
                println!("SENDING TO {:?}", buf); 
                let _ = platform_socket.send_to(&buffer[0..length], platform_send_addr);
            }
        });
    }
    
    pub fn handle_hue_lights(&mut self, _cx:&mut Cx, res:&HttpResponse){
        if let Some(data) = res.get_string_body() {
            let value = JsonValue::deserialize_json(&data).unwrap();
            // lets push these ids into a vec
            let mut lights = Vec::new();
            for (id,light) in value.key("lights").object(){
                let id = id.parse::<u64>().unwrap();
                lights.push((id, light.key("name").string(), light.key("uniqueid").string()));
            }
            lights.sort_by(|a,b| a.0.cmp(&b.0));
            for (id, name, _unique) in lights{
                log!("Hue light {}: {}", id, name);
            }
        }
    }
    
    pub fn fetch_hue_lights(&mut self, cx:&mut Cx){
        // lets http request the hue bridge
        let url = format!("https://{}/api/{}", HUE_BRIDGE, HUE_KEY);
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_ignore_ssl_cert();
        cx.http_request(live_id!(hue_fetch), request);
    }
    
    pub fn start_artnet_client(&mut self, cx:&mut Cx){
        let socket = UdpSocket::bind("0.0.0.0:6454").unwrap();
        let broadcast_addr = "255.255.255.255:6454";
        socket.set_broadcast(true).unwrap();
        socket.set_read_timeout(Some(Duration::from_nanos(1))).unwrap();
        let mut buffer = [0u8; 2048];
        
        #[derive(Debug,Default,SerRon, DeRon)]
        struct State{ 
            fade:[f32;9],
            dial_a:[f32;8], 
            dial_b:[f32;8], 
            dial_c:[f32;8], 
        }
        #[derive(Debug,Clone,Default)]
        struct Buttons{
            mute:[bool;9],
            rec:[bool;9],
            solo:bool,
            bank_left:bool,
            bank_right:bool
        }
        impl Buttons{
            fn delta(old:&Buttons, new:&Buttons)->Self{
                let mut mute = [false;9];
                let mut rec = [false;9];
                for i in 0..9{
                    mute[i] = !old.mute[i] && new.mute[i];
                    rec[i] = !old.rec[i] && new.rec[i];
                }
                Self{
                    mute,
                    rec,
                    solo: !old.solo && new.solo,
                    bank_left: !old.bank_left && new.bank_left,
                    bank_right: !old.bank_right && new.bank_right
                }
            }
        }
        
        let mut state = State::default();
        
        if let Ok(result) = std::fs::read_to_string("dmx.ron"){
            if let Ok(load) = State::deserialize_ron(&result){
                state = load   
            }
        }
        // alright the sender thread where we at 44hz poll our midi input and set up a DMX packet
        let mut midi_input = cx.midi_input();
        let mut hue_sender = self.hue_light_change.sender();
        
        std::thread::spawn(move || {
            let mut universe = [0u8;DMXOUTPUT_HEADER.len() + 512];
                        
            let mut new_buttons = Buttons::default();
            let mut old_buttons = Buttons::default();
            
            fn map_color(val:f32, fade:f32)->Vec4{
                let colors = ["fff", "ff7", "f00","ff0","0f0","0ff","00f","f0f"];
                let len = (colors.len()-1) as f32;
                // pick where we are in between
                let a = (val * len).floor();
                let b = (val * len).ceil();
                let gap = val * len - a; 
                use makepad_platform::makepad_live_tokenizer::colorhex::hex_bytes_to_u32;
                let c1 = Vec4::from_u32(hex_bytes_to_u32(colors[a as usize].as_bytes()).unwrap());
                let c2 = Vec4::from_u32(hex_bytes_to_u32(colors[b as usize].as_bytes()).unwrap());
                let c = Vec4::from_lerp(c1, c2, gap) * fade;
                c
            }
            
            fn map_wargb(val:f32, fade:f32, out:&mut [u8], bases: &[usize]){
                let c = map_color(val, fade);
                for base in bases{
                    out[base-1] = (c.x * 255.0) as u8;
                    out[base+0] = (c.y * 255.0) as u8;
                    out[base+1] = (c.z * 255.0) as u8;
                }
            }
            
            fn hue_wargb(sender: &mut ToUISender<(usize, HueLight)>,val:f32, fade:f32, hueids: &[usize]){
                let c = map_color(val, fade);
                let c = c.to_hsva();
                for id in hueids{
                    let _ = sender.send((*id,HueLight::Color{
                        on: if fade>0.01{true}else{false},
                        hue: c.x,
                        sat: c.y,
                        val: c.z
                    }));
                }
            }
            
            fn hue_switch(sender: &mut ToUISender<(usize, HueLight)>, on:bool, hueids:&[usize]){
                for id in hueids{
                    let _ = sender.send((*id,HueLight::Switch{
                        on,
                    }));
                }
            }
            
            fn dmx_u8(val: u8, out:&mut[u8], bases:&[usize], chan:usize){
                for base in bases{
                    out[base - 1 + chan - 1] = val
                }
            }
            fn dmx_f32(val: f32, out:&mut[u8], bases:&[usize], chan:usize){
                for base in bases{
                    out[base - 1 + chan - 1] = (val *255.0).min(255.0).max(0.0) as u8
                }
            }
                        
            for i in 0..DMXOUTPUT_HEADER.len(){universe[i] = DMXOUTPUT_HEADER[i];}
            let mut counter = 0;
            let mut clock = 0.0f64;
            loop {
                while let Ok((_length, _addr)) = socket.recv_from(&mut buffer){
                    //log!("READ {:x?}",&buffer[0..length]);
                } 
                // lets poll midi
                while let Some((_port,data)) = midi_input.receive(){
                    match data.decode() {
                        MidiEvent::ControlChange(cc) => {
                            let v = cc.value as f32 / 127.0;
                            match cc.param{
                                16=>state.dial_a[0] = v,
                                17=>state.dial_b[0] = v,
                                18=>state.dial_c[0] = v,
                                19=>state.fade[0] = v,
                                20=>state.dial_a[1] = v,
                                21=>state.dial_b[1] = v,
                                22=>state.dial_c[1] = v,
                                23=>state.fade[1] = v,
                                24=>state.dial_a[2] = v,
                                25=>state.dial_b[2] = v,
                                26=>state.dial_c[2] = v,
                                27=>state.fade[2] = v,
                                28=>state.dial_a[3] = v,
                                29=>state.dial_b[3] = v,
                                30=>state.dial_c[3] = v,
                                31=>state.fade[3] = v,                               
                                46=>state.dial_a[4] = v,
                                47=>state.dial_b[4] = v,
                                48=>state.dial_c[4] = v,
                                49=>state.fade[4] = v, 
                                50=>state.dial_a[5] = v,
                                51=>state.dial_b[5] = v,
                                52=>state.dial_c[5] = v,
                                53=>state.fade[5] = v,
                                54=>state.dial_a[6] = v,
                                55=>state.dial_b[6] = v,
                                56=>state.dial_c[6] = v,
                                57=>state.fade[6] = v,
                                58=>state.dial_a[7] = v,
                                59=>state.dial_b[7] = v,
                                60=>state.dial_c[7] = v,
                                61=>state.fade[7] = v,
                                62=>state.fade[8] = v,
                                _=>{
                                    log!("{} {}", cc.param, cc.value);
                                }
                            }
                        }
                        MidiEvent::Note(n)=>match n.note_number{
                            48..=55=>new_buttons.mute[n.note_number as usize -48] = n.is_on,
                            56..=63=>new_buttons.rec[n.note_number as usize -56] = n.is_on,
                            25=>new_buttons.bank_left = n.is_on,
                            26=>new_buttons.bank_right = n.is_on,
                            27=>new_buttons.solo = n.is_on,
                            x=>{log!("{}",x)}
                        }
                        x=>log!("{:?}",x)
                    }
                }
                let buttons = Buttons::delta(&old_buttons,&new_buttons);
                old_buttons = new_buttons.clone();
                
                universe[12] = counter as u8;
                if counter > 255{ counter = 0}
                clock += 1.0/44.0;
                counter += 1;
                let dmx = &mut universe[DMXOUTPUT_HEADER.len()..];
                
                // alright so these things are now Hue ids
                // except we need to throttle them
                // and turn them into HSV values
                
                // RIGHT KITCHEN (A) - 3
                // RIGHT WINDOW (B) - 8
                // LEFT WINDOW (A) - 19
                // DINNER TABLE2 (A) - 22
                // DINNER TABLE3 (B) - 23
                // DINNER TABLE1 (C) - 24
                // DINNER TABLE4 (C) - 25
                // FRONT DOOR 23 (A) - 29
                // CENTER WINDOW (C) - 32
                // KITCHEN CENTER (C) - 33
                // KITCHEN LEFT (B) - 34
                // KITCHEN STRIP (C) - 38
                // DESK (B)  - 39
                // TABLE (B) - 40
                hue_wargb(&mut hue_sender, state.dial_c[0], state.fade[0]*state.fade[8], &[3, 19, 22, 29]);
                hue_wargb(&mut hue_sender, state.dial_c[1], state.fade[1]*state.fade[8], &[8, 23, 34, 40, 39]);
                hue_wargb(&mut hue_sender, state.dial_c[2], state.fade[2]*state.fade[8], &[24, 25, 32, 33, 38]);
                if buttons.mute[7]{
                    hue_switch(&mut hue_sender,true, &[41]);
                }
                else if buttons.rec[7]{
                    hue_switch(&mut hue_sender,false, &[41]);
                }
                
                if buttons.mute[6]{
                    hue_wargb(&mut hue_sender,0.0, 1.0, &[12,13]);
                }
                else if buttons.rec[6]{
                    hue_wargb(&mut hue_sender,0.0, 0.0, &[12,13]);
                }
                
                if buttons.mute[5]{
                    hue_wargb(&mut hue_sender,0.0, 1.0, &[18]);
                }
                else if buttons.rec[5]{
                    hue_wargb(&mut hue_sender,0.0, 0.0, &[18]);
                }                
                
                if buttons.mute[4]{
                    hue_switch(&mut hue_sender,true,  &[16,21,26]);
                }
                else if buttons.rec[4]{
                    hue_switch(&mut hue_sender,false,  &[16,21,26]); 
                }                 
                 
                map_wargb(state.dial_c[3], 1.0, dmx, &[110+2-1]); // RGB laser color
                // lets set the laser mode with the slider
                let rgb_laser_addr = 110;
                match (state.fade[3] * 3.0) as usize{
                    0=>{ // laser off
                        dmx_u8(0, dmx, &[rgb_laser_addr], 1);
                    }
                    1=>{ // laser on left
                        dmx_u8(255, dmx, &[rgb_laser_addr], 1);
                        dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
                        dmx_u8(32, dmx, &[rgb_laser_addr], 7);
                    }
                    2=>{ // laser on right
                        dmx_u8(255, dmx, &[rgb_laser_addr], 1);
                        dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
                        dmx_u8(32, dmx, &[rgb_laser_addr], 7);
                    }
                    _=>{}
                }
                // overload the other laser onto the this laser
                let rgb_laser_addr = 110;
                map_wargb(state.dial_c[3], 1.0, dmx, &[rgb_laser_addr+2-1]); // RGB laser color
                match (state.fade[3] * 4.0) as usize{
                    0=>{ // laser off
                        dmx_u8(0, dmx, &[rgb_laser_addr], 1);
                    }
                    1=>{ // laser on left
                        dmx_u8(255, dmx, &[rgb_laser_addr], 1);
                        dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
                        dmx_u8(32, dmx, &[rgb_laser_addr], 7);
                    }
                    2=>{ // laser on right
                        dmx_u8(255, dmx, &[rgb_laser_addr], 1);
                        dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
                        dmx_u8(32, dmx, &[rgb_laser_addr], 7);
                    }
                    3=>{
                        dmx_u8(0, dmx, &[rgb_laser_addr], 1);
                    }
                    _=>{} 
                }
                let multi_fx_addr = 100;
                dmx_f32((state.fade[3]-0.5).max(0.0)*2.0, dmx, &[multi_fx_addr], 3);
                dmx_f32(state.fade[4], dmx, &[multi_fx_addr], 1);
                dmx_f32(state.fade[4], dmx, &[multi_fx_addr], 2);
                dmx_f32(state.dial_c[4], dmx, &[multi_fx_addr], 4);
                let rgb_strobe = 120;
                
                map_wargb(state.dial_c[5], state.fade[5], dmx, &[rgb_strobe+3-1]); // Strobe RGB
                dmx_f32(1.0, dmx, &[rgb_strobe], 1);
                dmx_f32(1.0-(state.fade[5].max(0.5).min(1.0)-0.5)*2.0, dmx, &[rgb_strobe], 10);
                
                dmx_f32(state.fade[6]*10.0, dmx, &[rgb_strobe], 6);
                dmx_f32(state.fade[6], dmx, &[rgb_strobe], 8);
                dmx_f32(state.dial_b[0], dmx, &[rgb_strobe], 7);
                dmx_f32(state.dial_b[1], dmx, &[rgb_strobe], 11);
                dmx_f32(state.dial_b[2], dmx, &[rgb_strobe], 9);
                dmx_f32(state.dial_b[3], dmx, &[rgb_strobe], 13);
                                
                // and finally the moving head
                let spot1 = 200;
                let spot2 = 250;
                dmx_f32(state.fade[7], dmx, &[spot1, spot2], 6);
                dmx_f32(state.dial_a[0], dmx, &[spot1], 1);
                dmx_f32(state.dial_a[0], dmx, &[ spot2], 1);
                dmx_f32(state.dial_a[1], dmx, &[spot1, spot2], 3);
                dmx_f32(state.dial_a[2], dmx, &[spot1, spot2], 14); 
                map_wargb(state.dial_a[3], 1.0, dmx, &[spot1+16-1, spot2+16-1]); // Strobe RGB
                
                dmx_f32(state.dial_a[4], dmx, &[spot1, spot2], 12);
                
                dmx_f32(state.dial_a[5], dmx, &[spot1, spot2], 13);
                dmx_f32(state.dial_a[6], dmx, &[spot1, spot2], 10);
                                                
                dmx_f32(state.dial_a[7], dmx, &[spot1, spot2], 8);
                
                // smoke machine
                let smoke = 300;
                // ok so depending on the state of c_[7] we do a percentage of a 
                let slot = 101.0f64;
                let needed = slot * state.dial_c[7] as f64;
                let t = clock.rem_euclid(slot);
                if t < needed{
                    dmx_f32(1.0, dmx, &[smoke], 1);
                }
                else{
                    dmx_f32(0.0, dmx, &[smoke], 1);
                }
                // in time modulus 
                let smoke2 = 310;
                dmx_f32(state.dial_b[7], dmx, &[smoke2], 1);
                dmx_f32(state.dial_b[6], dmx, &[smoke2], 2);
                //let buf = [(state.dial_b[7]*255.0) as u8, (state.dial_b[6]*255.0) as u8, (state.dial_b[5]*255.0) as u8];
                //let _ = rc_car_socket.send_to(&buf, rc_car_send_addr);
                
                                
                //map_wargb(state.dial[7], 1.0, dmx, &[spot + 16 - 1]); // Strobe RGB
                //dmx_f32(state.fade[7], dmx, &[spot], 6);
                                
                // alright so we want dial 
                // alright slider 4 = laser mode +RGB dial
                // slider 5 = matrix / uv mode
                // slider 6 = strobe white - slider = speed, dial =  mode
                // slider 7 = strobe RGB  - slider = mode, dial = color
                // slider 8 = moving head mode dial + thing
                
                // alright lets send out this thing \
                socket.send_to(&universe, broadcast_addr).unwrap();
                
                std::fs::write("dmx.ron", state.serialize_ron().as_bytes()).unwrap();
                
                //socket.send(&universe, broadcast_add.into());
                // lets sleep 1/44th of a second
                std::thread::sleep(Duration::from_secs_f64(1.0/44.0))
            }
        });
    }

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
        let volume_changed_by_ui = self.volume_changed_by_ui.clone();
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
                    if volume_changed_by_ui.check_and_clear(){
                        wire_data.clear();
                        TeamTalkWire::Volume{client_uid:my_client_uid, volume: store.global_volume.get()}.ser_bin(&mut wire_data);
                        write_audio.send_to(&wire_data, "255.255.255.255:41531").unwrap();
                    }
                    let wire_packet = if peak>0.005 {
                        TeamTalkWire::Audio {client_uid:my_client_uid, channel_count: 1, data: output_buffer.to_i16()}
                    }
                    else {
                        TeamTalkWire::Silence {client_uid:my_client_uid, frame_count: output_buffer.frame_count() as u32}
                    };
                    // serialise the packet enum for sending over the wire
                    wire_data.clear();
                    wire_packet.ser_bin(&mut wire_data);
                    // send to all peers
                    write_audio.send_to(&wire_data, "255.255.255.255:41531").unwrap();
                };
            }
        });
        let volume_changed_by_network = self.volume_changed_by_network.clone();
        let store = self.store.clone();
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
                    TeamTalkWire::Volume{client_uid, volume}=>{
                        if client_uid != my_client_uid{
                            store.global_volume.set(volume);
                            volume_changed_by_network.set();
                        }
                        continue
                    }
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