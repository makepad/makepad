use std::{
	fs::File,
	os::fd::AsRawFd,
	sync::{mpsc::Sender,
		Arc,
		Mutex,
	},
	thread::JoinHandle,
	thread,
	io::Read,
	time::Instant,
	cell::Cell,
	ffi::CStr,
};

use crate::{event::*,
	libc_sys,
	window::WindowId,
	makepad_math::*,
	KeyModifiers,
	area::Area,
};

use super::{input_sys::*,
	direct_event::*,
};

#[repr(C)]
#[derive(Default, Clone, Copy)]
///One linux InputEvent, a larger event group consists of multiple of these, ending in one with ty: EV_SYN and code SYN_REPORT
pub struct InputEvent {
    pub time: libc_sys::timeval,
    pub ty: InputEventType,
    pub code: u16,
    pub value: i32,
}

impl std::fmt::Debug for InputEvent {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self.ty {
			InputEventType::EV_SYN => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvSynCodes>(self.code) }, self.value),
			InputEventType::EV_KEY => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvKeyCodes>(self.code) }, self.value),
			InputEventType::EV_REL => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvRelCodes>(self.code) }, self.value),
			InputEventType::EV_ABS => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvAbsCodes>(self.code) }, self.value),
			InputEventType::EV_MSC => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvMscCodes>(self.code) }, self.value),
			InputEventType::EV_SW  => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvSwCodes>(self.code) }, self.value),
			InputEventType::EV_LED => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvLedCodes>(self.code) }, self.value),
			InputEventType::EV_SND => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvSndCodes>(self.code) }, self.value),
			InputEventType::EV_REP => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, unsafe{ std::mem::transmute::<u16, EvRepCodes>(self.code) }, self.value),
			_ => write!(f, "time: {:?}\ntype: {:?}\ncode: {:?}\nvalue: {:?}", self.time, self.ty, self.code, self.value),
		}
	}
}

#[repr(C)]
#[derive(Debug,Default,Clone)]
///Information about an EV_ABS code
pub struct InputAbsInfo {
	value: i32,
	minimum: i32,
	maximum: i32,
	fuzz: i32,
	flat: i32,
	resolution: i32,
}

#[derive(Debug)]
///Holds the important information about an Input device and runs the thread to read events from it.
pub struct InputDevice {
	///The event file of the device
	fd: File,
	///Time that the App was launched
	time_start: Instant,
	///Name of the device (unused right now)
	name: String,
	///Holds the bitfields for the device properties see input-event-codes.h
	property_bits: [u8;number_of_bytes(InputProperty::Count as u16)],
	///Holds the bitfields for the devices possible event types see input-event-codes.h
	event_bits: [u8;number_of_bytes(InputEventType::EV_CNT as u16)],
	///Holds the bitfields for the devices possible key event codes see input-event-codes.h
	key_bits: [u8;number_of_bytes(EvKeyCodes::KEY_CNT as u16)],
	///Holds the bitfields for the devices possible rel event codes see input-event-codes.h
	rel_bits: [u8;number_of_bytes(EvRelCodes::REL_CNT as u16)],
	///Holds the bitfields for the devices possible abs event codes see input-event-codes.h
	abs_bits: [u8;number_of_bytes(EvAbsCodes::ABS_CNT as u16)],
	///Holds the bitfields for the devices possible led event codes see input-event-codes.h
	led_bits: [u8;number_of_bytes(EvLedCodes::LED_CNT as u16)],
	///Holds the bitfields for the devices possible misc event codes see input-event-codes.h
	misc_bits: [u8;number_of_bytes(EvMscCodes::MSC_CNT as u16)],
	///Holds the bitfields for the devices possible switch event codes see input-event-codes.h
	sw_bits: [u8;number_of_bytes(EvSwCodes::SW_CNT as u16)],
	///Holds the bitfields for the devices possible rep event codes see input-event-codes.h
	rep_bits: [u8;number_of_bytes(EvRepCodes::REP_CNT as u16)],
	///Holds the bitfields for the devices possible ff event codes see input-event-codes.h
	ff_bits: [u8;number_of_bytes(EvFfCodes::FF_CNT as u16)],
	///Holds the bitfields for the devices possible snd event codes see input-event-codes.h
	snd_bits: [u8;number_of_bytes(EvSndCodes::SND_CNT as u16)],
	///Holds the info for every abs event code see input-event-codes.h
	abs_info: Vec<InputAbsInfo>,
	///Info about the current touches on this device
	touches: Vec<TouchPoint>,
	///Number of fingers currently on the touch surface
	num_fingers: usize,
	///Finger number thats being read from
	current_slot: usize,
	///Sender to send events back to the event loop
	sender: Sender<Vec<DirectEvent>>,
	///Shared between event threads, holds the absolute cursor position
	abs: Arc<Mutex<DVec2>>,
	///Shared between event threads, holds the window size/dpi_factor
	window: Arc<DVec2>,
	///Shared between event threads, holds the key modifiers
	modifiers: Arc<Mutex<KeyModifiers>>,
	///The screen dpi factor
	dpi_factor: Arc<f64>,
	///The makepad window id
	window_id: WindowId,
}

