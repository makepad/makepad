use crate::{
    makepad_platform::*,
    frame::*,
    widget::*,
};

pub enum DataBinding{
    ToWidgets{nodes:Vec<LiveNode>},
    FromWidgets{nodes:Vec<LiveNode>, updated:Vec<WidgetUid>}
}

pub struct MapBindTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId], &'a [LiveId])]);
pub struct BoolBindTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId], &'a [LiveId], &'a [LiveId], bool)]);
pub struct DataBindTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId])]);

impl DataBinding{
    pub fn new()->Self{
        let mut nodes = Vec::new();
        nodes.open();
        nodes.close();
        Self::FromWidgets{nodes, updated:Vec::new()}
    }
    
    pub fn set_updated(&mut self, uid:WidgetUid){
        match self{
            Self::FromWidgets{updated,..}=>
                if !updated.contains(&uid){updated.push(uid)
            }
            _=>()
        }
    }
    
    pub fn to_widgets(&mut self, nodes:Vec<LiveNode>){
        *self = Self::ToWidgets{nodes};
    }
    
    pub fn from_widgets(&self)->Option<&[LiveNode]>{
        match self{
            Self::FromWidgets{nodes,..} if nodes.len() > 2=>Some(nodes),
            _=>None
        }
    }
    
    pub fn nodes(&self)->&[LiveNode]{
        match self{
            Self::FromWidgets{nodes,..}=>nodes,
            Self::ToWidgets{nodes}=>nodes
        }
    }
    
    pub fn process_map_table(&self, cx:&mut Cx, ui:&FrameRef, map_table:MapBindTable){
        for map in map_table.0{
            let data_nodes = self.nodes();
            if let Some(value) = data_nodes.read_by_field_path(map.2){
                let mut ui_nodes = LiveNodeVec::new();
                ui_nodes.write_by_field_path(map.1, &[LiveNode::from_value(value.clone())]);
                ui.get_widget(map.0).apply_over(cx, &ui_nodes)
            }
        }   
     }
     
     pub fn process_bool_table(&self, cx:&mut Cx, ui:&FrameRef, tab_table:BoolBindTable ){
        for tab in tab_table.0{
            let nodes = self.nodes();
            if let Some(LiveValue::BareEnum(id)) = nodes.read_by_field_path(tab.2){
                let value =  *id == tab.3[0];
                let mut nodes = LiveNodeVec::new();
                nodes.write_by_field_path(tab.1, &[LiveNode::from_value(
                    LiveValue::Bool(if tab.4 {value} else {!value})
                )]);
                ui.get_widget(tab.0).apply_over(cx, &nodes)
            }
        }
     }
     
     pub fn process_data_table(&mut self, cx:&mut Cx, ui:&FrameRef, data_table: DataBindTable, act: &WidgetActions){
        for bind in data_table.0{
            ui.get_widget(bind.0).bind_to(cx, self, bind.1, act);
        }
     }
}

