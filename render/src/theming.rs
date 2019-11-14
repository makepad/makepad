use std::any::TypeId;
use crate::cx::*; 

pub struct CxThemeColors(pub Vec<Color>);
pub struct CxThemeLayouts(pub Vec<Layout>);
pub struct CxThemeTextStyles(pub Vec<TextStyle>);

impl Cx{
    
    pub fn _set_color(&self, val: Color, type_id:TypeId)->ColorId{
        // allocate a pal
        let id = self.color_id;
        self.color_id += 1;
        let theme_color_to_id = self.theme_color_to_id.borrow_mut();
        theme_color_to_id.insert(type_id, id);
        self.colors.0.resize(id + 1, Color{r:1.0,g:1.0,b:0.0,a:1.0});
        self.colors.0[id] = val;
        ColorId(id)
    }
    
    pub fn set_color<F>(&self, val: Color)->ColorId 
    where F: ThemeTextStyle + 'static {
        self._set_color(val, TypeId::of::<F>())
    }

    pub fn _set_text_style(&self, val: TextStyle, type_id:TypeId)->TextStyleId{
        // allocate a pal
        let id = self.text_style_id;
        self.text_style_id += 1;
        let theme_text_style_to_id = self.theme_text_style_to_id.borrow_mut();
        theme_text_style_to_id.insert(type_id, id);
        self.text_styles.0.resize(id + 1, TextStyle::default());
        self.text_styles.0[id] = val;
        TextStyleId(id)
    }

    pub fn set_text_style<F>(&self, val: TextStyle)->TextStyleId
    where F: ThemeTextStyle + 'static {
        self._set_text_style(val, TypeId::of::<F>())
    }

    fn _set_layout(&self, val: Layout, type_id:TypeId)->LayoutId{
        // allocate a pal
        let id = self.layout_id;
        self.layout_id += 1;
        let theme_layout_to_id = self.theme_layout_to_id.borrow_mut();
        theme_layout_to_id.insert(type_id, id);
        self.layouts.0.resize(id + 1, Layout::default());
        self.layouts.0[id] = val;
        LayoutId(id)
    }

    
    pub fn set_layout<F>(&self, val: Layout)->LayoutId
    where F: ThemeLayout + 'static {
        self._set_layout(val, TypeId::of::<F>())
    }
        
    pub fn theme_color<F>(&self) -> ColorId
    where F: ThemeColor + 'static {
        let type_id = TypeId::of::<F>();
        if let Some(stored_id) = self.theme_color_to_id.borrow().get(&type_id) {
            ColorId(*stored_id)
        }
        else {
            self._set_color(F::def(), type_id)
        }
    }
    
    pub fn theme_text_style<F>(&self) -> TextStyleId
    where F: ThemeTextStyle + 'static {
        let type_id = TypeId::of::<F>();
        if let Some(stored_id) = self.theme_text_style_to_id.borrow().get(&type_id) {
            TextStyleId(*stored_id)
        }
        else {
            self._set_text_style(F::def(), type_id)
        }
    }
    
    pub fn theme_layout<F>(&self) -> LayoutId
    where F: ThemeLayout + 'static {
        let type_id = TypeId::of::<F>();
        if let Some(stored_id) = self.theme_layout_to_id.borrow().get(&type_id) {
            LayoutId(*stored_id)
        }
        else {
            self._set_layout(F::def(), type_id)
        }
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
    fn name()->&'static str;
    fn id(cx:&Cx)->ColorId;
    fn def()->Color;
}

#[macro_export]
macro_rules!theme_color {
    ( $ theme_color: ident, $def: expr) => {
        pub struct $theme_color();
        impl ThemeColor for $theme_color{
            fn name()->&'static str{stringify!($theme_color)}
            fn id(cx:&Cx)->ColorId{cx.theme_color::<$theme_color>()}
            fn def()->Color{$def}
        }
    };
}



// Text



#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct TextStyleId(pub usize);

impl std::ops::Index<TextStyleId> for CxThemeTextStyles{
    type Output = TextStyle;
    fn index(&self, text_style_id:TextStyleId)->&Self::Output{
        if text_style_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[text_style_id.0]
        }
    }
}

impl CxThemeTextStyles{

    pub fn index(&self, text_style_id:TextStyleId)->&TextStyle{
        if text_style_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[text_style_id.0]
        }
    }
}

pub trait ThemeTextStyle{
    fn name()->&'static str;
    fn id(cx:&Cx)->TextStyleId;
    fn def()->TextStyle;
}

#[macro_export]
macro_rules!theme_text_style {
    ( $ theme_text_style: ident, $def: expr) => {
        pub struct $theme_text();
        impl ThemeTextStyle for $theme_text_style{
            fn name()->&'static str{stringify!($theme_text_style)}
            fn id(cx:&Cx)->TextStyleId{cx.theme_text_style:<$theme_text_style>()}
            fn def()->TextStyle{$def}
        }
    };
}


// Layout

#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct LayoutId(pub usize);

impl std::ops::Index<LayoutId> for CxThemeLayouts{
    type Output = Layout;
    fn index(&self, layout_id:LayoutId)->&Self::Output{
        if layout_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[layout_id.0]
        }
    }
}

impl CxThemeLayouts{

    pub fn index(&self, layout_id:TextStyleId)->&Layout{
        if layout_id.0 >= self.0.len(){
            &self.0[0]
        }
        else{
            &self.0[layout_id.0]
        }
    }
}

pub trait ThemeLayout{
    fn name()->&'static str;
    fn id(cx:&Cx)->LayoutId;
    fn def()->Layout;
}


#[macro_export]
macro_rules!theme_layout {
    ( $ theme_layout: ident, $def:expr) => {
        pub struct $theme_layout();
        impl ThemeLayout for $theme_layout{
            fn name()->&'static str{stringify!($theme_layout)}
            fn id(cx:&Cx)->LayoutId{cx.theme_layout:<$theme_layout>()}
            fn def()->Layout{$def}
        }
    };
}