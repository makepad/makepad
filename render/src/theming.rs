use std::any::TypeId;
use crate::cx::*;



// Colors


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ColorId(pub TypeId);

impl ColorId {
    pub fn get(&self, cx: &Cx) -> Color {
        *cx.theme_colors.get(&(*self, ThemeBase::id())).expect("Cannot find ColorId")
    }
    pub fn get_class(&self, cx: &Cx, class: ClassId) -> Color {
        if let Some(c) = cx.theme_colors.get(&(*self, class)) {return *c};
        self.get(cx)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ColorPart {
    Color(Color),
    Id(ColorId)
}

impl ColorPart {
    fn color(&self, cx: &Cx, class: ClassId) -> Color {
        return match self {
            ColorPart::Color(color) => *color,
            ColorPart::Id(id) => *cx.theme_colors.get(&(*id, class)).expect("Cannot find ColorId")
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ColorBlend {
    pub a: ColorPart,
    pub b: Option<ColorId>,
    pub f: f32
}

impl ColorBlend {
    
    pub fn blend(&self, cx: &Cx, class: ClassId) -> Color {
        if self.f<0.00001 {
            return self.a.color(cx, class)
        }
        if let Some(b) = self.b {
            if self.f>0.99999 {
                return *cx.theme_colors.get(&(b, class)).expect("Cannot find ColorId");
            }
            let a = self.a.color(cx, class);
            let b = *cx.theme_colors.get(&(b, class)).expect("Cannot find ColorId");
            let of = 1.0 - self.f;
            return Color {r: a.r * of + b.r * self.f, g: a.g * of + b.g * self.f, b: a.b * of + b.b * self.f, a: a.a * of + b.a * self.f}
        }
        Color::zero()
    }
    
    pub fn blend_to_part(&self, cx: &Cx, class: ClassId) -> ColorPart {
        if self.f<0.00001 {
            return self.a
        }
        if let Some(b) = self.b {
            if self.f>0.99999 {
                return ColorPart::Id(b)
            }
            let a = self.a.color(cx, class);
            let b = *cx.theme_colors.get(&(b, class)).expect("Cannot find ColorId");
            let of = 1.0 - self.f;
            return ColorPart::Color(Color {r: a.r * of + b.r * self.f, g: a.g * of + b.g * self.f, b: a.b * of + b.b * self.f, a: a.a * of + b.a * self.f})
        }
        ColorPart::Color(Color::zero())
    }
}

pub trait ThemeColor {
    fn id() -> ColorId;
    fn store() -> ShVarStore {
        ShVarStore::UniformColor(Self::id())
    }
    fn set(cx: &mut Cx, value: Color) {
        cx.theme_colors.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Color) {
        cx.theme_colors.insert((Self::id(), class), value);
    }
    fn get(cx: &Cx) -> Color {
        *cx.theme_colors.get(&(Self::id(), ThemeBase::id())).expect("Cannot find ColorId")
    }
    fn get_class(cx: &Cx, class: ClassId) -> Color {
        if let Some(color) = cx.theme_colors.get(&(Self::id(), class)) {return *color};
        Self::get(cx)
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
    pub fn get(&self, cx: &Cx) -> TextStyle {
        cx.theme_text_styles.get(&(*self, ThemeBase::id())).expect("Cannot find TextStyle").clone()
    }
    pub fn get_class(&self, cx: &Cx, class: ClassId) -> TextStyle {
        if let Some(ts) = cx.theme_text_styles.get(&(*self, class)) {return ts.clone()};
        self.get(cx)
    }
}

pub trait ThemeTextStyle {
    fn id() -> TextStyleId;
    fn set(cx: &mut Cx, value: TextStyle) {
        cx.theme_text_styles.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: TextStyle) {
        cx.theme_text_styles.insert((Self::id(), class), value);
    }
    fn get(cx: &Cx) -> TextStyle {
        cx.theme_text_styles.get(&(Self::id(), ThemeBase::id())).expect("Cannot find TextStyle").clone()
    }
    fn get_class(cx: &Cx, class: ClassId) -> TextStyle {
        if let Some(ts) = cx.theme_text_styles.get(&(Self::id(), class)) {return ts.clone()};
        Self::get(cx)
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
    pub fn get(&self, cx: &Cx) -> Layout {
        *cx.theme_layouts.get(&(*self, ThemeBase::id())).expect("Cannot find LayoutId")
    }
    pub fn get_class(&self, cx: &Cx, class: ClassId) -> Layout {
        if let Some(l) = cx.theme_layouts.get(&(*self, class)) {return *l};
        self.get(cx)
    }
}

pub trait ThemeLayout {
    fn id() -> LayoutId;
    fn set(cx: &mut Cx, value: Layout) {
        cx.theme_layouts.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Layout) {
        cx.theme_layouts.insert((Self::id(), class), value);
    }
    fn get(cx: &Cx) -> Layout {
        *cx.theme_layouts.get(&(Self::id(), ThemeBase::id())).expect("Cannot find Layout")
    }
    fn get_class(cx: &Cx, class: ClassId) -> Layout {
        if let Some(l) = cx.theme_layouts.get(&(Self::id(), class)) {return *l};
        Self::get(cx)
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
    pub fn get(&self, cx: &Cx) -> Walk {*cx.theme_walks.get(&(*self, ThemeBase::id())).expect("Cannot find WalkId")}
    pub fn get_class(&self, cx: &Cx, class: ClassId) -> Walk {
        if let Some(w) = cx.theme_walks.get(&(*self, class)) {return *w};
        self.get(cx)
    }
}

pub trait ThemeWalk {
    fn id() -> WalkId;
    fn set(cx: &mut Cx, value: Walk) {
        cx.theme_walks.insert((Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Walk) {
        cx.theme_walks.insert((Self::id(), class), value);
    }
    fn get(cx: &Cx) -> Walk {
        *cx.theme_walks.get(&(Self::id(), ThemeBase::id())).expect("Cannot find WalkId")
    }
    fn get_class(cx: &Cx, class: ClassId) -> Walk {
        if let Some(w) = cx.theme_walks.get(&(Self::id(), class)) {return *w};
        Self::get(cx)
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
    pub fn get(&self, cx: &Cx) -> Anim {cx.theme_anims.get(&(*self, ThemeBase::id())).expect("Cannot find WalkId").clone()}
    pub fn get_class(&self, cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&(*self, class)) {return a.clone()};
        self.get(cx)
    }
}

pub trait ThemeAnim {
    fn id() -> AnimId;
    fn set(cx: &mut Cx, value: Anim) {
        cx.theme_anims.insert(( Self::id(), ThemeBase::id()), value);
    }
    fn set_class(cx: &mut Cx, class: ClassId, value: Anim) {
        cx.theme_anims.insert(( Self::id(), class), value);
    }
    fn get(cx: &Cx) -> Anim {
        cx.theme_anims.get(&( Self::id(), ThemeBase::id())).expect("Cannot find WalkId").clone()
    }
    fn get_class(cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&( Self::id(), class)) {return a.clone()};
        Self::get(cx)
    }
}

#[macro_export]
macro_rules!theme_anim {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $ name();
        impl ThemeAnim for $ name {
            pub fn id() -> AnimId {AnimId(std::any::TypeId::of::< $ name>())}
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
