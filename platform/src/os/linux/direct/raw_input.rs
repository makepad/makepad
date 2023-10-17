use std::sync::atomic::{
    AtomicBool,
    AtomicUsize
};

use {
    self::super::{
        direct_event::*,
        input_device::InputDevice,
    },
    crate::{
        makepad_math::*,
        window::WindowId,
        event::*,
    },
    std::{
        fs::File,
        sync::{
            Arc,
            Mutex
        },
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
    let dir_entries = fs::read_dir("/dev/input/").and_then(|d| {
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

    dir_entries.into_iter().filter(|path| {
        path.file_name().unwrap().to_str().unwrap().starts_with("event")
    }).collect()
}

#[derive(Debug)]
pub struct RawInput {
    ///Shared between event threads, holds the absolute cursor position
    pub abs: Mutex<DVec2>,
    ///Shared between event threads, holds the window size/dpi_factor
    pub window: DVec2,
    ///Shared between event threads, holds the key modifiers
    pub modifiers: Mutex<KeyModifiers>,
    ///Shared between event threads, whether caps lock is active
    pub caps_lock: AtomicBool,
    ///The screen dpi factor
    pub dpi_factor: f64,
    ///Amount of ponter devices
    pub num_pointers: AtomicUsize,
    ///Starting time of the app
    pub time_start: Instant,
    ///Makepad window id
    pub window_id: WindowId,
    ///Event que
    pub direct_events: Mutex<Vec<DirectEvent>>,
}

impl RawInput {
    pub fn new(width: f64, height: f64, dpi_factor: f64, time_start: Instant, window_id: WindowId) -> Arc<Self> {
        let input_state = Arc::new(RawInput {
            abs: Mutex::new(dvec2(0.0, 0.0)),
            window: dvec2(width, height),
            modifiers: Mutex::new(KeyModifiers::default()),
            caps_lock: false.into(),
            dpi_factor,
            num_pointers: 0.into(),
            time_start,
            window_id,
            direct_events: Mutex::new(Vec::new()),
        });
        let raw_in_reference = input_state.clone();
        std::thread::spawn(move || { //main input thread that scans for changes in the input devices (new devices)
            println!("input devices:");
            for event_file in get_event_files() {
                if let Ok(kb) = File::open(event_file) {
                    InputDevice::new(kb,
                        input_state.clone(),
                    );
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
                            if let Ok(kb) = File::open(format!("/dev/input/{}",event.name.unwrap().to_str().unwrap())) {
                                InputDevice::new(kb,
                                    input_state.clone(),
                                );
                            }
                        }
                    }
                }
            }
        });
        raw_in_reference
    }

    pub fn has_pointer(&self) -> bool {
        self.num_pointers.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    ///Check what the time is since the start of the application in seconds
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_secs_f64()
    }
}

