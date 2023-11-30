use crate::fish_ports::*;
use crate::makepad_micro_serde::*;
use crate::fish_param_storage::*;
use crate::fish_block::*;

#[derive(Clone,Debug, SerRon, DeRon, Default)]

pub struct FishBlockTemplate{
    
    pub id: i32,
    pub name: String,
    pub displayname: String,
    pub description: String,
    pub creator: String,
    pub path: String,
   
   pub parameters: Vec<FishParamStorage>,
   pub inputs: Vec<FishInputPort>,
   pub outputs: Vec<FishOutputPort>,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishBlockLibrary{
    pub allblocks: Vec<FishBlockTemplate>,
    pub nulltemplate: FishBlockTemplate
}

impl FishBlockLibrary
{

    pub fn populate_library(&mut self, basepath: &str)
    {
        self.nulltemplate =  FishBlockTemplate{
          
            outputs: vec![],
            inputs: vec![],
            parameters: vec![],
            
            id: -1, name: String::from("Unknown"), displayname: String::from("Unknown"), description:String::from("This is the empty null block. Is something missing in your library?"), creator: String::from("Stijn Haring-Kuipers"),  path:String::from("/null") };
    }
    pub fn find_template(&self, name: &str) -> &FishBlockTemplate
    {
        if let Some(result) = self.allblocks.iter().find(|v| v.name == name){
            return result;
        }
        return &self.nulltemplate;
    }
    pub fn create_instance_from_template(&self, name: String) -> FishBlock
    {

        let mut f = FishBlock::default();

        f
    }
}