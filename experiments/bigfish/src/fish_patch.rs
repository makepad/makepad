use crate::fish_block::*;
use crate::fish_block_template::*;
use crate::fish_connection::*;
use crate::fish_preset::*;

use crate::makepad_micro_serde::*;
use makepad_widgets::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub enum UndoableThing {
    #[default]
    OpenMarker,
    CloseMarker,
    Block,
    Connection,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub enum IdAction {
    #[default]
    Nop,
    Modify {
        id: u64,
    },
    Delete {
        id: u64,
    },
    Create {
        id: u64,
    },
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct UndoState {
    pub undo_things: Vec<(UndoableThing, IdAction, String)>,
    pub redo_things: Vec<(UndoableThing, IdAction, String)>,
    pub undo_level: usize,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishPatch {
    pub id: u64,
    pub name: String,
    pub presets: Vec<FishPreset>,
    pub blocks: Vec<FishBlock>,
    pub connections: Vec<FishConnection>,
    pub creationid: u64,
    pub undo: UndoState,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishPatchSelection
{
    pub blocks: Vec<u64>
}

impl FishPatchSelection {
    pub fn add(&mut self, id: u64) {
        if self.blocks.contains(&id) {
            return;
        }
        self.blocks.push(id);
    }

    pub fn clear(&mut self)    {
        self.blocks.clear();
    }
}
pub trait FindBlock {
    fn find_mut(&mut self, id: u64) -> Option<&mut FishBlock>;
    fn find(&self, id: u64) -> Option<&FishBlock>;
}

impl FindBlock for Vec<FishBlock> {
    fn find_mut(&mut self, id: u64) -> Option<&mut FishBlock> {
        self.iter_mut().find(|x| x.id == id)
    }
    fn find(&self, id: u64) -> Option<&FishBlock> {
        self.iter().find(|x| x.id == id)
    }
}

pub trait FindConnection {
    fn find_mut(&mut self, id: u64) -> Option<&mut FishConnection>;
    fn find(&self, id: u64) -> Option<&FishConnection>;
}

impl FindConnection for Vec<FishConnection> {
    fn find_mut(&mut self, id: u64) -> Option<&mut FishConnection> {
        self.iter_mut().find(|x| x.id == id)
    }
    fn find(&self, id: u64) -> Option<&FishConnection> {
        self.iter().find(|x| x.id == id)
    }
}

impl FishPatch {

    pub fn select_rectangle(&mut self, start: DVec2, end:DVec2) -> FishPatchSelection {
        let mut blocks = Vec::new();
        for block in self.blocks.iter() {
            if block.is_in_rect(start, end) {
                blocks.push(block.id);
            }
        }
        FishPatchSelection { blocks }
    }

    pub fn connect(&mut self, blockfrom: u64, outputfrom: u64, blockto: u64, intputto: u64) {
        // todo: check if connection exists
        self.undo_checkpoint_start();
        let id = self.get_new_id();
        self.connections.push(FishConnection {
            id: id,
            from_block: blockfrom,
            to_block: blockto,
            from_port: outputfrom,
            to_port: intputto,
        });
        self.undo_checkpoint_end();
    }
    pub fn get_new_id(&mut self) -> u64{
        self.creationid = self.creationid + 1;
        self.creationid
    }
    pub fn undo_checkpoint_start(&mut self) -> usize {
        self.undo.redo_things.clear();

        self.undo
            .undo_things
            .push((UndoableThing::OpenMarker, IdAction::Nop, String::new()));

        self.undo.undo_level = self.undo.undo_level + 1;
        self.undo.undo_level
    }

    pub fn undo_checkpoint_end(&mut self) {
        self.undo
            .undo_things
            .push((UndoableThing::CloseMarker, IdAction::Nop, String::new()));
        self.undo.undo_level = self.undo.undo_level - 1;
    }

    pub fn undo_checkpoint_end_if_match(&mut self, undolevel: usize) -> usize {
        if self.undo.undo_level == undolevel {
            self.undo
                .undo_things
                .push((UndoableThing::CloseMarker, IdAction::Nop, String::new()));
            self.undo.undo_level = self.undo.undo_level - 1;
            return self.undo.undo_level;
        }
        undolevel
    }

    pub fn get_undo_pair(
        &mut self,
        undo: bool,
    ) -> (
        &mut Vec<(UndoableThing, IdAction, String)>,
        &mut Vec<(UndoableThing, IdAction, String)>,
    ) {
        match undo {
            true => (&mut self.undo.undo_things, &mut self.undo.redo_things),
            false => (&mut self.undo.redo_things, &mut self.undo.undo_things),
        }
    }

    /// pub fn push_action_on_stack(&mut self, bool undo )
    pub fn action_stack_pump(&mut self, _lib: &FishBlockLibrary, undo: bool) {
        if self.get_undo_pair(undo).0.len() == 0 {
            return;
        }
        let mut level = 0;
        let mut done = false;
        while !done {
            let item = self.get_undo_pair(undo).0.pop().unwrap();
            match item.0 {
                UndoableThing::OpenMarker => {
                    level = level - 1;
                    if level == 0 {
                        done = true;
                    }

                    self.get_undo_pair(undo).1.push((
                        UndoableThing::CloseMarker,
                        IdAction::Nop,
                        String::new(),
                    ));
                }

                UndoableThing::CloseMarker => {
                    level = level + 1;
                    self.get_undo_pair(undo).1.push((
                        UndoableThing::OpenMarker,
                        IdAction::Nop,
                        String::new(),
                    ));
                }

                UndoableThing::Block => match item.1 {
                    IdAction::Create { id } => {
                        // undo of create = delete
                        let b = self.get_block(&id).expect("find block");
                        let b_string = b.serialize_ron();

                        let index = self.blocks.iter().position(|x| x.id == id).unwrap();
                        self.blocks.remove(index);

                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Block,
                            IdAction::Delete { id: id },
                            b_string,
                        ));
                    }
                    IdAction::Modify { id } => {
                        // deserialize old state in to existing block
                        let b = self.get_block(&id).expect("find block");
                        let b_string = b.serialize_ron();
                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Block,
                            IdAction::Modify { id: id },
                            b_string,
                        ));                    
                        let b = self.blocks.find_mut(id).expect("find block");
                        b.reload_from_string(&item.2);
                    
                    }
                    IdAction::Delete { id } => {
                        // undo of delete = create
                        let zombie = FishBlock::deserialize_ron(&item.2).expect("create a block");
                        self.blocks.push(zombie);
                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Block,
                            IdAction::Create { id: id },
                            String::new(),
                        ));
                    }
                    _ => {}
                },

                UndoableThing::Connection => match item.1 {
                    IdAction::Create { id } => {
                        // undo of create = delete
                        let c = self.get_connection(id).expect("find connection");
                        let c_string = c.serialize_ron();

                        let index = self.connections.iter().position(|x| x.id == id).unwrap();
                        self.connections.remove(index);

                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Connection,
                            IdAction::Delete { id: id },
                            c_string,
                        ));
                    }

                    IdAction::Modify { id } => {
                        // deserialize old state in to existing block
                        let c = self.get_connection(id).expect("find connection");
                        let c_string = c.serialize_ron();
                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Connection,
                            IdAction::Modify { id: id },
                            c_string,
                        ));
                        let c = self.connections.find_mut(id).expect("find connection");
                        c.reload_from_string(&item.2);
                    }

                    IdAction::Delete { id } => {
                        // undo of delete = create
                        let zombie =
                            FishConnection::deserialize_ron(&item.2).expect("create a connection");
                        self.connections.push(zombie);
                        self.get_undo_pair(undo).1.push((
                            UndoableThing::Connection,
                            IdAction::Create { id: id },
                            String::new(),
                        ));
                    }
                    _ => {}
                },
            }
        }
    }

    pub fn undo(&mut self, lib: &FishBlockLibrary) {
        self.action_stack_pump(lib, true);
    }

    pub fn redo(&mut self, lib: &FishBlockLibrary) {
        self.action_stack_pump(lib, false);
    }

    pub fn add_block(&mut self, lib: &FishBlockLibrary) {
        self.undo_checkpoint_start();
        self.create_block(lib, String::from("Utility"), 100., 100.);
        self.undo_checkpoint_end();
    }

    pub fn remove_connection(&mut self, id: u64) {
        self.undo_checkpoint_start();

        let c = self.get_connection(id).expect("find block");
        let cstring = c.serialize_ron();

        println!("deleting connection {}", id);

        let index = self.connections.iter().position(|x| x.id == id).unwrap();
        self.connections.remove(index);

        self.undo.undo_things.push((
            UndoableThing::Connection,
            IdAction::Delete { id: id },
            cstring,
        ));

        self.undo_checkpoint_end();
    }

    pub fn remove_block(&mut self, id: u64) {
        self.undo_checkpoint_start();

        // remove connections involving block
        // remove preset data for block
        let mut deleteconns: Vec<u64> = vec![];
        for i in 0..self.connections.len() {
            if self.connections[i].from_block == id || self.connections[i].to_block == id {
                deleteconns.push(self.connections[i].id.clone());
            }
        }
        for i in 0..deleteconns.len() {
            self.remove_connection(deleteconns[i]);
        }
        let b = self.get_block(&id).expect("find block");
        let bstring = b.serialize_ron();

        let index = self.blocks.iter().position(|x| x.id == id).unwrap();
        self.blocks.remove(index);

        self.undo
            .undo_things
            .push((UndoableThing::Block, IdAction::Delete { id: id }, bstring));

        self.undo_checkpoint_end();
    }

    pub fn get_block(&self, id: &u64) -> Option<&FishBlock> {
        self.blocks.iter().find(|&x| x.id == *id)
    }

    pub fn get_connection(&self, id: u64) -> Option<&FishConnection> {
        self.connections.iter().find(|&x| x.id == id)
    }

    pub fn undo_save_block_before_modify(&mut self, id: &u64) {
        let bopt = self.get_block(id);
        if bopt.is_some() == false {
            return;
        };
        let b = bopt.unwrap();
        let bstring = b.serialize_ron();

        self.undo
            .undo_things
            .push((UndoableThing::Block, IdAction::Modify { id: *id }, bstring));
    }

    pub fn move_block(&mut self, id: &u64, x: f64, y: f64) {
        self.undo_checkpoint_start();
        self.undo_save_block_before_modify(id);
        let g = self.blocks.iter_mut().find(|x| x.id == *id);

        if g.is_none() {
            return;
        }

        let b = g.unwrap();
        //let g = self.get_block(id).unwrap();
        let mult = 1.;
        let div = 1. / (mult as f64);
        b.x = floor(x * div ).max(0.) * mult;
        b.y = floor(y * div ).max(0.) * mult;

        self.undo_checkpoint_end();
    }


    pub fn move_selection(&mut self, selection: &FishPatchSelection, dx: f64, dy: f64) {
        self.undo_checkpoint_start();

        for id in selection.blocks.iter(){
            self.undo_save_block_before_modify(id);
            let g = self.blocks.iter_mut().find(|x| x.id == *id);

            if g.is_none() {
                return;
            }

            let b = g.unwrap();
            //let g = self.get_block(id).unwrap();
            let mult = 1.;
            let div = 1. / (mult );
            b.x += dx;//floor(((b.x as f64 + dx) * div) ).max(0.) * mult;
            b.y += dy;//floor(((b.y as f64 + dy) * div) ).max(0.) * mult;

        }

        self.undo_checkpoint_end();
    }

    pub fn create_block(&mut self, lib: &FishBlockLibrary, name: String, x: f64, y: f64) {
        self.undo_checkpoint_start();
        let mut b = lib.create_instance_from_template(&name);
        b.x = x;
        b.y = y;
        b.id = LiveId::unique().0;
        let id = b.id;
        let b_string = b.serialize_ron();

        self.blocks.push(b);

        self.undo
            .undo_things
            .push((UndoableThing::Block, IdAction::Create { id: id }, b_string));

        self.undo_checkpoint_end();
    }

    pub fn create_test_patch(id: u64, lib: &FishBlockLibrary) -> FishPatch {
        let mut patch = FishPatch::default();
        patch.name = String::from(format!("Test Patch {:?}", id));
        patch.id = id;
        patch.undo_checkpoint_start();
        let mut i = 0.;

        patch.create_block(
            lib,
            String::from("Oscillator"),
            i % 3. * 300.,
            i / 3. * 300. + 100.,
        );

        i = i + 1.;
        patch.create_block(lib, String::from("Filter"), i % 3. * 300., i / 3. * 300. + 100.);

        i = i + 1.;
        patch.create_block(lib, String::from("Effect"), i % 3. * 300., i / 3. * 300. + 100.);

        i = i + 1.;
        patch.create_block(lib, String::from("Meta"), i % 3. * 300., i / 3. * 300. + 100.);

        i = i + 1.;
        patch.create_block(
            lib,
            String::from("Envelope"),
            i % 3. * 300.,
            i / 3. * 300. + 100.,
        );

        i = i + 1.;
        patch.create_block(
            lib,
            String::from("Modulator"),
            i % 3. * 300.,
            i / 3. * 300. + 100.,
        );
        i = i + 1.;

        patch.create_block(lib, String::from("Utility"), 0., i / 3. * 300. + 100.); //i=i+1;

        for i in 0..7 {
            patch.presets.push(FishPreset::create_test_preset(i));
        }

        for i in 0..8 {
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
        patch.undo_checkpoint_end();
        patch
    }
}
