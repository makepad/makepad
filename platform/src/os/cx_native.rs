use {crate::cx::Cx, std::time::SystemTime};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EventFlow {
    Poll,
    Wait,
    Exit,
}

// lets start a websocket thread

impl Cx {
    pub fn time_now() -> f64 {
        if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            return elapsed.as_secs_f64();
        }
        return 0.0;
    }
}
