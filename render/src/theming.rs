use std::any::TypeId;
use crate::cx::*; 

pub struct CxThemeColors(pub Vec<Color>);
pub struct CxThemeLayouts(pub Vec<Layout>);
pub struct CxThemeTextStyles(pub Vec<TextStyle>);
pub struct CxThemeWalks(pub Vec<Walk>);

impl Cx{
    
    pub fn _set_color(&mut self, val: Color, type_id:TypeId){
        // if we already have it don't alloc it
        let id = if let Some(stored_id) = self.theme_color_to_id.get(&type_id) {
            *stored_id
        }
        else{
            let new_id = self.color_id;
            self.color_id += 1;
            self.colors.0.resize(self.color_id, Color{r:1.0,g:1.0,b:0.0,a:1.0});
            self.theme_color_to_id.insert(type_id, new_id);
            new_id
        };
        self.colors.0[id] = val;
    }
    
    pub fn _set_text_style(&mut self, val: TextStyle, type_id:TypeId){
        let id = if let Some(stored_id) = self.theme_text_style_to_id.get(&type_id) {
            *stored_id
        }
        else{
            let new_id = self.text_style_id;
            self.text_style_id += 1;
            self.text_styles.0.resize(self.text_style_id, TextStyle::default());
            self.theme_text_style_to_id.insert(type_id, new_id);
            new_id
        };
        self.text_styles.0[id] = val;
    }

    pub fn _set_layout(&mut self, val: Layout, type_id:TypeId){
        let id = if let Some(stored_id) = self.theme_text_style_to_id.get(&type_id) {
            *stored_id
        }
        else{
            let new_id = self.layout_id;
            self.layout_id += 1;
            self.layouts.0.resize(self.layout_id, Layout::default());
            self.theme_layout_to_id.insert(type_id, new_id);
            new_id
        };
        self.layouts.0[id] = val;
    }
    
    pub fn _set_walk(&mut self, val: Walk, type_id:TypeId){
        let id = if let Some(stored_id) = self.theme_walk_to_id.get(&type_id) {
            *stored_id
        }
        else{
            let new_id = self.walk_id;
            self.walk_id += 1;
            self.walks.0.resize(self.walk_id, Walk::default());
            self.theme_walk_to_id.insert(type_id, new_id);
            new_id
        };
        self.walks.0[id] = val;
    }
}


// Colors


#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct ColorId(pub usize);

#[derive(Copy, Clone, Debug)]
pub enum ColorPart{
    Color(Color),
    Id(ColorId)
}

