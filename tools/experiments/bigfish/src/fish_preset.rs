use crate::fish_param_storage::*;

use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishPreset
{
    pub id: i32,
    pub name: String,
    pub patch: String,
    pub tags: Vec<String>,
    pub values: Vec<FishParamStorage>
}

impl FishPreset
{
    pub fn create_test_preset(id: i32) -> FishPreset
    {
        let mut preset = FishPreset::default();
        preset.name = String::from(format!("Preset {:?}", id));
        preset.id = id;
        

        preset
    }
}