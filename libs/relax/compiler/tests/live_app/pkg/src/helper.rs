// A value of it lives in a static (LAST): a field added to it takes the restart rung, the step
// that checks the state-preserving restart. (A struct no running value holds takes its new
// layout live instead: reload.rs no_live_values.)
pub struct Conf {
    pub base: u64,
}

pub static LAST: std::sync::Mutex<Option<Conf>> = std::sync::Mutex::new(None);

pub fn conf() -> Conf {
    Conf { base: 1 }
}

pub fn value(_n: u64) -> u64 {
    *LAST.lock().unwrap() = Some(conf());
    conf().base
}