impl ColorPart{
    fn color(&self, theme_colors:&CxThemeColors)->Color{
        return match self{
            ColorPart::Color(color)=>*color,
            ColorPart::Id(id) => *theme_colors.index(*id)
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ColorBlend{
    pub a:ColorPart,
    pub b:ColorId,
    pub f:f32
}


impl std::ops::Index<ColorId> for CxThemeColors{
    type Output = Color;
    fn index(&self, color_id:ColorId)->&Self::Output{
        if color_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[color_id.0]
        }
    }
}

impl CxThemeColors{

    pub fn index(&self, color_id:ColorId)->&Color{
        if color_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[color_id.0]
        }
    }

    pub fn blend(&self, fac:ColorBlend)->Color{
        if fac.f<0.00001{
            return fac.a.color(self)
        }
        if fac.f>0.99999{
            return *self.index(fac.b);
        }
        let a = fac.a.color(self);
        let b = self.index(fac.b);
        let of = 1.0-fac.f;
        return Color{r:a.r*of + b.r*fac.f, g:a.g*of + b.g*fac.f, b:a.b*of + b.b*fac.f, a:a.a*of+b.a*fac.f }
    }
    
    pub fn blend_to_part(&self, fac:ColorBlend)->ColorPart{
        if fac.f<0.00001{
            return fac.a
        }
        if fac.f>0.99999{
            return ColorPart::Id(fac.b)
        }
        let a = fac.a.color(self);
        let b = self.index(fac.b);
        let of = 1.0-fac.f;
        return ColorPart::Color(Color{r:a.r*of + b.r*fac.f, g:a.g*of + b.g*fac.f, b:a.b*of + b.b*fac.f, a:a.a*of+b.a*fac.f })
    }
    
}

pub trait ThemeColor{
    fn store()->ShVarStore;
    fn type_id()->std::any::TypeId;
    fn set(cx:&mut Cx, value:Color);
    fn id(cx:&Cx)->ColorId;
}

#[macro_export]
macro_rules!theme_color {
    ( $ name: ident) => {
        pub struct $name();
        impl ThemeColor for $name{
            fn store()->ShVarStore{ShVarStore::UniformColor($name::type_id())}
            fn type_id()->std::any::TypeId{std::any::TypeId::of::<$name>()}
            fn set(cx:&mut Cx, value:Color){cx._set_color(value, $name::type_id())}
            fn id(cx:&Cx)->ColorId{
                let type_id = $name::type_id();
                if let Some(stored_id) = cx.theme_color_to_id.get(&type_id) {
                    ColorId(*stored_id)
                }
                else {
                    panic!("ThemeColor {} not set", stringify!($name))
                }
            }
        }
    };
}



// Text



#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct TextStyleId(pub usize);

impl std::ops::Index<TextStyleId> for CxThemeTextStyles{
    type Output = TextStyle;
    fn index(&self, text_style_id:TextStyleId)->&Self::Output{
        &self.0[text_style_id.0]
    }
}

pub trait ThemeTextStyle{
    fn type_id()->TypeId;
    fn set(cx:&mut Cx, value:TextStyle);
    fn id(cx:&Cx)->TextStyleId;
}

#[macro_export]
macro_rules!theme_text_style {
    ( $ name: ident) => {
        pub struct $name();
        impl ThemeTextStyle for $name{
            fn type_id()->std::any::TypeId{std::any::TypeId::of::<$name>()}
            fn set(cx:&mut Cx, value:TextStyle){cx._set_text_style(value, $name::type_id())}
            fn id(cx:&Cx)->TextStyleId{
                let type_id = $name::type_id();
                if let Some(stored_id) = cx.theme_text_style_to_id.get(&type_id) {
                    TextStyleId(*stored_id)
                }
                else {
                    panic!("ThemeTextStyle {} not set", stringify!($name))
                }
            }
        }
    };
}


// Layout

#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct LayoutId(pub usize);

impl std::ops::Index<LayoutId> for CxThemeLayouts{
    type Output = Layout;
    fn index(&self, layout_id:LayoutId)->&Self::Output{
        &self.0[layout_id.0]
    }
}

pub trait ThemeLayout{
    fn type_id()->TypeId;
    fn set(cx:&mut Cx, value:Layout);
    fn id(cx:&Cx)->LayoutId;
}


#[macro_export]
macro_rules!theme_layout {
    ( $ name: ident) => {
        pub struct $name();
        impl ThemeLayout for $name{
            fn type_id()->std::any::TypeId{std::any::TypeId::of::<$name>()}
            fn set(cx:&mut Cx, value:Layout){cx._set_layout(value, $name::type_id())}
            fn id(cx:&Cx)->LayoutId{
                let type_id = $name::type_id();
                if let Some(stored_id) = cx.theme_layout_to_id.get(&type_id) {
                    LayoutId(*stored_id)
                }
                else {
                    panic!("ThemeLayout {} not set", stringify!($name))
                }
            }
        }
    };
}


// Walks

#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct WalkId(pub usize);

impl std::ops::Index<WalkId> for CxThemeWalks{
    type Output = Walk;
    fn index(&self, walk_id:WalkId)->&Self::Output{
        &self.0[walk_id.0]
    }
}

pub trait ThemeWalk{
    fn walk_type_id()->TypeId;
    fn set(cx:&mut Cx, value:Walk);
    fn id(cx:&Cx)->WalkId;
}


#[macro_export]
macro_rules!theme_walk {
    ( $ name: ident) => {
        pub struct $name();
        impl ThemeWalk for $name{
            fn walk_type_id()->std::any::TypeId{std::any::TypeId::of::<$name>()}
            fn set(cx:&mut Cx, value:Walk){cx._set_walk(value, $name::walk_type_id())}
            fn id(cx:&Cx)->WalkId{
                let type_id = $name::walk_type_id();
                if let Some(stored_id) = cx.theme_walk_to_id.get(&type_id) {
                    WalkId(*stored_id)
                }
                else {
                    panic!("ThemeWalk {} not set", stringify!($name))
                }
            }
        }
    };
}

