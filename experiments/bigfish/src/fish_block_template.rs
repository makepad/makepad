use crate::fish_block::*;
use crate::fish_param_storage::*;
use crate::fish_ports::*;
use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub enum FishBlockCategory {
    Meta,
    Generator,
    Modulator,
    Effect,
    Filter,
    Envelope,
    #[default]
    Utility,
}
#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishBlockTemplate {
    pub id: u64,
    pub name: String,
    pub displayname: String,
    pub description: String,
    pub creator: String,
    pub path: String,
    pub category: FishBlockCategory,

    pub parameters: Vec<FishParamStorage>,
    pub inputs: Vec<FishInputPort>,
    pub outputs: Vec<FishOutputPort>,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishBlockLibrary {
    pub allblocks: Vec<FishBlockTemplate>,
    pub nulltemplate: FishBlockTemplate,
}

impl FishBlockLibrary {
    pub fn populate_library(&mut self, _basepath: &str) {
        self.nulltemplate = FishBlockTemplate {
            category: FishBlockCategory::Meta,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Unknown"),
            displayname: String::from("Unknown"),
            description: String::from(
                "This is the empty null block. Is something missing in your library?",
            ),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/null"),
        };

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Generator,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Oscillator"),
            displayname: String::from("Oscillator"),
            description: String::from("Generic osc!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/oscillator"),
        });

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Effect,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Effect"),
            displayname: String::from("Effect"),
            description: String::from("Generic effect!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/effect"),
        });

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Filter,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Filter"),
            displayname: String::from("Filter"),
            description: String::from("Generic filter!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/filter"),
        });

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Meta,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Meta"),
            displayname: String::from("Meta"),
            description: String::from("Generic meta!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/meta"),
        });

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Utility,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Utility"),
            displayname: String::from("Utility"),
            description: String::from("Generic utility!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/util"),
        });

        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Envelope,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Envelope"),
            displayname: String::from("Envelope"),
            description: String::from("Generic envelope!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/envelope"),
        });
        self.allblocks.push(FishBlockTemplate {
            category: FishBlockCategory::Modulator,
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],

            id: 0,
            name: String::from("Modulator"),
            displayname: String::from("Modulator"),
            description: String::from("Generic modulator!"),
            creator: String::from("Stijn Haring-Kuipers"),
            path: String::from("/modulator"),
        });

        self.add_dummy_inputs_and_outputs();
    }

    pub fn add_dummy_inputs_and_outputs(&mut self) {
        for i in &mut self.allblocks {
            i.inputs.push(FishInputPort {
                id: 0,
                name: String::from("in 1"),
                datatype: ConnectionType::Audio,
            });
            i.outputs.push(FishOutputPort {
                id: 0,
                name: String::from("out 1"),
                datatype: ConnectionType::Audio,
            });

            i.inputs.push(FishInputPort {
                id: 1,
                name: String::from("in 2"),
                datatype: ConnectionType::Control,
            });
            i.outputs.push(FishOutputPort {
                id: 1,
                name: String::from("out 2"),
                datatype: ConnectionType::Control,
            });

            i.inputs.push(FishInputPort {
                id: 2,
                name: String::from("in 3"),
                datatype: ConnectionType::MIDI,
            });
            i.outputs.push(FishOutputPort {
                id: 2,
                name: String::from("out 3"),
                datatype: ConnectionType::MIDI,
            });
        }
    }

    pub fn find_template(&self, name: &str) -> &FishBlockTemplate {
        if let Some(result) = self.allblocks.iter().find(|v| v.name == name) {
            return result;
        }
        return &self.nulltemplate;
    }

    pub fn create_instance_from_template(&self, name: &str) -> FishBlock {
        let t = self.find_template(name);
        let mut f = FishBlock::default();
        f.category = t.category.clone();
        f.library_id = t.id.clone();
        f.name = t.name.clone();

        for i in &t.inputs {
            f.input_ports.push(FishInputPortInstance {
                id: i.id,
                source_id: i.id,
                name: i.name.clone(),
                display_x: 0,
                display_y: 0,
                datatype: i.datatype,
            })
        }
        for i in &t.outputs {
            f.output_ports.push(FishOutputPortInstance {
                id: i.id,
                source_id: i.id,
                name: i.name.clone(),
                display_x: 0,
                display_y: 0,
                datatype: i.datatype,
            })
        }

        f
    }
}
