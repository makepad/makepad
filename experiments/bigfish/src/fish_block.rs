use makepad_widgets::DVec2;
use crate::makepad_platform::*;
use crate::fish_block_template::*;
use crate::fish_param_storage::*;
use crate::fish_ports::*;
use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishBlock {
    pub id: u64,
    pub library_id: u64,
    pub x: f64,
    pub y: f64,
    pub h: f64,
    pub w: f64,
    pub name: String,
    pub category: FishBlockCategory,
    pub block_type: String,
    pub parameters: Vec<FishParamStorage>,
    pub input_ports: Vec<FishInputPortInstance>,
    pub output_ports: Vec<FishOutputPortInstance>,
}

impl FishBlock {
    pub fn _create_test_block(id: u64) -> FishBlock {
        let mut block = FishBlock::default();
        block.block_type = String::from(format!("BlockType {:?}", id));
        block.id = id;

        block
    }

    pub fn reload_from_string(&mut self, serialized: &str) {
        *self = FishBlock::deserialize_ron(serialized).expect("deserialize a block");
    }

    pub fn get_output_instance(&self, id: u64) -> Option<&FishOutputPortInstance> {
        self.output_ports.iter().find(|&x| x.id == id)
    }
    pub fn get_input_instance(&self, id: u64) -> Option<&FishInputPortInstance> {
        self.input_ports.iter().find(|&x| x.id == id)
    }

    pub fn is_in_rect(&self, start: DVec2, end: DVec2) -> bool {
        let block_rect = Rect {pos: DVec2{x: self.x as f64,y: self.y as f64}, size: DVec2{ x:self.w as f64, y: self.h as f64}};
        let containerrect = Rect{pos: start, size: end - start};
        if block_rect.intersects(containerrect) || block_rect.inside(containerrect) {return true};
        return  false;

    }
}