impl InputDevice {
	///Spawn a new InputDevice and start running a thread to read events from it.
	pub fn new(file: File, sender: Sender<Vec<DirectEvent>>, time_start: Instant, abs: Arc<Mutex<DVec2>>, window: Arc<DVec2>, modifiers: Arc<Mutex<KeyModifiers>>, dpi_factor: Arc<f64>, window_id: WindowId) -> JoinHandle<()>{
		let mut dev = InputDevice { 
			fd: file,
			time_start,
			name: String::default(),
			property_bits: [0u8;number_of_bytes(InputProperty::Count as u16)],
			event_bits: [0u8;number_of_bytes(InputEventType::EV_CNT as u16)],
			key_bits: [0u8;number_of_bytes(EvKeyCodes::KEY_CNT as u16)],
			rel_bits: [0u8;number_of_bytes(EvRelCodes::REL_CNT as u16)],
			abs_bits: [0u8;number_of_bytes(EvAbsCodes::ABS_CNT as u16)],
			led_bits: [0u8;number_of_bytes(EvLedCodes::LED_CNT as u16)],
			misc_bits: [0u8;number_of_bytes(EvMscCodes::MSC_CNT as u16)],
			sw_bits: [0u8;number_of_bytes(EvSwCodes::SW_CNT as u16)],
			rep_bits: [0u8;number_of_bytes(EvRepCodes::REP_CNT as u16)],
			ff_bits: [0u8;number_of_bytes(EvFfCodes::FF_CNT as u16)],
			snd_bits: [0u8;number_of_bytes(EvSndCodes::SND_CNT as u16)],
			abs_info: Vec::new(),
			touches: Vec::new(),
			num_fingers: 0,
			current_slot: 0,
			sender,
			abs,
			window,
			modifiers,
			dpi_factor,
			window_id,
		};
		let mut name_buff = [0u8;256];
		unsafe {
			//get available event types, it seems that it is impossible to get the available sync codes, it gives the event types instead.
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_SYN, dev.event_bits.len()), dev.event_bits.as_mut_ptr());
			//get the device properties if available
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCPROP(dev.property_bits.len()), dev.property_bits.as_mut_ptr());
			//get the available relative codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_REL, dev.rel_bits.len()), dev.rel_bits.as_mut_ptr());
			//get the available absolute codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_ABS, dev.abs_bits.len()), dev.abs_bits.as_mut_ptr());
			//get the available led codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_LED, dev.led_bits.len()), dev.led_bits.as_mut_ptr());
			//get the available key codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_KEY, dev.key_bits.len()), dev.key_bits.as_mut_ptr());
			//get the available switch codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_SW, dev.sw_bits.len()), dev.sw_bits.as_mut_ptr());
			//get the available misc codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_MSC, dev.misc_bits.len()), dev.misc_bits.as_mut_ptr());
			//get the available force feedback codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_FF, dev.ff_bits.len()), dev.ff_bits.as_mut_ptr());
			//get the available sound codes
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGBIT(InputEventType::EV_SND, dev.snd_bits.len()), dev.snd_bits.as_mut_ptr());
			//get the name of the device
			let _ = ioctl(dev.fd.as_raw_fd(), EVIOCGNAME(name_buff.len() - 1), name_buff.as_mut_ptr());
			dev.name = CStr::from_bytes_until_nul(&name_buff).unwrap().to_str().unwrap().to_string();
			//get all the abs info fields
			if dev.has_event_type(InputEventType::EV_ABS)	{
				dev.abs_info = vec![InputAbsInfo::default();EvAbsCodes::ABS_CNT as usize];
				for abs_code in 0..EvAbsCodes::ABS_MAX as usize {
					if Self::is_bit_set(&dev.abs_bits, abs_code as u16) {
						ioctl(dev.fd.as_raw_fd(), EVIOCGABS(abs_code as u16), dev.abs_info.get_mut(abs_code).unwrap());
					}
				}
			}
		}
		println!("{} connected",dev.name);
		thread::spawn(move || {
			loop {
				let mut evts: Vec<InputEvent> = Vec::new();
				loop {
					if let Ok(evt) = dev.read_input_event() {
						match evt.ty {
							InputEventType::EV_SYN => {
								let code: EvSynCodes = unsafe { std::mem::transmute_copy(&evt.code) };
								match code {
									EvSynCodes::SYN_REPORT => { //end of event reached send event through the channel, break and make a new buffer
										evts.push(evt);
										let dir_evts = dev.process_event_group(evts);
										dev.sender.send(dir_evts).unwrap();
										break;
									},
									EvSynCodes::SYN_DROPPED => { //evdev client buffer overrun, ignore event till now and up untill the next SYN_REPORT
										evts.clear();
										while let Ok(dropped) = dev.read_input_event() {
											match dropped.ty {
												InputEventType::EV_SYN => {
													if dropped.code == EvSynCodes::SYN_REPORT as u16 {
														break;
													}
												},
												_ => continue
											}
										}
									},
									_ => evts.push(evt)
								}
							},
							_ => {
								evts.push(evt);
							}
						}
					} else {
						println!("{} disconnected",dev.name);
						return
					}
				}
			}
		})
	}

	///Check if a bit is set in a large number that is represented as an array
	fn is_bit_set(bitfield: &[u8], bit: u16) -> bool {
		(bitfield[bit as usize / 8] & (1u8 << (bit % 8))) > 0
	}

	///Check if the device is capable of reading a specific event code of a specific event type
	fn has_event_code(&self, evt_type: InputEventType, evt_code: u16) -> bool {
		if self.has_event_type(evt_type) {
			match evt_type {
				InputEventType::EV_SYN => return true,
				InputEventType::EV_KEY => Self::is_bit_set(&self.key_bits, evt_code),
				InputEventType::EV_REL => Self::is_bit_set(&self.rel_bits, evt_code),
				InputEventType::EV_ABS => Self::is_bit_set(&self.abs_bits, evt_code),
				InputEventType::EV_MSC => Self::is_bit_set(&self.misc_bits, evt_code),
				InputEventType::EV_SW  => Self::is_bit_set(&self.sw_bits, evt_code),
				InputEventType::EV_LED => Self::is_bit_set(&self.led_bits, evt_code),
				InputEventType::EV_SND => Self::is_bit_set(&self.snd_bits, evt_code),
				InputEventType::EV_REP => Self::is_bit_set(&self.rep_bits, evt_code),
				InputEventType::EV_FF  => Self::is_bit_set(&self.ff_bits, evt_code),
				_ => return false,
			}
		} else {
			false
		}
	}

	///Check if the device is capable of receiving a certain event type
	fn has_event_type(&self, evt_type: InputEventType) -> bool {
		Self::is_bit_set(&self.event_bits, evt_type as u16)
	}

	///Check if the device has a certain property
	fn has_property(&self, prop: InputProperty) -> bool {
		Self::is_bit_set(&self.property_bits, prop as u16)
	}

	///Check if the device is a multitouch type B device
	fn is_mt_type_b(&self) -> bool {
		self.has_event_code(InputEventType::EV_ABS, EvAbsCodes::ABS_MT_SLOT as u16)
	}

	///Check if the absolute input is directly related to position of the screen (touchscreen for example)
	fn is_direct(&self) -> bool {
		self.has_property(InputProperty::Direct)
	}

	///Check if the absolute input is relatively related to the position of the pointer on the screen (touchpad for example)
	fn is_pointer(&self) -> bool {
		self.has_property(InputProperty::Pointer)
	}

	///Check what the time is since the start of the application in seconds
	fn time_now(&self) -> f64 {
		let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_secs_f64()
	}

	///Read one InputEvent from the event file
	fn read_input_event(&mut self) -> Result<InputEvent, ()> {
        let mut buf = [0u8; std::mem::size_of::<InputEvent>()];
        loop {
            match self.fd.read_exact(&mut buf) { //read exact to get rid of the length check that was here
                Ok(()) => {
                    let buf: InputEvent = unsafe {std::mem::transmute(buf)};
                    return Ok(buf)
                },
                Err(err) => {
                    match err.kind() {
                        std::io::ErrorKind::UnexpectedEof => {
                            continue;
                        },
                        _ => return Err(())

                    }
                }
            }
        }
    }

	///Process a group of InputEvents ending in an EV_SYN:SYN_REPORT
	fn process_event_group(&mut self, evts: Vec<InputEvent>) -> Vec<DirectEvent> {
		let time = self.time_now(); //Cant use the event timeval_t unfortunately because it seems to use system time, while rust Instant seems to use time since boot.
		let mut dir_evts = Vec::new();
		for evt in evts {
            match evt.ty {
                InputEventType::EV_REL => { // relative input
                    self.process_rel_event(evt, &mut dir_evts, time);
                }
                InputEventType::EV_ABS => { // absolute input
                    self.process_abs_event(evt, &mut dir_evts, time);
                }
                InputEventType::EV_KEY => { // key press
                    self.process_key_event(evt, &mut dir_evts, time);
                }
				InputEventType::EV_SYN => {
					self.process_syn_event(evt, &mut dir_evts, time);
				}
                    _ => ()
            };
        };
		dir_evts
	}

	///Process a synchronisation event
	fn process_syn_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64){
		let code: EvSynCodes = unsafe { std::mem::transmute(evt.code) };
		match code {
			EvSynCodes::SYN_REPORT => { //finish up the event.
				if !self.is_mt_type_b() { //received empty touch event of type A all fingers are gone
					if self.current_slot > self.num_fingers {
						for touch in self.touches.iter_mut() {
							touch.state = TouchState::Stop;
						}
					}
					else if self.touches.len() > self.num_fingers {
						let len = self.touches.len();
						for touch in self.touches.get_mut(self.num_fingers..len).unwrap().iter_mut() {
							touch.state = TouchState::Stop;
						}
					}
				}
				//if there are any fingers on the screen make a TouchUpdateEvent
				if self.touches.len() > 0 {
					dir_evts.push(DirectEvent::TouchUpdate(TouchUpdateEvent {
						time,
						window_id: self.window_id,
						modifiers: self.modifiers.lock().unwrap().clone(),
						touches: self.touches.clone(), //TODO this is pretty bad and should be fixed but dont know how (yet)
					}))
				};
				//if all fingers left the screen clear the buffer so it doesn't get sent to the event handler
				if self.num_fingers == 0 {
					self.touches.clear()
				};
				if !self.is_mt_type_b() {
					self.num_fingers = 0; //start counting from 0 on the next event for type A multitouch.
					self.current_slot = 0;
				}
			},
			EvSynCodes::SYN_MT_REPORT => {
				self.current_slot +=1;
			},
			_=> ()
		};
	}

	///Process a relative input event
    fn process_rel_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64){
        let code: EvRelCodes = unsafe { std::mem::transmute(evt.code) };
        match code {
            EvRelCodes::REL_X => {
				let mut abs = self.abs.lock().unwrap();
                abs.x += evt.value as f64;
                if abs.x < 0.0{ abs.x = 0.0}
                if abs.x > self.window.x{ abs.x = self.window.x}
                
            },
            EvRelCodes::REL_Y => {
            	let mut abs = self.abs.lock().unwrap();
				abs.y += evt.value as f64;
                if abs.y < 0.0{ abs.y = 0.0}
                if abs.y > self.window.y{ abs.y = self.window.y}
            },
            _ => return ()
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
            abs: self.abs.lock().unwrap().clone(),
            window_id: self.window_id,
            modifiers: self.modifiers.lock().unwrap().clone(),
            time,
            handled: Cell::new(Area::Empty),
        }));
    }

	///Process an absolute input event TODO implement touchpad using the self.is_pointer() property.
    fn process_abs_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64){
        let code: EvAbsCodes = unsafe { std::mem::transmute(evt.code) };
        static mut FIRST_TOUCH_X: bool = false;
        static mut FIRST_TOUCH_Y: bool = false;
        match code {
            EvAbsCodes::ABS_X => {
				let mut abs = self.abs.lock().unwrap();
                abs.x = (evt.value as f64 / 32767.0) * self.window.x;
            },

            EvAbsCodes::ABS_Y => {
				let mut abs = self.abs.lock().unwrap();
                abs.y = (evt.value as f64 / 32767.0) * self.window.y;
            },

            EvAbsCodes::ABS_MT_POSITION_X => {
				if !self.is_mt_type_b() {
					self.num_fingers +=1; //Type A will always send X and Y, but we only need to increment num fingers once, so we do it on X
				}
				if let Some(touch) = self.touches.get_mut(self.current_slot){
					touch.abs.x = evt.value as f64 / self.dpi_factor.as_ref();
					if unsafe {!FIRST_TOUCH_X} {
						touch.state = TouchState::Move;

					} else {
						unsafe { FIRST_TOUCH_X = false }
					}
				} else { //MT Type A could start with an absolute x, in which case a touchpoint might not yet exist
					self.touches.push(TouchPoint {
						state: TouchState::Start,
						abs: DVec2 { x: evt.value as f64 / self.dpi_factor.as_ref(), y: 0.0 },
						uid: 0,
						rotation_angle: 0.0,
						force: 0.0,
						radius: DVec2 { x: 0.0, y: 0.0 },
						handled: Cell::new(Area::Empty),
						sweep_lock: Cell::new(Area::Empty)
					});
					unsafe { FIRST_TOUCH_Y = true }
				}
            },

            EvAbsCodes::ABS_MT_POSITION_Y => {
				if let Some(touch) = self.touches.get_mut(self.current_slot) {
					touch.abs.y = evt.value as f64 / self.dpi_factor.as_ref();
					if unsafe {!FIRST_TOUCH_Y} {
						touch.state = TouchState::Move;
					} else {
						unsafe { FIRST_TOUCH_Y = false }
					}
				} else {
					self.touches.push(TouchPoint {
						state: TouchState::Start,
						abs: DVec2 { x: 0.0, y: evt.value as f64 / self.dpi_factor.as_ref() },
						uid: 0,
						rotation_angle: 0.0,
						force: 0.0,
						radius: DVec2 { x: 0.0, y: 0.0 },
						handled: Cell::new(Area::Empty),
						sweep_lock: Cell::new(Area::Empty)
					});
					unsafe { FIRST_TOUCH_X = true }
				}
            },

            EvAbsCodes::ABS_MT_TRACKING_ID => { //new finger shows up or is removed
				if self.is_mt_type_b() {
					if evt.value>=0 { //new finger id is assigned
						unsafe {
							FIRST_TOUCH_X = true;
							FIRST_TOUCH_Y = true; 
						}
						if self.current_slot == self.touches.len() { //new touch is needed
							self.touches.push(TouchPoint {
								state: TouchState::Start,
								abs: DVec2 { x: 0.0, y: 0.0 },
								uid: evt.value as u64,
								rotation_angle: 0.0,
								force: 0.0,
								radius: DVec2 { x: 0.0, y: 0.0 },
								handled: Cell::new(Area::Empty),
								sweep_lock: Cell::new(Area::Empty)
							})
						} else { //old touch can be reused
							if let Some(index) = self.touches.iter().position(|touch| touch.state == TouchState::Stop) {
								*self.touches.get_mut(index).unwrap() = TouchPoint {
									state: TouchState::Start,
									abs: DVec2 { x: 0.0, y: 0.0 },
									uid: evt.value as u64,
									rotation_angle: 0.0,
									force: 0.0,
									radius: DVec2 { x: 0.0, y: 0.0 },
									handled: Cell::new(Area::Empty),
									sweep_lock: Cell::new(Area::Empty),
								};
							}
						}
						self.num_fingers += 1;
					} else { //finger is removed
						self.num_fingers -= 1;
						self.touches.get_mut(self.current_slot).expect(MTSLOTERROR).state = TouchState::Stop;
					}
				}	
            },

			EvAbsCodes::ABS_MT_SLOT => { //change touch index to track
				self.current_slot = evt.value as usize;
			},

			EvAbsCodes::ABS_PRESSURE => {
				self.touches.get_mut(self.current_slot).expect(MTSLOTERROR).force = evt.value as f64;
			}
            _=> return ()
        }
        dir_evts.push(DirectEvent::MouseMove(MouseMoveEvent {
            abs: self.abs.lock().unwrap().clone(),
            window_id: self.window_id,
            modifiers: self.modifiers.lock().unwrap().clone(),
            time,
            handled: Cell::new(Area::Empty),
        }))
    }

	fn process_key_event(&mut self, evt: InputEvent, dir_evts: &mut Vec<DirectEvent>, time: f64){
        let code: EvKeyCodes = unsafe { std::mem::transmute(evt.code) };
        let key_action: KeyAction = unsafe {std::mem::transmute(evt.value) };
        let key_code = match code {
            EvKeyCodes::KEY_ESC => KeyCode::Escape,
            EvKeyCodes::KEY_1 => KeyCode::Key1,
            EvKeyCodes::KEY_2 => KeyCode::Key2,
            EvKeyCodes::KEY_3 => KeyCode::Key3,
            EvKeyCodes::KEY_4 => KeyCode::Key4,
            EvKeyCodes::KEY_5 => KeyCode::Key5,
            EvKeyCodes::KEY_6 => KeyCode::Key6,
            EvKeyCodes::KEY_7 => KeyCode::Key7,
            EvKeyCodes::KEY_8 => KeyCode::Key8,
            EvKeyCodes::KEY_9 => KeyCode::Key9,
            EvKeyCodes::KEY_0 => KeyCode::Key0,
            EvKeyCodes::KEY_MINUS => KeyCode::Minus,
            EvKeyCodes::KEY_EQUAL => KeyCode::Equals,
            EvKeyCodes::KEY_BACKSPACE => KeyCode::Backspace,
            EvKeyCodes::KEY_TAB => KeyCode::Tab,
            EvKeyCodes::KEY_Q => KeyCode::KeyQ,
            EvKeyCodes::KEY_W => KeyCode::KeyW,
            EvKeyCodes::KEY_E => KeyCode::KeyE,
            EvKeyCodes::KEY_R => KeyCode::KeyR,
            EvKeyCodes::KEY_T => KeyCode::KeyT,
            EvKeyCodes::KEY_Y => KeyCode::KeyY,
            EvKeyCodes::KEY_U => KeyCode::KeyU,
            EvKeyCodes::KEY_I => KeyCode::KeyI,
            EvKeyCodes::KEY_O => KeyCode::KeyO,
            EvKeyCodes::KEY_P => KeyCode::KeyP,
            EvKeyCodes::KEY_LEFTBRACE => KeyCode::LBracket,
            EvKeyCodes::KEY_RIGHTBRACE => KeyCode::RBracket,
            EvKeyCodes::KEY_ENTER => KeyCode::ReturnKey,
            EvKeyCodes::KEY_LEFTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_A => KeyCode::KeyA,
            EvKeyCodes::KEY_S => KeyCode::KeyS,
            EvKeyCodes::KEY_D => KeyCode::KeyD,
            EvKeyCodes::KEY_F => KeyCode::KeyF,
            EvKeyCodes::KEY_G => KeyCode::KeyG,
            EvKeyCodes::KEY_H => KeyCode::KeyH,
            EvKeyCodes::KEY_J => KeyCode::KeyJ,
            EvKeyCodes::KEY_K => KeyCode::KeyK,
            EvKeyCodes::KEY_L => KeyCode::KeyL,
            EvKeyCodes::KEY_SEMICOLON => KeyCode::Semicolon,
            EvKeyCodes::KEY_APOSTROPHE => KeyCode::Quote,
            EvKeyCodes::KEY_GRAVE => KeyCode::Backtick,
            EvKeyCodes::KEY_LEFTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_BACKSLASH => KeyCode::Backslash,
            EvKeyCodes::KEY_Z => KeyCode::KeyZ,
            EvKeyCodes::KEY_X => KeyCode::KeyX,
            EvKeyCodes::KEY_C => KeyCode::KeyC,
            EvKeyCodes::KEY_V => KeyCode::KeyV,
            EvKeyCodes::KEY_B => KeyCode::KeyB,
            EvKeyCodes::KEY_N => KeyCode::KeyN,
            EvKeyCodes::KEY_M => KeyCode::KeyM,
            EvKeyCodes::KEY_COMMA => KeyCode::Comma,
            EvKeyCodes::KEY_DOT => KeyCode::Period,
            EvKeyCodes::KEY_SLASH => KeyCode::Slash,
            EvKeyCodes::KEY_RIGHTSHIFT => KeyCode::Shift,
            EvKeyCodes::KEY_KPASTERISK => KeyCode::NumpadMultiply,
            EvKeyCodes::KEY_LEFTALT => KeyCode::Alt,
            EvKeyCodes::KEY_SPACE => KeyCode::Space,
            EvKeyCodes::KEY_CAPSLOCK => KeyCode::Capslock,
            EvKeyCodes::KEY_F1 => KeyCode::F1,
            EvKeyCodes::KEY_F2 => KeyCode::F2,
            EvKeyCodes::KEY_F3 => KeyCode::F3,
            EvKeyCodes::KEY_F4 => KeyCode::F4,
            EvKeyCodes::KEY_F5 => KeyCode::F5,
            EvKeyCodes::KEY_F6 => KeyCode::F6,
            EvKeyCodes::KEY_F7 => KeyCode::F7,
            EvKeyCodes::KEY_F8 => KeyCode::F8,
            EvKeyCodes::KEY_F9 => KeyCode::F9,
            EvKeyCodes::KEY_F10 => KeyCode::F10,
            EvKeyCodes::KEY_NUMLOCK => KeyCode::Numlock,
            EvKeyCodes::KEY_SCROLLLOCK => KeyCode::ScrollLock,
            EvKeyCodes::KEY_KP7 => KeyCode::Numpad7,
            EvKeyCodes::KEY_KP8 => KeyCode::Numpad8,
            EvKeyCodes::KEY_KP9 => KeyCode::Numpad9,
            EvKeyCodes::KEY_KPMINUS => KeyCode::NumpadSubtract,
            EvKeyCodes::KEY_KP4 => KeyCode::Numpad4,
            EvKeyCodes::KEY_KP5 => KeyCode::Numpad5,
            EvKeyCodes::KEY_KP6 => KeyCode::Numpad6,
            EvKeyCodes::KEY_KPPLUS => KeyCode::NumpadAdd,
            EvKeyCodes::KEY_KP1 => KeyCode::Numpad1,
            EvKeyCodes::KEY_KP2 => KeyCode::Numpad2,
            EvKeyCodes::KEY_KP3 => KeyCode::Numpad3,
            EvKeyCodes::KEY_KP0 => KeyCode::Numpad0,
            EvKeyCodes::KEY_KPDOT => KeyCode::NumpadDecimal,
            EvKeyCodes::KEY_ZENKAKUHANKAKU => KeyCode::Unknown,
            EvKeyCodes::KEY_102ND => KeyCode::Backtick, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_F11 => KeyCode::F11,
            EvKeyCodes::KEY_F12 => KeyCode::F12,
            EvKeyCodes::KEY_RO => KeyCode::NumpadDivide, //Seems odd but this was in the code this replaced
            EvKeyCodes::KEY_KPENTER => KeyCode::NumpadEnter,
            EvKeyCodes::KEY_RIGHTCTRL => KeyCode::Control,
            EvKeyCodes::KEY_KPSLASH => KeyCode::NumpadDivide,
            EvKeyCodes::KEY_SYSRQ => KeyCode::PrintScreen,
            EvKeyCodes::KEY_RIGHTALT => KeyCode::Alt,
            EvKeyCodes::KEY_HOME => KeyCode::Home,
            EvKeyCodes::KEY_UP => KeyCode::ArrowUp,
            EvKeyCodes::KEY_PAGEUP => KeyCode::PageUp,
            EvKeyCodes::KEY_LEFT => KeyCode::ArrowLeft,
            EvKeyCodes::KEY_RIGHT => KeyCode::ArrowRight,
            EvKeyCodes::KEY_END => KeyCode::End,
            EvKeyCodes::KEY_DOWN => KeyCode::ArrowDown,
            EvKeyCodes::KEY_PAGEDOWN => KeyCode::PageDown,
            EvKeyCodes::KEY_INSERT => KeyCode::Insert,
            EvKeyCodes::KEY_DELETE => KeyCode::Delete,
            EvKeyCodes::KEY_LEFTMETA => KeyCode::Logo,
            EvKeyCodes::KEY_RIGHTMETA => KeyCode::Logo,
            _ => KeyCode::Unknown,
        };
        match key_action {
            KeyAction::KeyDown => {
                match key_code {
                    KeyCode::Shift => self.modifiers.lock().unwrap().shift = true,
                    KeyCode::Control => self.modifiers.lock().unwrap().control = true,
                    KeyCode::Logo => self.modifiers.lock().unwrap().logo = true,
                    KeyCode::Alt => self.modifiers.lock().unwrap().alt = true,
                    _ => ()
                };
                match code {
                    EvKeyCodes::BTN_LEFT | EvKeyCodes::BTN_RIGHT | EvKeyCodes::BTN_MIDDLE => {
                        dir_evts.push(DirectEvent::MouseDown(MouseDownEvent {
                            button: (evt.code - EvKeyCodes::BTN_LEFT as u16) as usize,
                            abs: self.abs.lock().unwrap().clone(),
                            window_id: self.window_id,
                            modifiers: self.modifiers.lock().unwrap().clone(),
                            time,
                            handled: Cell::new(Area::Empty),
                        }))
                    },
                    _ => {
						let modifiers = self.modifiers.lock().unwrap();
                        if !modifiers.control && !modifiers.alt && !modifiers.logo {
                            let uc = modifiers.shift;
                            let inp = key_code.to_char(uc);
                            if let Some(inp) = inp {
                                dir_evts.push(DirectEvent::TextInput(TextInputEvent {
                                    input: format!("{}", inp),
                                    was_paste: false,
                                    replace_last: false
                                }));
                            }
                        }
                        dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                            key_code,
                            is_repeat: false,
                            modifiers: modifiers.clone(),
                            time,
                        }))
                    }
                }
                
            },
            KeyAction::KeyUp => {
                match key_code {
                    KeyCode::Shift => self.modifiers.lock().unwrap().shift = false,
                    KeyCode::Control => self.modifiers.lock().unwrap().control = false,
                    KeyCode::Logo => self.modifiers.lock().unwrap().logo = false,
                    KeyCode::Alt => self.modifiers.lock().unwrap().alt = false,
                    _ => ()
                };
                match code {
                    EvKeyCodes::BTN_LEFT | EvKeyCodes::BTN_RIGHT | EvKeyCodes::BTN_MIDDLE => {
                        dir_evts.push(DirectEvent::MouseUp(MouseUpEvent {
                            button: (evt.code - EvKeyCodes::BTN_LEFT as u16) as usize,
                            abs: self.abs.lock().unwrap().clone(),
                            window_id: self.window_id,
                            modifiers: self.modifiers.lock().unwrap().clone(),
                            time,
                        }))
                    },
					EvKeyCodes::BTN_TOUCH => {
						self.touches.get_mut(self.current_slot).unwrap().state = TouchState::Stop;
						
					},
                    _ => {
                        dir_evts.push(DirectEvent::KeyUp(KeyEvent {
                            key_code,
                            is_repeat: false,
                            modifiers: self.modifiers.lock().unwrap().clone(),
                            time
                        }))
                    }
                }
            },
            KeyAction::KeyRepeat => {
                dir_evts.push(DirectEvent::KeyDown(KeyEvent {
                    key_code,
                    is_repeat: false,
                    modifiers: self.modifiers.lock().unwrap().clone(),
                    time
                }))
            }
        }
    }
}

///Check the number of bytes needed to hold the number of bits
const fn number_of_bytes(value: u16) -> usize {
	if (value % 8) > 0 {
		return (value / 8 + 1) as usize;
	} else {
		return (value / 8) as usize;
	}
}