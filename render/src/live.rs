use crate::cx::*;
use makepad_live_parser::LiveValue;

#[derive(Clone, Debug)]
pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub live_types: Vec<LiveType>
}

pub trait LiveFactory {
    fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveComponent>;
    fn live_fields(&self, fields: &mut Vec<LiveField>);
    //    fn live_type(&self) -> LiveType;
}

pub trait LiveNew {
    fn live_new(cx: &mut Cx) -> Self;
    fn live_type() -> LiveType;
    fn live_register(cx: &mut Cx);
}

pub trait ToGenValue {
    fn to_gen_value(&self)->GenValue;
}

#[derive(Debug)]
pub struct GenNode {
    pub id: Id,
    pub value: GenValue
}

#[derive(Debug)]
pub enum GenValue {
    Str(&'static str),
    String(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Id(Id),
    EnumBare{base:Id, variant:Id},
    // stack items
    EnumTuple{base:Id, variant:Id},
    EnumNamed{base:Id, variant:Id},
    ClassBare, // subnodes including this one
    ClassNamed {class: Id}, // subnodes including this one
    
    Close // closes call/class
}

impl GenValue{
    pub fn is_close(&self)->bool{
        if let Self::Close = self{
            true
        }
        else{
            false
        }
    }

    pub fn skip_value(index:&mut usize, nodes: &[GenNode]) {
        let mut stack_depth = 0;
        loop{
            match &nodes[*index].value{
                GenValue::EnumTuple{..} | 
                GenValue::EnumNamed{..}| 
                GenValue::ClassNamed{..} |
                GenValue::ClassBare=>{
                    stack_depth += 1;
                }
                GenValue::Close=>{
                    stack_depth -= 1;
                    if stack_depth == 0{
                        *index += 1;
                        return
                    }
                }
                _=>{
                    if stack_depth == 0{
                        *index += 1;
                        return
                    }
                }
            }
            *index += 1;
        }
    }
}// ok so if every sub has a true 'skip' the outer loop can do it

impl GenNode{
    pub fn new_from_live_node(cx:&mut Cx, live_ptr:LivePtr)->Vec<GenNode>{
        // OK! awesome.
        Vec::new()
    }
}


pub trait LiveComponentValue {
    fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr);
    fn apply_value(&mut self, cx: &mut Cx,  ndex:&mut usize, nodes: &[GenNode]);
}

pub trait LiveComponent {
    fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr);
    fn apply_index(&mut self, cx: &mut Cx, index:&mut usize, nodes: &[GenNode]);
    fn apply(&mut self, cx: &mut Cx, nodes: &[GenNode]){
        if nodes.len()>2{
            self.apply_index(cx, &mut 0, nodes);
        }
    }
}

pub trait CanvasComponent : LiveComponent {
    fn handle(&mut self, cx: &mut Cx, event:&mut Event);
    fn draw(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes:&[GenNode]){
        self.apply(cx, nodes);
        self.draw(cx);
    }
}

pub trait LiveComponentHooks {
    fn live_update_value_unknown(&mut self, _cx: &mut Cx, _id: Id, _ptr: LivePtr) {}
    fn apply_value_unknown(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
        GenValue::skip_value(index, nodes);
    }
    fn before_live_update(&mut self, _cx: &mut Cx, _live_ptr: LivePtr) {}
    fn after_live_update(&mut self, _cx: &mut Cx, _live_ptr: LivePtr) {}
    fn before_apply_index(&mut self, _cx: &mut Cx, _index:usize, _nodes: &[GenNode]) {}
    fn after_apply_index(&mut self, _cx: &mut Cx, _index:usize, _nodes: &[GenNode]) {}
}

pub enum LiveFieldKind {
    Local,
    Live,
}

pub struct LiveField {
    pub id: Id,
    pub live_type: Option<LiveType>,
    pub kind: LiveFieldKind
}

