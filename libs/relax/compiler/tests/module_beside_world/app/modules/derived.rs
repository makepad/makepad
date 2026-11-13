#[derive(mbw_app::Answer)]
pub struct InModule;

pub fn answer() -> u32 {
    InModule::answer()
}
