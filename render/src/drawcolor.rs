use crate::cx::*;
use crate::drawquad::DrawQuad;

live_body!{
    
    use crate::shader_std::*;
    use crate::drawquad::DrawQuad;
    
    DrawColor: DrawQuad {
        rust_type: {{DrawColor}};
        fn pixel(self) -> vec4 {
            return self.color;
        }
    }
}


//#[derive(Debug)]
#[repr(C)]
pub struct DrawColor {
    deref_target: DrawQuad,
    color: Vec4
}

impl std::ops::Deref for DrawColor {
    type Target = DrawQuad;
    fn deref(&self) -> &Self::Target {&self.deref_target}
}

impl std::ops::DerefMut for DrawColor {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.deref_target}
}

impl DrawColor {
    fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        match id {
            id!(color) => self.color.live_update(cx, ptr),
            _ => self.deref_target.live_update_value(cx, id, ptr)
        }
    }
}

impl LiveUpdateHooks for DrawColor {
    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        self.deref_target.live_update_value_unknown(cx, id, ptr);
    }
    
    fn before_live_update(&mut self, cx:&mut Cx, live_ptr: LivePtr){
        self.deref_target.before_live_update(cx, live_ptr);
    }
    
    fn after_live_update(&mut self, cx: &mut Cx, live_ptr:LivePtr) {
        self.deref_target.after_live_update(cx, live_ptr);
    }
}

// how could we compile this away
impl LiveNew for DrawColor {
    fn live_new(cx: &mut Cx) -> Self {
        Self {
            deref_target: LiveNew::live_new(cx),
            color: Vec4::all(0.0)
        }
    }
    
    fn live_type() -> LiveType {
        LiveType(std::any::TypeId::of::<DrawColor>())
    }
    
    fn live_register(cx: &mut Cx) {
        cx.register_live_body(live_body());
        struct Factory();
        impl LiveFactory for Factory {
            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveUpdate> {
                Box::new(DrawColor ::live_new(cx))
            }
            
            fn live_fields(&self, fields: &mut Vec<LiveField>) {
                fields.push(LiveField {id: id!(deref_target), live_type: DrawQuad::live_type()});
                fields.push(LiveField {id: id!(color), live_type: Vec4::live_type()});
            }
            
            fn live_type(&self) -> LiveType {
                DrawColor::live_type()
            }
        }
        cx.register_factory(DrawColor::live_type(), Box::new(Factory()));
    }
}

impl LiveUpdate for DrawColor {
    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {
        self.before_live_update(cx, live_ptr);
        // how do we verify this?
        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {
            while let Some((id, live_ptr)) = iter.next(&cx.shader_registry.live_registry) {
                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {
                    // give off an error/warning somehow!
                    return;
                }
                self.live_update_value(cx, id, live_ptr)
            }
        }
        self.after_live_update(cx, live_ptr);
    }
    
    fn _live_type(&self) -> LiveType {
        Self::live_type()
    }
}