#[derive(Default)]
pub struct LiveBinding {
    pub live_ptr: Option<LivePtr>
}


impl Cx {
    pub fn live_register(&mut self) {
        crate::drawquad::live_register(self);
        crate::drawcolor::live_register(self);
        crate::drawtext::live_register(self);
        crate::geometrygen::live_register(self);
        crate::shader_std::live_register(self);
        crate::font::live_register(self);
        crate::turtle::live_register(self);
    }
    
    pub fn live_ptr_from_id(&self, path: &str, id: Id) -> LivePtr {
        self.shader_registry.live_registry.live_ptr_from_path(
            ModulePath::from_str(path).unwrap(),
            &[id]
        ).unwrap()
    }
    
    pub fn resolve_live_ptr(&self, live_ptr: LivePtr) -> &LiveNode {
        self.shader_registry.live_registry.resolve_ptr(live_ptr)
    }
    
    pub fn scan_live_ptr(&self, class_ptr: LivePtr, seek_id: Id) -> Option<LivePtr> {
        if let Some(mut iter) = self.shader_registry.live_registry.live_class_iterator(class_ptr) {
            while let Some((id, live_ptr)) = iter.next_id(&self.shader_registry.live_registry) {
                if id == seek_id {
                    return Some(live_ptr)
                }
            }
        }
        None
    }
    
    // ok so now what. now we should run the expansion
    pub fn live_expand(&mut self) {
        // lets expand the f'er
        let mut errs = Vec::new();
        self.shader_registry.live_registry.expand_all_documents(&mut errs);
        for err in errs {
            println!("Error expanding live file {}", err);
        }
    }
    
    pub fn verify_type_signature(&self, live_ptr: LivePtr, live_type: LiveType) -> bool {
        let node = self.shader_registry.live_registry.resolve_ptr(live_ptr);
        if let LiveValue::LiveType(ty) = node.value {
            if ty == live_type {
                return true
            }
        }
        println!("TYPE SIGNATURE VERIFY FAILED");
        false
    }
    
    pub fn register_live_body(&mut self, live_body: LiveBody) {
        // ok so now what.
        //println!("{}", live_body.code);
        //let cm = CrateModule::from_module_path_check(&live_body.module_path).unwrap();
        //println!("register_live_body: {}", ModulePath::from_str(&live_body.module_path).unwrap());
        // ok so here we parse the live file
        
        let result = self.shader_registry.live_registry.parse_live_file(
            &live_body.file,
            ModulePath::from_str(&live_body.module_path).unwrap(),
            live_body.code,
            live_body.live_types,
            &self.live_enums,
            live_body.line
        );
        if let Err(msg) = result {
            println!("Error parsing live file {}", msg);
        }
    }
    
    pub fn register_factory(&mut self, live_type: LiveType, factory: Box<dyn LiveFactory>) {
        self.live_factories.insert(live_type, factory);
    }
    
    pub fn register_enum(&mut self, live_type: LiveType, info: LiveEnumInfo) {
        self.live_enums.insert(live_type, info);
    }
    
    pub fn get_factory(&mut self, live_type: LiveType) -> &Box<dyn LiveFactory> {
        self.live_factories.get(&live_type).unwrap()
    }
}


#[macro_export]
macro_rules!live_primitive {
    ( $ ty: ident, $ default: expr, $ update: item, $ apply: item, $to_gen_value: item) => {
        impl ToGenValue for $ty {
            $to_gen_value           
        }
        impl LiveComponent for $ ty {
            $ update
                $ apply
        }
        impl LiveNew for $ ty {
            fn live_new(_cx: &mut Cx) -> Self {
                $ default
            }
            fn live_type() -> LiveType {
                LiveType(std::any::TypeId::of::< $ ty>())
            }
            fn live_register(cx: &mut Cx) {
                struct Factory();
                impl LiveFactory for Factory {
                    fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveComponent> where Self: Sized {
                        Box::new( $ ty ::live_new(cx))
                    }
                    
                    fn live_fields(&self, _fields: &mut Vec<LiveField>) where Self: Sized {
                    }
                }
                cx.live_factories.insert( $ ty::live_type(), Box::new(Factory()));
            }
        }
    }
}

