use crate::cx::*;
use std::any::TypeId;

// Colors

 
#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ColorId(pub TypeId);

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

impl Into<ColorId> for UniqueId{
    fn into(self) -> ColorId{ColorId(self.0)}
}


// Text



#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct TextStyleId(pub TypeId);

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

impl Into<TextStyleId> for UniqueId{
    fn into(self) -> TextStyleId{TextStyleId(self.0)}
}


// Layout

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct LayoutId(pub TypeId);

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

impl Into<LayoutId> for UniqueId{
    fn into(self) -> LayoutId{LayoutId(self.0)}
}


// Walks


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct WalkId(pub TypeId);

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


impl Into<WalkId> for UniqueId{
    fn into(self) -> WalkId{WalkId(self.0)}
}



// Animations


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct AnimId(pub TypeId);

impl AnimId {
    pub fn set_base(&self, cx: &mut Cx, value: Anim) {
        cx.theme_anims.insert((*self, ClassId::base()), value);
    }
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, value: Anim) {
        cx.theme_anims.insert((*self, class), value);
    }
    pub fn base(&self, cx: &Cx) -> Anim {cx.theme_anims.get(&(*self, ClassId::base())).expect("Cannot find WalkId").clone()}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Anim {
        if let Some(a) = cx.theme_anims.get(&(*self, class)) {return a.clone()};
        self.base(cx)
    }
}

impl Into<AnimId> for UniqueId{
    fn into(self) -> AnimId{AnimId(self.0)}
}


// Shaders

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ShaderId(pub TypeId);

impl ShaderId {
    pub fn set_base(&self, cx: &mut Cx, sg: ShaderGen) {
        let shader = cx.add_shader(sg, &format!("{:?}", self.0));
        cx.theme_shaders.insert((*self, ClassId::base()), shader);
    }
    pub fn set_class(&self, cx: &mut Cx, class: ClassId, sg: ShaderGen) {
        let shader = cx.add_shader(sg, &format!("{:?}", self.0));
        cx.theme_shaders.insert((*self, class), shader);
    }
    pub fn base(&self, cx: &Cx) -> Shader {cx.theme_shaders.get(&(*self, ClassId::base())).expect("Cannot find WalkId").clone()}
    pub fn class(&self, cx: &Cx, class: ClassId) -> Shader {
        if let Some(a) = cx.theme_shaders.get(&(*self, class)) {return a.clone()};
        self.base(cx)
    }
}


impl Into<ShaderId> for UniqueId{
    fn into(self) -> ShaderId{ShaderId(self.0)}
}


// Classes


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ClassId(pub TypeId);

impl Into<ClassId> for UniqueId{
    fn into(self) -> ClassId{ClassId(self.0)}
}


impl ClassId {
    pub fn base() -> ClassId{uid!()}
}
