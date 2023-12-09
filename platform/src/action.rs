
use crate::cx::Cx;
use std::any::Any;

pub type Action = Box<dyn Any>;
pub type ActionsBuf = Vec<Action>;
pub type Actions = [Action];

pub trait ActionCast<T> {
    fn cast(&self) -> T;
}

impl<T: Any + 'static + Default + Clone> ActionCast<T> for Box<dyn Any>{
    fn cast(&self) -> T where T: Default + Clone{
        if let Some(item) = self.downcast_ref::<T>() {
            item.clone()
        }
        else {
            T::default()
        }
    }
}

impl Cx{
    pub fn action(&mut self, action: impl Any){
        self.new_actions.push(Box::new(action));
    }
    
    pub fn extend_actions(&mut self, actions: ActionsBuf){
        self.new_actions.extend(actions);
    }
    
    pub fn map_actions<F, G, R>(&mut self, f: F, g:G) -> R where
    F: FnOnce(&mut Cx) -> R,
    G: FnOnce(&mut Cx, ActionsBuf)->ActionsBuf,
    {
        let start = self.new_actions.len();
        let r = f(self);
        let end = self.new_actions.len();
        if start != end{
            let buf = self.new_actions.drain(start..end).collect();
            let buf = g(self, buf);
            self.new_actions.extend(buf);
        }
        r
    }

    pub fn scope_actions<F>(&mut self, f: F) -> ActionsBuf where
    F: FnOnce(&mut Cx),
    {
        let mut actions = Vec::new();
        std::mem::swap(&mut self.new_actions, &mut actions);
        f(self);
        std::mem::swap(&mut self.new_actions, &mut actions);
        actions
    }
}