live_primitive!(Id, Id::empty(), fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::MultiPack(id) => {
            match id.unpack() {
                MultiUnpack::SingleId(id) => {
                    *self = id
                },
                MultiUnpack::LivePtr(ptr) => {
                    let other_node = cx.shader_registry.live_registry.resolve_ptr(ptr);
                    *self = other_node.id;
                }
                _ => ()
            }
        }
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match nodes[0].value{
        GenValue::Id(id)=>{
            *self = id;
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }
}, fn to_gen_value(&self)->GenValue{
    GenValue::Id(*self)
});

live_primitive!(LivePtr, LivePtr {file_id: FileId(0), local_ptr: LocalPtr {level: 0, index: 0}}, fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::MultiPack(id) => {
            match id.unpack() {
                MultiUnpack::LivePtr(ptr) => {
                    *self = ptr;
                }
                _ => ()
            }
        }
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    GenValue::skip_value(index, nodes)
}, fn to_gen_value(&self)->GenValue{
    panic!()
});

live_primitive!(f32, 0.0f32, fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::Int(val) => *self = val as f32,
        LiveValue::Float(val) => *self = val as f32,
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match nodes[*index].value{
        GenValue::Float(val)=>{
            *self = val as f32;
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }    
},fn to_gen_value(&self)->GenValue{
    GenValue::Float(*self as f64)
});

live_primitive!(Vec2, Vec2::default(), fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::Vec2(v) => *self = v,
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match nodes[*index].value{
        GenValue::Vec2(val)=>{
            *self = val;
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }        
},fn to_gen_value(&self)->GenValue{
    GenValue::Vec2(*self)
});

live_primitive!(Vec3, Vec3::default(), fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::Vec3(v) => *self = v,
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match nodes[*index].value{
        GenValue::Vec3(val)=>{
            *self = val;
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }       
},fn to_gen_value(&self)->GenValue{
    GenValue::Vec3(*self)
});


live_primitive!(Vec4, Vec4::default(), fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::Color(v) => *self = Vec4::from_u32(v),
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match nodes[*index].value{
        GenValue::Color(v)=>{
            *self = Vec4::from_u32(v);
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }       
},fn to_gen_value(&self)->GenValue{
    GenValue::Color(self.to_u32())
});


live_primitive!(String, String::default(), fn live_update(&mut self, cx: &mut Cx, ptr: LivePtr) {
    let node = cx.shader_registry.live_registry.resolve_ptr(ptr);
    match node.value {
        LiveValue::String {string_start, string_count} => {
            let origin_doc = cx.shader_registry.live_registry.get_origin_doc_from_token_id(node.token_id);
            origin_doc.get_string(string_start, string_count, self);
        }
        _ => ()
    }
}, fn apply_index(&mut self, _cx: &mut Cx, index:&mut usize, nodes: &[GenNode]) {
    match &nodes[*index].value{
        GenValue::Str(v)=>{
            *self = v.to_string();
            *index += 1;
        }
        GenValue::String(v)=>{
            *self = v.clone();
            *index += 1;
        }
        _=>GenValue::skip_value(index, nodes)
    }       
},fn to_gen_value(&self)->GenValue{
    GenValue::String(self.clone())
});

/*
pub trait DrawInputType {
    fn slots() -> usize;
    fn to_ty() -> Ty;
    // this writes a value to the area (wether texture, uniform or instance)
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str);
    /*
    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized;
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized;
    */
}

impl DrawInputType for f32 {
    fn slots() -> usize {1}
    
    fn to_ty() -> Ty {
        Ty::Float
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, id, Ty::Float, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride] = self;
            }
        }
    }
