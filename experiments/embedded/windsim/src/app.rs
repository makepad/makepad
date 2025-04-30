/*
TeamTalk is a LAN (only) p2p audiochat supporting as many clients as you have bandwidth.
For 6 clients it should pull about 25 megabits. You can use it to have a super low latency
helicopter-headset experience, silent disco, and so on.
*/

use { 
    crate::{
        makepad_micro_serde::*,
        makepad_widgets::*,
        makepad_platform::live_atomic::*,
    },
    std::sync::Arc,
    std::net::UdpSocket,
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
                slider1 = <Slider> {
                    padding: 0
                    height: Fit,
                    width: 125,
                    margin: {top: 1, left: 2}
                    text: "Wind"
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveAtomic, LiveHook, LiveRead, LiveRegister)]
#[live_ignore]
pub struct Store{
    #[live(0.5f64)] slider1: f64a,
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] store: Arc<Store>,
    #[rust] slider_changed_by_ui: SignalFromUI,
}


impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

impl App{
    pub fn store_to_widgets(&self, cx:&mut Cx){
        let db = DataBindingStore::from_nodes(self.store.live_read());
        Self::data_bind_map(db.data_to_widgets(cx, &self.ui));
    }
    
    pub fn data_bind_map(mut db: DataBindingMap) {
        db.bind(id!(slider1), ids!(slider1));
    }
}

impl MatchEvent for App{
   
    
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        let mut db = DataBindingStore::new();
        db.data_bind(cx, actions, &self.ui, Self::data_bind_map);
        self.store.apply_over(cx, &db.nodes);
        if db.contains(id!(slider1)){
            self.slider_changed_by_ui.set();
        }
    }
    
    fn handle_startup(&mut self,  cx: &mut Cx){
        self.store_to_widgets(cx);
        self.start_forza_forward(cx);
        self.start_am2_forward(cx);
    }
}
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
} 


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

#[allow(unused)]
#[derive(DeBin)]
struct PjCars1Telem{
    build_version: u16,
    seq_packet: u8,
    game_session_state: u8,
    viewed_participant_index: i8,
    num_participants: i8,
    unfiltered_throttle: u8,
    unfiltered_brake: u8,
    unfiltered_steering: i8,
    unfiltered_clutch: u8,
    race_state_flags: u8,
    laps_in_event: u8,
    best_lap_time: f32,
    last_lap_time: f32,
    current_time: f32,
    split_time_ahead: f32,
    split_time_behind: f32,
    split_time: f32,
    event_time_remaining: f32,
    personal_fastest_lap_time: f32,
    world_fastest_lap_time: f32,
    current_sector1_time: f32,
    current_sector2_time: f32,
    current_sector3_time: f32,
    fastest_sector1_time: f32,
    fastest_sector2_time: f32,
    fastest_sector3_time: f32,
    personal_fastest_sector1_time: f32,
    personal_fastest_sector2_time: f32,
    personal_fastest_sector3_time: f32,
    world_fastest_sector1_time: f32,
    world_fastest_sector2_time: f32,
    world_fastest_sector3_time: f32,          
    joy_pad: u16,
    highest_flag: u8,
    pit_mode_schedule: u8,
    oil_temp_celsius: i16,
    oil_pressure_kpa: u16,
    water_temp_celcius: i16,
    water_pressure_kpa: u16,
    fuel_pressure_kpa: u16,
    car_flags: u8,
    fuel_capacity: u8,
    brake: u8,
    throttle: u8,
    clutch: u8,
    steering: i8,
    fuel_level: f32,
    speed: f32,
    rpm: u16,
    max_rpm: u16,
    gear_num_gears: u8,
    boost_amount: u8,
    enforced_pit_stop_lap: i8,
    crash_sxtate: u8,
    odometer_km: f32,
    orientation_x: f32,
    orientation_y: f32,
    orientation_z: f32,
    local_velocity_x: f32,
    local_velocity_y: f32,
    local_velocity_z: f32,
    world_velocity_x: f32,
    world_velocity_y: f32,
    world_velocity_z: f32,
    angular_velocity_x: f32,
    angular_velocity_y: f32,
    angular_velocity_z: f32,
    local_acceleration_x: f32,
    local_acceleration_y: f32,
    local_acceleration_z: f32,
    world_acceleration_x: f32,
    world_acceleration_y: f32,
    world_acceleration_z: f32,
    extents_center_x: f32,
    extents_center_y: f32,
    extents_center_z: f32,
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
    
    pub fn start_am2_forward(&mut self, _cx:&mut Cx){
        let wind_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let wind_send_addr = "10.0.0.202:44443";
        
        // open up port udp X and forward packets to both wind + platform
        let am2_recv = UdpSocket::bind("0.0.0.0:5606").unwrap();
        let buf = [0 as u8,]; 
        let _ = wind_socket.send_to(&buf, wind_send_addr);

        std::thread::spawn(move || {
            let mut buffer = [0u8;2048];
            loop{
                while let Ok((length, _addr)) = am2_recv.recv_from(&mut buffer){
                    if let Ok(pjc) = PjCars1Telem::deserialize_bin(&buffer[0..length]){
                        println!("GOT SPEED {}", pjc.speed);
                        let buf = [(pjc.speed*1.8).min(255.0) as u8,];
                        let _ = wind_socket.send_to(&buf, wind_send_addr);
                    }
                }
            }
        });
    }
}