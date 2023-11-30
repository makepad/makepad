use crate::fish_block_template::FishBlockLibrary;
use crate::fish_patch::*;
use crate::makepad_micro_serde::*;

use std::fs;


#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishDoc{
    pub name: String,
    pub patches: Vec<FishPatch>,
}

impl FishDoc
{
    pub fn save(&self, filename: &str) ->Result<(), String>
    {
        let docdata = self.serialize_ron();
        let _ =  fs::write(filename, docdata).map_err(|err| format!("failed to write to {:?} - {:?}", filename, err));
        println!("saved to {:?}", filename);
        Ok(())
    }

    pub fn load(&mut self, filename: &str)->Result<(), String>
    {
        let docdata = fs::read_to_string(filename).map_err(|_| format!("Failed to load {:?}", filename)).unwrap(); 
        *self = FishDoc::deserialize_ron(&docdata).map_err(|_| format!("failed to deserialize {:?}", filename)).unwrap();
        println!("loaded from {:?}", filename);
        Ok(())
    }

    pub fn create_test_doc() -> FishDoc
    {
        let mut doc =  FishDoc::default();
        let mut lib = FishBlockLibrary::default();
        lib.populate_library(&String::from("./"));

        doc.patches.push(FishPatch::create_test_patch(1,&lib));
        doc.patches.push(FishPatch::create_test_patch(2,&lib));
        doc.patches.push(FishPatch::create_test_patch(3,&lib));
        doc.patches.push(FishPatch::create_test_patch(4,&lib));

        doc
    }
}