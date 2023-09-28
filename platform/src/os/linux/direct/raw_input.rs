use {
    self::super::{direct_event::*,
		input_device::InputDevice,
	},
    crate::{
        makepad_math::*,
        window::WindowId,
        event::*,
    },
    std::{
        fs::File,
        sync::{mpsc, Arc,Mutex},
        path::PathBuf,
        fs,
		time::Instant,
    },
    inotify::{
        EventMask,
        WatchMask,
        Inotify,
    },
};

fn get_event_files() -> Vec<PathBuf> {
    let dirs = fs::read_dir("/dev/input/").and_then(|d| {
        d.map(|e| {
            e.map(|e| {
                if e.file_type().unwrap().is_file() {
                    PathBuf::from(e.path().file_name().unwrap())
                } else {
                    e.path()
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()
    }).unwrap();

    dirs.into_iter().filter(|path| {
        path.file_name().unwrap().to_str().unwrap().starts_with("event")
    }).collect()
}

pub struct RawInput {
    receiver: mpsc::Receiver<Vec<DirectEvent>>,
}

impl RawInput {
    pub fn new(width: f64, height: f64, dpi_factor: f64, time_start: Instant, window_id: WindowId) -> Self {
        let (send, receiver) = mpsc::channel();
        let send = send.clone();
		let abs_position = Arc::new(Mutex::new(dvec2(0.0, 0.0)));
		let window = Arc::new(dvec2(width, height));
		let modifiers = Arc::new(Mutex::new(KeyModifiers::default()));
		let dpi = Arc::new(dpi_factor);
        std::thread::spawn(move || { //main input thread that scans for changes in the input devices (new devices)
            for event_file in get_event_files() {
                let send = send.clone();
                if let Ok(kb) = File::open(event_file) {
					InputDevice::new(kb, send, time_start.clone(), abs_position.clone(), window.clone(), modifiers.clone(), dpi.clone(), window_id);
                }
            }

			let mut file_watcher = Inotify::init().unwrap();
            file_watcher
                .watches()
                .add(
                    "/dev/input/",
                    WatchMask::CREATE,
                )
                .unwrap();
            let mut buffer = [0u8; 4096];
            loop {
                let events = file_watcher
                    .read_events_blocking(&mut buffer)
                    .expect("Failed to read inotify events");   

                for event in events {
                    if event.mask.contains(EventMask::CREATE) {
                        if !event.mask.contains(EventMask::ISDIR) {
                            let send = send.clone();
                            if let Ok(kb) = File::open(format!("/dev/input/{}",event.name.unwrap().to_str().unwrap())) {
								InputDevice::new(kb, send, time_start.clone(), abs_position.clone(), window.clone(), modifiers.clone(), dpi.clone(), window_id);
                            }
                        }
                    }
                }
            }
        });
        
        Self {
            receiver,
        }
    }
    
    pub fn poll_raw_input(&self) -> Vec<DirectEvent> {
        let mut dir_evts: Vec<DirectEvent> = Vec::new();
        while let Ok(mut new) = self.receiver.try_recv() {
            dir_evts.append(&mut new);
        }
        dir_evts
    }
}

