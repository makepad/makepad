use crate::{
    makepad_platform::*,
    widget::*,
};

#[derive(Debug)]
pub struct DataBindingStore {
    pub nodes: Vec<LiveNode>,
    mutated_by: Vec<WidgetUid>,
}

impl DataBindingStore {
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        nodes.open_object(LiveId(0));
        nodes.close();
        Self {
            nodes,
            mutated_by: Vec::new(),
        }
    }
    
    pub fn from_nodes(nodes:Vec<LiveNode>)-> Self {
        Self {
            nodes,
            mutated_by: Vec::new(),
        }
    }
    
    pub fn set_mutated_by(&mut self, uid: WidgetUid) {
        if !self.mutated_by.contains(&uid) {
            self.mutated_by.push(uid);
        }
    }
    
    pub fn data_bind<F>(&mut self, cx:&mut Cx,actions:&Actions, ui:&WidgetRef, f:F) where F:Fn(DataBindingMap){
        f(self.widgets_to_data(cx, actions, ui));
        f(self.data_to_widgets(cx, ui))
    }
    
    pub fn contains(&mut self, data_id: &[LiveId])->bool{
        self.nodes.read_field_value(data_id).is_some()
    }
    
}

enum Direction<'a> {
    DataToWidgets(&'a DataBindingStore),
    WidgetsToData(&'a Actions, &'a mut DataBindingStore)
}

pub struct DataBindingMap<'a> {
    pub debug_missing: bool,
    pub cx: &'a mut Cx,
    direction: Direction<'a>,
    pub ui: &'a WidgetRef,
}

impl DataBindingStore {
    pub fn data_to_widgets<'a>(&'a self, cx: &'a mut Cx,  ui: &'a WidgetRef) -> DataBindingMap<'a> {
        DataBindingMap {
            debug_missing: false,
            direction: Direction::DataToWidgets(self),
            cx,
            ui,
        }
    }
    
    pub fn widgets_to_data<'a>(&'a mut self, cx: &'a mut Cx, actions: &'a Actions, ui: &'a WidgetRef) -> DataBindingMap<'a> {
        DataBindingMap {
            debug_missing: false,
            direction: Direction::WidgetsToData(actions, self),
            cx,
            ui,
        }
    }
}

impl<'a> DataBindingMap<'a> {
    pub fn with_debug_missing(mut self) -> Self {
        self.debug_missing = true;
        self
    }
    
    pub fn is_data_to_widgets(&self) -> bool {
        if let Direction::DataToWidgets(_) = self.direction {true}else {false}
    }
    
    pub fn is_widgets_to_data(&self) -> bool {
        if let Direction::WidgetsToData(_,_) = self.direction {true}else {false}
    }
    
    pub fn bind(&mut self, data_id: &[LiveId], widgets: &[&[LiveId]]) {
        // alright so. we have a direction.
        match &mut self.direction{
            Direction::WidgetsToData(actions, store) =>{
                if actions.len() == 0 {
                    return
                }
                for widget in self.ui.widgets(widgets).iter() {
                    if widget.widget_to_data(self.cx, actions, &mut store.nodes, data_id) {
                        store.set_mutated_by(widget.widget_uid());
                    }
                }
            }
            Direction::DataToWidgets(store)=>{
                let mut any_found = false;
                for widget in self.ui.widgets(widgets).iter() {
                    any_found = true;
                    let uid = widget.widget_uid();
                    if !store.mutated_by.contains(&uid) {
                        widget.data_to_widget(self.cx, &store.nodes, data_id);
                    }
                }
                if !any_found && self.debug_missing {
                    log!("No widgets found for databinding {:?}", widgets);
                }
            }
        }
    }
    
    pub fn apply<F>(&mut self, data: &[LiveId], widget_val: &[&[LiveId]; 2], map: F)
    where F: FnOnce(LiveValue) -> LiveValue {
        if let Direction::DataToWidgets(store) = &self.direction{
            if let Some(v) = store.nodes.read_field_value(data) {
                let mut ui_nodes = LiveNodeVec::new();
                ui_nodes.write_field_value(widget_val[1], map(v.clone()));
                let widgets = self.ui.widgets(&[widget_val[0]]);
                for widget in widgets.iter(){
                    widget.apply_over(self.cx, &ui_nodes)
                }
                /*else if self.debug_missing {
                    log!("Databinding apply cannot find widget {:?}", widget_val[0]);
                }*/
            }
        }
    }
}

