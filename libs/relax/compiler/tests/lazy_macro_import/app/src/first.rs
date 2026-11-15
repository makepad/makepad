use lmi_dep::lmi_dep;
use lmi_dep::sync::Mutex;

static COUNT: Mutex<u32> = Mutex::new(1);

#[lmi_dep]
pub fn bump() -> u32 {
    let mut count = COUNT.lock().unwrap();
    *count += 1;
    *count
}
