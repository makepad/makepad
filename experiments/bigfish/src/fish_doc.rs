use crate::fish_block_template::FishBlockLibrary;
use crate::fish_patch::*;
use crate::makepad_micro_serde::*;

use std::fs;

#[derive(Clone, Debug, SerRon, DeRon)]

pub struct FishDoc {
    pub name: String,
    pub lib: FishBlockLibrary,
    pub patches: Vec<FishPatch>,
}
impl Default for FishDoc {
    fn default() -> Self {
        let m = FishDoc {
            name: String::new(),
            patches: vec![],
            lib: FishBlockLibrary::default(),
        };
        m
    }
}

impl FishDoc {
    pub fn save(&self, filename: &str) -> Result<(), String> {
        let docdata = self.serialize_ron();
        let _ = fs::write(filename, docdata)
            .map_err(|err| format!("failed to write to {:?} - {:?}", filename, err));
        log!("saved to {:?}", filename);
        Ok(())
    }

    pub fn undo(&mut self) -> Result<(), String> {
        self.patches[0].undo(&self.lib);

        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), String> {
        self.patches[0].redo(&self.lib);
        Ok(())
    }

    pub fn add_block(&mut self) -> Result<(), String> {
        self.patches[0].add_block(&self.lib);
        Ok(())
    }

    pub fn load(&mut self, filename: &str) -> Result<(), String> {
        let docdata = fs::read_to_string(filename)
            .map_err(|_| format!("Failed to load {:?}", filename))
            .unwrap();
        *self = FishDoc::deserialize_ron(&docdata)
            .map_err(|_| format!("failed to deserialize {:?}", filename))
            .unwrap();
            log!("loaded from {:?}", filename);
        Ok(())
    }

    pub fn create_test_doc() -> FishDoc {
        let mut doc = FishDoc::default();
        doc.lib.populate_library(&String::from("./"));

        doc.patches.push(FishPatch::create_test_patch(1, &doc.lib));
        doc.patches.push(FishPatch::create_test_patch(2, &doc.lib));
        doc.patches.push(FishPatch::create_test_patch(3, &doc.lib));
        doc.patches.push(FishPatch::create_test_patch(4, &doc.lib));

        doc
    }
}
