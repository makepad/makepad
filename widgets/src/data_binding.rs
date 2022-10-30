use crate::{
    makepad_platform::*,
    frame::*,
    widget::*,
};

pub enum DataBinding {
    ToWidgets {nodes: Vec<LiveNode>},
    FromWidgets {nodes: Vec<LiveNode>, updated: Vec<WidgetUid>}
}

pub struct DataBindingCxBorrow<'a> {
    db: &'a mut DataBinding,
    cx: &'a mut Cx,
    act: &'a WidgetActions,
    ui: &'a WidgetRef,
}

#[macro_export]
macro_rules!data_to_apply {
    ( $ db: expr, $($data:ident).+ => $($widget:ident).+, $($value:ident).+ => $ map: expr) => {
        $ db.data_to_apply(id!($($data).+), id!($($widget).+), id!($($value).+), | v | { $ map(v).to_live_value()})
    };
}

#[macro_export]
macro_rules!data_to_widget {
    ( $ db: expr, $($data:ident).+ => $($widget:ident).+) => {
        $ db.data_to_widget(id!($($data).+), id!($($widget).+))
    }
}

impl DataBinding {
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        nodes.open();
        nodes.close();
        Self::FromWidgets {nodes, updated: Vec::new()}
    }
    
    pub fn set_updated(&mut self, uid: WidgetUid) {
        match self {
            Self::FromWidgets {updated, ..} =>
            if !updated.contains(&uid) {updated.push(uid)}
            _ => ()
        }
    }
    
    pub fn to_widgets(&mut self, nodes: Vec<LiveNode>) {
        *self = Self::ToWidgets {nodes};
    }
    
    pub fn from_widgets(&self) -> Option<&[LiveNode]> {
        match self {
            Self::FromWidgets {nodes, ..} if nodes.len() > 2 => Some(nodes),
            _ => None
        }
    }
    
    pub fn nodes(&self) -> &[LiveNode] {
        match self {
            Self::FromWidgets {nodes, ..} => nodes,
            Self::ToWidgets {nodes} => nodes
        }
    }
    
    pub fn borrow_cx<'a>(&'a mut self, cx: &'a mut Cx, ui: &'a FrameRef, act: &'a WidgetActions) -> DataBindingCxBorrow<'a> {
        DataBindingCxBorrow {
            db: self,
            cx,
            ui,
            act
        }
    }
}

impl<'a> DataBindingCxBorrow<'a> {
    pub fn data_to_widget(&mut self, data: &[LiveId], widget: &[LiveId]) {
        self.ui.get_widget(widget).bind_to(self.cx, self.db, self.act, data);
    }
    
    pub fn data_to_apply<F>(&mut self, data: &[LiveId], widget: &[LiveId], value: &[LiveId], map: F)
    where F: FnOnce(LiveValue) -> LiveValue{
        let data_nodes = self.db.nodes();
        if let Some(v) = data_nodes.read_by_field_path(data){
            let mut ui_nodes = LiveNodeVec::new();
            ui_nodes.write_by_field_path(value, &[LiveNode::from_value(map(v.clone()))]);
            self.ui.get_widget(widget).apply_over(self.cx, &ui_nodes)
        }
    }
}
