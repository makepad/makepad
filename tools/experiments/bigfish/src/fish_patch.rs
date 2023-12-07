

use crate::fish_block_template::*;
use crate::fish_preset::*;
use crate::fish_block::*;
use crate::fish_connection::*;

use crate::makepad_micro_serde::*;
use makepad_widgets::*;


#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishPatch
{
    pub id: i32,
    pub name: String,
    pub presets: Vec<FishPreset>,
    pub blocks: Vec<FishBlock>,
    pub connections: Vec<FishConnection> 
}

impl  FishPatch{


    pub fn connect(&mut self, blockfrom:u64, outputfrom: u64, blockto: u64, intputto: u64)
    {
        self.connections.push( 
            FishConnection {
                id: 0, 
                from_block: blockfrom,
                to_block: blockto,
                from_port: outputfrom,
                to_port: intputto                 
                    }
        );
    }

    pub fn get_block(&self, id: u64) ->Option<&FishBlock>
    {
       self.blocks.iter().find(|&x| x.id == id)
    }

    pub fn create_block(&mut self, lib: &FishBlockLibrary, name: String, x: i32, y: i32) 
    {
        let mut b = lib.create_instance_from_template(&name);
        b.x = x;
        b.y = y;
        b.id = LiveId::unique().0;

        self.blocks.push(b);

    }

    pub fn create_test_patch(id: i32, lib: &FishBlockLibrary) -> FishPatch
    {
        let mut patch = FishPatch::default();
        patch.name = String::from(format!("Test Patch {:?}", id));
        patch.id = id;
    
        let mut i =0 ;
    
        patch.create_block(lib, String::from("Oscillator"), i%3*300, i/3*300+ 100);i=i+1;
        patch.create_block(lib, String::from("Filter"), i%3*300, i/3*300+ 100);i=i+1;
        patch.create_block(lib, String::from("Effect"), i%3*300, i/3*300+ 100);i=i+1;
        patch.create_block(lib, String::from("Meta"), i%3*300, i/3*300+100);i=i+1;
        patch.create_block(lib, String::from("Envelope"), i%3*300, i/3*300+100);i=i+1;
        patch.create_block(lib, String::from("Modulator"), i%3*300, i/3*300+100);i=i+1;
    
    
        patch.create_block(lib, String::from("Utility"), i%3*300, i/3*300 + 100);//i=i+1;

        for i in 0..20{
            patch.presets.push(FishPreset::create_test_preset(i));
            let fromblock = &patch.blocks[(i as usize) % patch.blocks.len()];
            let toblock = &patch.blocks[(i as usize+1)% patch.blocks.len()];
            
            patch.connect(fromblock.id,0, toblock.id,  0);
        }
      
        patch
    }
}