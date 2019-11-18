use std::any::TypeId;
use crate::cx::*;



// Colors


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ColorId(pub &'static str, pub u32);

impl ColorId {
    pub fn store(&self) -> ShVarStore {
        ShVarStore::UniformColor(*self)
    }
    pub fn set_base(&self, cx: &mut Cx, value: Color) {
        cx.theme_colors.insert((*self, ClassId::base()), value);
    }
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, value: Color) {
        cx.theme_colors.insert((*self, class), value);
    }
    pub fn base(&self, cx: &Cx) -> Color {
        *cx.theme_colors.get(&(*self, ClassId::base())).expect("Cannot find ColorId")
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> Color {
        if let Some(c) = cx.theme_colors.get(&(*self, class)) {return *c};
        self.base(cx)
    }
}

#[macro_export]
macro_rules!color_id {
    () => {
        ColorId(file!(), line!())
    };
}



// Text



#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct TextStyleId(pub &'static str, pub u32);

impl TextStyleId {
    pub fn set_base(&self, cx: &mut Cx, value: TextStyle) {
        cx.theme_text_styles.insert((*self, ClassId::base()), value);
    }
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, value: TextStyle) {
        cx.theme_text_styles.insert((*self, class), value);
    }
    pub fn base(&self, cx: &Cx) -> TextStyle {
        cx.theme_text_styles.get(&(*self, ClassId::base())).expect("Cannot find TextStyle").clone()
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> TextStyle {
        if let Some(ts) = cx.theme_text_styles.get(&(*self, class)) {return ts.clone()};
        self.base(cx)
    }
}

#[macro_export]
macro_rules!text_style_id {
    ()  => {
        TextStyleId(file!(), line!())
    };
}


// Layout

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct LayoutId(pub &'static str, pub u32);

impl LayoutId {
    pub fn set_base(&self, cx: &mut Cx, value: Layout) {
        cx.theme_layouts.insert((*self, ClassId::base()), value);
    }
    
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, value: Layout) {
        cx.theme_layouts.insert((*self, class), value);
    }
    
    pub fn base(&self, cx: &Cx) -> Layout {
        *cx.theme_layouts.get(&(*self, ClassId::base())).expect("Cannot find LayoutId")
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> Layout {
        if let Some(l) = cx.theme_layouts.get(&(*self, class)) {return *l};
        self.base(cx)
    }
}

#[macro_export]
macro_rules!layout_id {
    () => {
        LayoutId(file!(), line!())
    };
}


// Walks


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct WalkId(pub &'static str, pub u32);

impl WalkId {
    pub fn set_base(&self, cx: &mut Cx, value: Walk) {
        cx.theme_walks.insert((*self, ClassId::base()), value);
    }
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, value: Walk) {
        cx.theme_walks.insert((*self, class), value);
    }
    pub fn base(&self, cx: &Cx) -> Walk {*cx.theme_walks.get(&(*self, ClassId::base())).expect("Cannot find WalkId")}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Walk {
        if let Some(w) = cx.theme_walks.get(&(*self, class)) {return *w};
        self.base(cx)
    }
}


#[macro_export]
macro_rules!walk_id {
    () => {
        WalkId(file!(), line!())
    };
}



// Animations


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct AnimId(pub TypeId);

impl AnimId {
    pub fn base(&self, cx: &Cx) -> Anim {cx.theme_anims.get(&(*self, ClassId::base())).expect("Cannot find WalkId").clone()}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&(*self, class)) {return a.clone()};
        self.base(cx)
    }
}

pub trait ThemeAnim {
    fn id() -> AnimId;
    fn set_base(cx: &mut Cx, value: Anim) {
        cx.theme_anims.insert((Self::id(), ClassId::base()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Anim) {
        cx.theme_anims.insert((Self::id(), class), value);
    }
    fn base(cx: &Cx) -> Anim {
        cx.theme_anims.get(&(Self::id(), ClassId::base())).expect("Cannot find WalkId").clone()
    }
    fn class(cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&(Self::id(), class)) {return a.clone()};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_anim {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeAnim for $ name {
            fn id() -> AnimId {AnimId(std::any::TypeId::of::< $ name>())}
        }
    };
}


// Shaders

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ShaderId(pub TypeId);

impl ShaderId {
    pub fn base(&self, cx: &Cx) -> Shader {cx.theme_shaders.get(&(*self, ClassId::base())).expect("Cannot find WalkId").clone()}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Shader {
        if let Some(a) = cx.theme_shaders.get(&(*self, class)) {return a.clone()};
        self.base(cx)
    }
}

pub trait ThemeShader {
    fn id() -> ShaderId;
    fn name() -> &'static str;
    fn set_base(cx: &mut Cx, sg: ShaderGen) {
        let shader = cx.add_shader(sg, Self::name());
        cx.theme_shaders.insert((Self::id(), ClassId::base()), shader);
    }
    fn set_class(cx: &mut Cx, class: ClassId, sg: ShaderGen) {
        let shader = cx.add_shader(sg, Self::name());
        cx.theme_shaders.insert((Self::id(), class), shader);
    }
    fn base(cx: &Cx) -> Shader {
        cx.theme_shaders.get(&(Self::id(), ClassId::base())).expect("Cannot find WalkId").clone()
    }
    fn class(cx: &Cx, class: ClassId) -> Shader {
        if let Some(a) = cx.theme_shaders.get(&(Self::id(), class)) {return a.clone()};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_shader {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeShader for $ name {
            fn id() -> ShaderId {ShaderId(std::any::TypeId::of::< $ name>())}
            fn name() -> &'static str {stringify!( $ name)}
        }
    };
}


// Classes


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ClassId(pub &'static str, pub u32);

#[macro_export]
macro_rules!class_id {
    ( ) => {
        ClassId(file!(), line!())
    };
}


impl ClassId {
    pub fn base() -> ClassId{class_id!()}
}
