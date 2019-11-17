use std::any::TypeId;
use crate::cx::*; 

// Colors


#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct ColorId(pub TypeId);

impl ColorId{
    pub fn get(&self, cx:&Cx)->Color{*cx.theme_colors.get(self).expect("Cannot find ColorId")}
}

#[derive(Copy, Clone, Debug)]
pub enum ColorPart{
    Color(Color),
    Id(ColorId)
}

impl ColorPart{
    fn color(&self, cx:&Cx)->Color{
        return match self{
            ColorPart::Color(color)=>*color,
            ColorPart::Id(id) =>*cx.theme_colors.get(id).expect("Cannot find ColorId")
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ColorBlend{
    pub a:ColorPart,
    pub b:Option<ColorId>,
    pub f:f32
}

impl ColorBlend{
   
    pub fn blend(&self, cx:&Cx)->Color{
        if self.f<0.00001{
            return self.a.color(cx)
        }
        if let Some(b) = self.b{
            if self.f>0.99999{
                return *cx.theme_colors.get(&b).expect("Cannot find ColorId");
            }
            let a = self.a.color(cx);
            let b = *cx.theme_colors.get(&b).expect("Cannot find ColorId");
            let of = 1.0-self.f;
            return Color{r:a.r*of + b.r*self.f, g:a.g*of + b.g*self.f, b:a.b*of + b.b*self.f, a:a.a*of+b.a*self.f }
        }
        Color::zero()
    }
    
    pub fn blend_to_part(&self, cx:&Cx)->ColorPart{
        if self.f<0.00001{
            return self.a
        }
        if let Some(b) = self.b{
            if self.f>0.99999{
                return ColorPart::Id(b)
            }
            let a = self.a.color(cx);
            let b = *cx.theme_colors.get(&b).expect("Cannot find ColorId");
            let of = 1.0-self.f;
            return ColorPart::Color(Color{r:a.r*of + b.r*self.f, g:a.g*of + b.g*self.f, b:a.b*of + b.b*self.f, a:a.a*of+b.a*self.f })
        }
        ColorPart::Color(Color::zero())
    }
}

#[macro_export]
macro_rules!theme_color {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $name();
        impl $name{
            pub fn store()->ShVarStore{ShVarStore::UniformColor($name::id())}
            pub fn id()->ColorId{ColorId(std::any::TypeId::of::<$name>())}
            pub fn set(cx:&mut Cx, value:Color){cx.theme_colors.insert($name::id(), value);}
            pub fn get(cx:&Cx)->Color{*cx.theme_colors.get(&$name::id()).expect("Cannot find ColorId")}
        }
    };
}



// Text



#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct TextStyleId(pub TypeId);

impl TextStyleId{
    pub fn get(&self, cx:&Cx)->TextStyle{cx.theme_text_styles.get(self).expect("Cannot find TextStyle").clone()}
}

#[macro_export]
macro_rules!theme_text_style {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $name();
        impl $name{
            pub fn id()->TextStyleId{TextStyleId(std::any::TypeId::of::<$name>())}
            pub fn set(cx:&mut Cx, value:TextStyle){cx.theme_text_styles.insert($name::id(), value);}
            pub fn get(cx:&Cx)->TextStyle{cx.theme_text_styles.get(&$name::id()).expect("Cannot find TextStyle").clone()}
        }
    };
}


// Layout

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct LayoutId(pub TypeId);

impl LayoutId{
    pub fn get(&self, cx:&Cx)->Layout{*cx.theme_layouts.get(self).expect("Cannot find LayoutId")}
}


#[macro_export]
macro_rules!theme_layout {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $name();
        impl $name{
            pub fn id()->LayoutId{LayoutId(std::any::TypeId::of::<$name>())}
            pub fn set(cx:&mut Cx, value:Layout){cx.theme_layouts.insert($name::id(), value);}
            pub fn get(cx:&Cx)->Layout{*cx.theme_layouts.get(&$name::id()).expect("Cannot find Layout")}
        }
    };
}


// Walks

#[derive(PartialEq, Copy, Clone, Debug, Hash, Eq)]
pub struct WalkId(pub TypeId);

impl WalkId{
    pub fn get(&self, cx:&Cx)->Walk{*cx.theme_walks.get(self).expect("Cannot find WalkId")}
}

#[macro_export]
macro_rules!theme_walk {
    ( $ name: ident) => {
        #[allow(non_camel_case_types)]
        pub struct $name();
        impl $name{
            pub fn id()->WalkId{WalkId(std::any::TypeId::of::<$name>())}
            pub fn set(cx:&mut Cx, value:Walk){cx.theme_walks.insert($name::id(), value);}
            pub fn get(cx:&Cx)->Walk{*cx.theme_walks.get(&$name::id()).expect("Cannot find WalkId")}
        }
    };
}

