use crate::fish_block_template::*;
use crate::fish_param_storage::*;
use crate::fish_ports::*;
use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishBlock {
    pub id: u64,
    pub library_id: u64,
    pub x: i32,
    pub y: i32,
    pub category: FishBlockCategory,
    pub block_type: String,
    pub parameters: Vec<FishParamStorage>,
    pub input_ports: Vec<FishInputPortInstance>,
    pub output_ports: Vec<FishOutputPortInstance>,
}

impl FishBlock {
    pub fn create_test_block(id: u64) -> FishBlock {
        let mut block = FishBlock::default();
        block.block_type = String::from(format!("BlockType {:?}", id));
        block.id = id;

        block
    }

    pub fn get_output_instance(&self, id: u64) -> Option<&FishOutputPortInstance> {
        self.output_ports.iter().find(|&x| x.id == id)
    }
    pub fn get_input_instance(&self, id: u64) -> Option<&FishInputPortInstance> {
        self.input_ports.iter().find(|&x| x.id == id)
    }
}