/*
    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_float(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_float(cx, live_item_id, time)
    }*/
}

impl DrawInputType for Vec2 {
    fn slots() -> usize {2}
    
    fn to_ty() -> Ty {
        Ty::Vec2
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, id, Ty::Vec2, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
            }
        }
    }
/*
    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec2(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec2(cx, live_item_id, time)
    }*/
}

impl DrawInputType for Vec3 {
    fn slots() -> usize {3}
    
    fn to_ty() -> Ty {
        Ty::Vec3
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, id, Ty::Vec3, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
                wr.buffer[i * wr.stride + 2] = self.z;
            }
        }
    }

/*
    fn last_animate( animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec3(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec3(cx, live_item_id, time)
    }*/

}

impl DrawInputType for Vec4 {
    fn slots() -> usize {4}
    
    fn to_ty() -> Ty {
        Ty::Vec4
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, id, Ty::Vec4, name) {
            for i in 0..wr.repeat {
                wr.buffer[i * wr.stride + 0] = self.x;
                wr.buffer[i * wr.stride + 1] = self.y;
                wr.buffer[i * wr.stride + 2] = self.z;
                wr.buffer[i * wr.stride + 3] = self.w;
            }
        }
    }

/*
    fn last_animate(animator:&Animator, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.last_vec4(live_item_id)
    }
    
    fn animate(cx: &mut Cx, animator:&mut Animator, time:f64, live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        animator.calc_vec4(cx, live_item_id, time)
    }*/

}

impl DrawInputType for Mat4 {
    fn slots() -> usize {16}
    
    fn to_ty() -> Ty {
        Ty::Mat4
    }
    
    // find uniform, then find instance prop
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(wr) = area.get_write_ref(cx, id, Ty::Mat4, name) {
            for i in 0..wr.repeat {
                for j in 0..16 {
                    wr.buffer[i * wr.stride + j] = self.v[j];
                }
            }
        }
    }
/*
    fn last_animate(_animator:&Animator, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }
    
    fn animate(_cx: &mut Cx, _animator:&mut Animator, _time:f64, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }*/

}

impl DrawInputType for Texture2D {
    fn slots() -> usize {0}
    
    fn to_ty() -> Ty {
        Ty::Texture2D
    }
    
    fn write_draw_input(self, cx: &mut Cx, area: Area, id: Id, name: &str) {
        if let Some(u) = self.0 {
            area.write_texture_2d_id(cx, id, name, u as usize)
        }
    }
/*
    fn last_animate(_animator:&Animator, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }
    
    fn animate(_cx: &mut Cx, _animator:&mut Animator, _time:f64, _live_item_id: LiveItemId)->Option<Self> where Self: Sized{
        None
    }*/
}

#[macro_export]
macro_rules!write_draw_input {
    ( $ cx: ident, $ area: expr, $ path: path, $ value: expr) => {
        ( $ value).write_draw_input( $ cx, $ area, live_str_to_id(module_path!(), stringify!( $ path)), stringify!( $ path))
    }
}



#[macro_export]
macro_rules!default_shader {
    () => {
        Shader {shader_id: 0, location_hash: live_location_hash(file!(), line!() as u64, column!() as u64)}
    }
}

/*
#[macro_export]
macro_rules!default_shader_overload {
    ( $ cx: ident, $ base: ident, $ path: path) => {
        $ cx.live_styles.get_default_shader_overload(
            $ base,
            live_str_to_id(module_path!(), stringify!( $ path)),
            module_path!(),
            stringify!( $ path)
        )
    }
}

#[macro_export]
macro_rules!default_shader {
    () => {
        Shader {shader_id: 0, location_hash: live_location_hash(file!(), line!() as u64, column!() as u64)}
    }
}*/*/

#[macro_export]
macro_rules!uid {
    () => {{
        struct Unique {}
        std::any::TypeId::of::<Unique>().into()
    }};
}
