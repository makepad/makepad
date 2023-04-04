use crate::{
    makepad_platform::*,
    widget::*,
};

pub struct DataBindingStore {
    pub nodes: Vec<LiveNode>,
    mutated_by: Vec<WidgetUid>
}

impl DataBindingStore {
    pub fn new() -> Self {
        let mut nodes = Vec::new();
        nodes.open_object(LiveId(0));
        nodes.close();
        Self {
            nodes,
            mutated_by: Vec::new()
        }
    }
    
    pub fn set_mutated_by(&mut self, uid:WidgetUid){
        if !self.mutated_by.contains(&uid) {
            self.mutated_by.push(uid);
        }        
    }
}

enum Direction {
    DataToWidgets,
    WidgetsToData
}

pub struct DataBindingMap<'a> {
    pub store: &'a mut DataBindingStore,
    pub cx: &'a mut Cx,
    direction: Direction,
    pub actions: &'a WidgetActions,
    pub ui: &'a WidgetRef,
}

impl DataBindingStore {
    pub fn data_to_widgets<'a>(&'a mut self, cx: &'a mut Cx, actions: &'a WidgetActions, ui: &'a WidgetRef) -> DataBindingMap {
        DataBindingMap {
            direction: Direction::DataToWidgets,
            store: self,
            cx,
            ui,
            actions
        }
    }
    
    pub fn widgets_to_data<'a>(&'a mut self, cx: &'a mut Cx, actions: &'a WidgetActions, ui: &'a WidgetRef) -> DataBindingMap {
        DataBindingMap {
            direction: Direction::WidgetsToData,
            store: self,
            cx,
            ui,
            actions
        }
    }
}

impl<'a> DataBindingMap<'a> {
    pub fn is_data_to_widgets(&self) -> bool {
        if let Direction::DataToWidgets = self.direction {true}else {false}
    }
    
    pub fn is_widgets_to_data(&self) -> bool {
        if let Direction::WidgetsToData = self.direction {true}else {false}
    }
    
    pub fn bind(&mut self, data_id: &[LiveId], widgets: &[&[LiveId]]) {
        // alright so. we have a direction.
        if self.is_data_to_widgets() {
            let mut any_found = false;
            for widget in self.ui.get_widgets(widgets).iter() {
                any_found = true;
                let uid = widget.widget_uid();
                if !self.store.mutated_by.contains(&uid) {
                    widget.data_to_widget(self.cx, &self.store.nodes, data_id);
                }
            }
            if !any_found{
                log!("No widgets found for databinding {:?}", widgets);
            }
        }
        else {
            for widget in self.ui.get_widgets(widgets).iter() {
                if widget.widget_to_data(self.cx, self.actions, &mut self.store.nodes, data_id) {
                    self.store.set_mutated_by(widget.widget_uid());
                }
            }
        }
    }
    
    pub fn apply<F>(&mut self, data: &[LiveId], widget_val:&[&[LiveId];2], map:F)
        where F: FnOnce(LiveValue) -> LiveValue{
        if let Some(v) = self.store.nodes.read_field_value(data){
            let mut ui_nodes = LiveNodeVec::new();
            ui_nodes.write_field_value(widget_val[1], map(v.clone()));
            self.ui.get_widget(widget_val[0]).apply_over(self.cx, &ui_nodes)
        }
    }
}

