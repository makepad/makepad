use crate::generate_any_trait_api;
use crate::cx::Cx;
use std::any::{TypeId};
use std::fmt::Debug;
use std::fmt;

use std::sync::{
    Mutex,
    mpsc::{Sender}
};


pub (crate) static ACTION_SENDER_GLOBAL: Mutex<Option<Sender<ActionSendSync>>> = Mutex::new(None);

pub trait ActionTrait: 'static {
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    fn ref_cast_type_id(&self) -> TypeId where Self: 'static {TypeId::of::<Self>()}
}

impl<T: 'static + Debug + ?Sized > ActionTrait for T {
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.fmt(f)
    }
}

generate_any_trait_api!(ActionTrait);

impl Debug for dyn ActionTrait {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result{
        self.debug_fmt(f)
    }
}
pub type ActionSendSync = Box<dyn ActionTrait + Send + Sync>;
pub type Action = Box<dyn ActionTrait>;
pub type ActionsBuf = Vec<Action>;
pub type Actions = [Action];

pub trait ActionDefaultRef{
    fn default_ref()->&'static Self;
}

pub trait ActionCast<T> {
    fn cast(&self) -> T;
}

pub trait ActionCastRef<T> {
    fn cast_ref(&self) -> &T;
}

impl<T: ActionTrait + Default + Clone> ActionCast<T> for Box<dyn ActionTrait>{
    fn cast(&self) -> T{
        if let Some(item) = (*self).downcast_ref::<T>() {
            item.clone()
        }
        else {
            T::default()
        }
    }
}


impl<T: ActionTrait + ActionDefaultRef> ActionCastRef<T> for Box<dyn ActionTrait>{
    fn cast_ref(&self) -> &T{
        if let Some(item) = (*self).downcast_ref::<T>() {
            item
        }
        else {
            T::default_ref()
        }
    }
}


impl Cx{
    pub fn handle_action_receiver(&mut self){
        while let Ok(action) = self.action_receiver.try_recv(){
            self.new_actions.push(action);
        }
        self.handle_actions();
    }
    
    pub fn post_action(action:impl ActionTrait + Send + Sync){
        ACTION_SENDER_GLOBAL.lock().unwrap().as_mut().unwrap().send(Box::new(action)).unwrap();
    }
    
    pub fn action(&mut self, action: impl ActionTrait){
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
    
    pub fn mutate_actions<F, G, R>(&mut self, f: F, g:G) -> R where
    F: FnOnce(&mut Cx) -> R,
    G: FnOnce(&mut [Action]),
    {
        let start = self.new_actions.len();
        let r = f(self);
        let end = self.new_actions.len();
        if start != end{
            g(&mut self.new_actions[start..end]);
        }
        r
    }

    pub fn capture_actions<F>(&mut self, f: F) -> ActionsBuf where
    F: FnOnce(&mut Cx),
    {
        let mut actions = Vec::new();
        std::mem::swap(&mut self.new_actions, &mut actions);
        f(self);
        std::mem::swap(&mut self.new_actions, &mut actions);
        actions
    }
}