use crate::{
    makepad_platform::*,
    frame::*,
    widget::*,
};

pub enum DataBinding{
    ToWidgets(Vec<LiveNode>),
    FromWidgets(Vec<LiveNode>)
}

pub struct BindMapTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId], &'a [LiveId])]);
pub struct BindTabTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId], &'a [LiveId], &'a [LiveId], bool)]);
pub struct BindDataTable<'a>(pub &'a [(&'a [LiveId], &'a [LiveId])]);

impl DataBinding{
    pub fn new()->Self{
        let mut nodes = Vec::new();
        nodes.open();
        nodes.close();
        Self::FromWidgets(nodes)
    }
    
    pub fn to_widgets(&mut self, nodes:Vec<LiveNode>){
        *self = Self::ToWidgets(nodes);
    }
    
    pub fn from_widgets(&self)->Option<&[LiveNode]>{
        match self{
            Self::FromWidgets(v) if v.len() > 2=>Some(v),
            _=>None
        }
    }
    
    pub fn nodes(&self)->&[LiveNode]{
        match self{
            Self::FromWidgets(v)=>v,
            Self::ToWidgets(v)=>v
        }
    }
    
    pub fn process_map_table(&self, cx:&mut Cx, ui:&FrameRef, map_table:BindMapTable){
        for map in map_table.0{
            let data_nodes = self.nodes();
            if let Some(value) = data_nodes.read_by_field_path(map.2){
                let mut ui_nodes = LiveNodeVec::new();
                ui_nodes.write_by_field_path(map.1, &[LiveNode::from_value(value.clone())]);
                ui.get_widget(map.0).apply_over(cx, &ui_nodes)
            }
        }   
     }
     
     pub fn process_tab_table(&self, cx:&mut Cx, ui:&FrameRef, tab_table:BindTabTable ){
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
     
     pub fn process_data_table(&mut self, cx:&mut Cx, ui:&FrameRef, data_table: BindDataTable, act: &WidgetActions){
        for bind in data_table.0{
            ui.get_widget(bind.0).bind_to(cx, self, bind.1, act);
        }
     }
}

