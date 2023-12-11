use crate::fish_block::*;
use crate::fish_block_template::*;
use crate::fish_connection::*;
use crate::fish_preset::*;

use crate::makepad_micro_serde::*;
use makepad_widgets::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishPatch {
    pub id: i32,
    pub name: String,
    pub presets: Vec<FishPreset>,
    pub blocks: Vec<FishBlock>,
    pub connections: Vec<FishConnection>,
}

impl FishPatch {
    pub fn connect(&mut self, blockfrom: u64, outputfrom: u64, blockto: u64, intputto: u64) {
        self.connections.push(FishConnection {
            id: 0,
            from_block: blockfrom,
            to_block: blockto,
            from_port: outputfrom,
            to_port: intputto,
        });
    }

    pub fn get_block(&self, id: u64) -> Option<&FishBlock> {
        self.blocks.iter().find(|&x| x.id == id)
    }

    pub fn move_block(&mut self, id: u64, dx: f64, dy: f64) {
        let g = self.blocks.iter_mut().find(|x| x.id == id).unwrap();

        //let g = self.get_block(id).unwrap();
        g.x += dx as i32;
        g.y += dy as i32;
    }

    pub fn create_block(&mut self, lib: &FishBlockLibrary, name: String, x: i32, y: i32) {
        let mut b = lib.create_instance_from_template(&name);
        b.x = x;
        b.y = y;
        b.id = LiveId::unique().0;

        self.blocks.push(b);
    }

    pub fn create_test_patch(id: i32, lib: &FishBlockLibrary) -> FishPatch {
        let mut patch = FishPatch::default();
        patch.name = String::from(format!("Test Patch {:?}", id));
        patch.id = id;

        let mut i = 0;

        patch.create_block(
            lib,
            String::from("Oscillator"),
            i % 3 * 300,
            i / 3 * 300 + 100,
        );

        i = i + 1;
        patch.create_block(lib, String::from("Filter"), i % 3 * 300, i / 3 * 300 + 100);

        i = i + 1;
        patch.create_block(lib, String::from("Effect"), i % 3 * 300, i / 3 * 300 + 100);

        i = i + 1;
        patch.create_block(lib, String::from("Meta"), i % 3 * 300, i / 3 * 300 + 100);

        i = i + 1;
        patch.create_block(
            lib,
            String::from("Envelope"),
            i % 3 * 300,
            i / 3 * 300 + 100,
        );

        i = i + 1;
        patch.create_block(
            lib,
            String::from("Modulator"),
            i % 3 * 300,
            i / 3 * 300 + 100,
        );
        i = i + 1;

        patch.create_block(lib, String::from("Utility"), i % 3 * 300, i / 3 * 300 + 100); //i=i+1;

        for i in 0..7 {
            patch.presets.push(FishPreset::create_test_preset(i));
        }
        for i in 0..7 {
            let fromidx = i as usize % patch.blocks.len();
            let toidx = (i as usize + 1) % patch.blocks.len();
            let fromblock = &patch.blocks[fromidx];
            let toblock = &patch.blocks[toidx];
            let fromport = (i as usize + 3) % fromblock.output_ports.len();
            let toport = (i as usize + 4) % toblock.input_ports.len();

            patch.connect(
                fromblock.id,
                fromblock.output_ports[fromport].id,
                toblock.id,
                toblock.input_ports[toport].id,
            );
        }

        /*  for i in 0..7 {
            let fromidx = i as usize % patch.blocks.len();
            let toidx = (i as usize + 5) % patch.blocks.len();
            let fromblock = &patch.blocks[fromidx];
            let toblock = &patch.blocks[toidx];
            let fromport = (i as usize + 4) % fromblock.output_ports.len();
            let toport = (i as usize + 5) % toblock.input_ports.len();

            patch.connect(
                fromblock.id,
                fromblock.output_ports[fromport].id,
                toblock.id,
                toblock.input_ports[toport].id,
            );
        }*/

        patch
    }
}
