use std::any::TypeId;
use crate::cx::*;



// Colors


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ColorId(pub TypeId);

impl ColorId {
    pub fn base(&self, cx: &Cx) -> Color {
        *cx.theme_colors.get(&(*self, ThemeBase::id())).expect("Cannot find ColorId")
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> Color {
        if let Some(c) = cx.theme_colors.get(&(*self, class)) {return *c};
        self.base(cx)
    }
}

pub trait ThemeColor {
    fn id() -> ColorId;
    fn store() -> ShVarStore {
        ShVarStore::UniformColor(Self::id())
    }
    fn set_base(cx: &mut Cx, value: Color) {
        cx.theme_colors.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Color) {
        cx.theme_colors.insert((Self::id(), class), value);
    }
    fn base(cx: &Cx) -> Color {
        *cx.theme_colors.get(&(Self::id(), ThemeBase::id())).expect("Cannot find ColorId")
    }
    fn class(cx: &Cx, class: ClassId) -> Color {
        if let Some(color) = cx.theme_colors.get(&(Self::id(), class)) {return *color};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_color {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeColor for $ name {
            fn id() -> ColorId {ColorId(std::any::TypeId::of::< $ name>())}
        }
    };
}



// Text



#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct TextStyleId(pub TypeId);

impl TextStyleId {
    pub fn base(&self, cx: &Cx) -> TextStyle {
        cx.theme_text_styles.get(&(*self, ThemeBase::id())).expect("Cannot find TextStyle").clone()
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> TextStyle {
        if let Some(ts) = cx.theme_text_styles.get(&(*self, class)) {return ts.clone()};
        self.base(cx)
    }
}

pub trait ThemeTextStyle {
    fn id() -> TextStyleId;
    fn set_base(cx: &mut Cx, value: TextStyle) {
        cx.theme_text_styles.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: TextStyle) {
        cx.theme_text_styles.insert((Self::id(), class), value);
    }
    fn base(cx: &Cx) -> TextStyle {
        cx.theme_text_styles.get(&(Self::id(), ThemeBase::id())).expect("Cannot find TextStyle").clone()
    }
    fn class(cx: &Cx, class: ClassId) -> TextStyle {
        if let Some(ts) = cx.theme_text_styles.get(&(Self::id(), class)) {return ts.clone()};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_text_style {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeTextStyle for $ name {
            fn id() -> TextStyleId {TextStyleId(std::any::TypeId::of::< $ name>())}
        }
    };
}


// Layout

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct LayoutId(pub TypeId);

impl LayoutId {
    pub fn base(&self, cx: &Cx) -> Layout {
        *cx.theme_layouts.get(&(*self, ThemeBase::id())).expect("Cannot find LayoutId")
    }
    pub fn class(&self, cx: &Cx, class: ClassId) -> Layout {
        if let Some(l) = cx.theme_layouts.get(&(*self, class)) {return *l};
        self.base(cx)
    }
}

pub trait ThemeLayout {
    fn id() -> LayoutId;
    fn set_base(cx: &mut Cx, value: Layout) {
        cx.theme_layouts.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Layout) {
        cx.theme_layouts.insert((Self::id(), class), value);
    }
    fn base(cx: &Cx) -> Layout {
        *cx.theme_layouts.get(&(Self::id(), ThemeBase::id())).expect("Cannot find Layout")
    }
    fn class(cx: &Cx, class: ClassId) -> Layout {
        if let Some(l) = cx.theme_layouts.get(&(Self::id(), class)) {return *l};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_layout {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeLayout for $ name {
            fn id() -> LayoutId {LayoutId(std::any::TypeId::of::< $ name>())}
        }
    };
}


// Walks


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct WalkId(pub TypeId);

impl WalkId {
    pub fn base(&self, cx: &Cx) -> Walk {*cx.theme_walks.get(&(*self, ThemeBase::id())).expect("Cannot find WalkId")}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Walk {
        if let Some(w) = cx.theme_walks.get(&(*self, class)) {return *w};
        self.base(cx)
    }
}

pub trait ThemeWalk {
    fn id() -> WalkId;
    fn set_base(cx: &mut Cx, value: Walk) {
        cx.theme_walks.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Walk) {
        cx.theme_walks.insert((Self::id(), class), value);
    }
    fn base(cx: &Cx) -> Walk {
        *cx.theme_walks.get(&(Self::id(), ThemeBase::id())).expect("Cannot find WalkId")
    }
    fn class(cx: &Cx, class: ClassId) -> Walk {
        if let Some(w) = cx.theme_walks.get(&(Self::id(), class)) {return *w};
        Self::base(cx)
    }
}

#[macro_export]
macro_rules!theme_walk {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeWalk for $ name {
            fn id() -> WalkId {WalkId(std::any::TypeId::of::< $ name>())}
        }
    };
}


// Animations


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct AnimId(pub TypeId);

impl AnimId {
    pub fn base(&self, cx: &Cx) -> Anim {cx.theme_anims.get(&(*self, ThemeBase::id())).expect("Cannot find WalkId").clone()}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&(*self, class)) {return a.clone()};
        self.base(cx)
    }
}

pub trait ThemeAnim {
    fn id() -> AnimId;
    fn set_base(cx: &mut Cx, value: Anim) {
        cx.theme_anims.insert(( Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Anim) {
        cx.theme_anims.insert(( Self::id(), class), value);
    }
    fn base(cx: &Cx) -> Anim {
        cx.theme_anims.get(&( Self::id(), ThemeBase::id())).expect("Cannot find WalkId").clone()
    }
    fn class(cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&( Self::id(), class)) {return a.clone()};
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
    pub fn base(&self, cx: &Cx) -> Shader {cx.theme_shaders.get(&(*self, ThemeBase::id())).expect("Cannot find WalkId").clone()}
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
        cx.theme_shaders.insert(( Self::id(), ThemeBase::id()), shader);
    }
    fn set_class(cx: &mut Cx, class: ClassId, sg: ShaderGen) {
        let shader = cx.add_shader(sg, Self::name());
        cx.theme_shaders.insert(( Self::id(), class), shader);
    }
    fn base(cx: &Cx) -> Shader {
        cx.theme_shaders.get(&( Self::id(), ThemeBase::id())).expect("Cannot find WalkId").clone()
    }
    fn class(cx: &Cx, class: ClassId) -> Shader {
        if let Some(a) = cx.theme_shaders.get(&( Self::id(), class)) {return a.clone()};
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
            fn name() -> &'static str {stringify!($ name)}
        }
    };
}


// Classes


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ClassId(pub TypeId);

#[macro_export]
macro_rules!theme_class {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl $ name {
            pub fn id() -> ClassId {ClassId(std::any::TypeId::of::< $ name>())}
        }
    };
}

theme_class!(ThemeBase);
