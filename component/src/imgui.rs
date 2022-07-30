use {
    std::rc::Rc,
    std::cell::{RefMut, RefCell},
    crate::{
        makepad_platform::*,
        frame::*,
        frame_traits::*
    },
};



// lets implement LiveApply for FrameUI to forward to the inner frame

#[derive(Clone)]
pub struct ImGUIActions(pub Rc<Vec<FrameActionItem >>);

pub struct ImGUIRun<'a> {
    pub cx: &'a mut Cx,
    pub event: &'a Event,
    pub actions: ImGUIActions,
    pub auto_id: u64,
    pub new_items: Vec<LiveId>,
    pub imgui: ImGUI
}

impl<'a> ImGUIRun<'a> {
    pub fn checked_item<T: 'static + FrameComponent>(&self, what: Option<&mut Box<dyn FrameComponent >>) -> ImGUIItem {
        let uid = if let Some(what) = what {
            if what.cast::<T>().is_none() {
                FrameUid::empty()
            }
            else {
                FrameUid::from_frame_component(what)
            }
        }
        else {
            FrameUid::empty()
        };
        ImGUIItem {
            actions: self.actions.clone(),
            imgui: self.imgui.clone(),
            uid
        }
    }
    
    pub fn frame(&self) -> RefMut<'_, Frame> {
        self.imgui.frame()
    }
    
    pub fn alloc_auto_id(&mut self) -> u64 {
        self.auto_id += 1;
        self.auto_id
    }
    
    pub fn stop(self) {}
    
    // fetch all the children on this frame and call data_bind_read
    pub fn bind_read(&mut self, nodes: &[LiveNode]) {
        self.imgui.frame().bind_read(self.cx, nodes);
    }
    
    
    
    // ImGUI event forwards
    
    
    pub fn on_bind_deltas(&self) -> Vec<Vec<LiveNode >> {
        let mut ret = Vec::new();
        for item in self.actions.0.iter() {
            if let Some(delta) = &item.bind_delta {
                ret.push(delta.clone())
            }
        }
        ret
    }
    

    pub fn on_midi_1_notes(&self) -> Vec<Midi1Note> {
        let mut ret = Vec::new();
        if let Event::Midi1InputData(inputs) = self.event {
            for input in inputs{
                if let Midi1Event::Note(note) = input.data.decode() {
                    ret.push(note);
                }
            }
        }
        ret
    }
    
    pub fn on_construct(&self) -> bool {
        if let Event::Construct = self.event {true}else {false}
    }
    
}

pub struct ImGUIItem {
    pub imgui: ImGUI,
    pub actions: ImGUIActions,
    pub uid: FrameUid,
}

impl ImGUIItem {
    pub fn find_single_action(&self) -> Option<&FrameActionItem> {
        self.actions.0.iter().find( | v | v.uid() == self.uid)
    }
    
    pub fn get<T: 'static + FrameComponent>(&mut self) -> Option<std::cell::RefMut<'_, T >> {
        if self.uid.is_empty() {
            None
        }
        else {
            Some(std::cell::RefMut::map(self.imgui.frame(), | frame | {
                frame.component_by_uid(self.uid).unwrap().cast_mut::<T>().unwrap()
            }))
        }
    }
}

pub struct ImGUIInner {
    frame: Frame,
    _old_items: Option<Vec<LiveId >>,
}

#[derive(Clone)]
pub struct ImGUI {
    inner: Rc<RefCell<ImGUIInner >>,
}

impl ImGUI {
    pub fn frame(&self) -> RefMut<'_, Frame> {
        RefMut::map(self.inner.borrow_mut(), | v | &mut v.frame)
    }
    
    pub fn run<'a>(&self, cx: &'a mut Cx, event: &'a Event) -> ImGUIRun<'a> {
        // fetch actions and wrap
        let actions = ImGUIActions(Rc::new(self.frame().handle_event_iter(cx, event)));
        ImGUIRun {
            event,
            cx,
            actions,
            auto_id: 0,
            new_items: Vec::new(),
            imgui: self.clone()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) -> FrameDraw {
        self.inner.borrow_mut().frame.draw(cx)
    }
    
    pub fn by_type<T: 'static + FrameComponent>(&mut self) -> Option<std::cell::RefMut<'_, T >> {
        if self.frame().by_type::<T>().is_some() {
            Some(std::cell::RefMut::map(self.frame(), | frame | {
                frame.by_type::<T>().unwrap()
            }))
        }
        else {
            None
        }
    }
}

impl LiveNew for ImGUI {
    fn new(cx: &mut Cx) -> Self {
        Self {
            inner: Rc::new(RefCell::new(ImGUIInner {
                frame: Frame::new(cx),
                _old_items: None
            }))
        }
    }
    
    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo {
        Frame::live_type_info(cx)
    }
}

impl LiveApply for ImGUI {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.inner.borrow_mut().frame.apply(cx, from, index, nodes)
    }
}

impl LiveHook for ImGUI {}
