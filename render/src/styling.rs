use crate::cx::*;
use std::any::TypeId;

impl Cx{
    pub fn begin_style(&mut self, style_id:StyleId){
        // lets fetch the style, if it doesnt exist allocate it
        if let Some(index) = self.style_map.get(&style_id){
            self.style_stack.push(*index);
        }
        else{
            let index = self.styles.len();
            self.style_map.insert(style_id, index);
            self.styles.push(CxStyle::default());
            self.style_stack.push(index);
        }
    }

    pub fn end_style(&mut self){
        self.style_stack.pop().expect("end_style pop failed");
    }

    pub fn get_mut_style_top(&mut self)->&mut CxStyle{
        if let Some(last) = self.style_stack.last(){
            &mut self.styles[*last]
        }
        else{
            &mut self.style_base
        }
    }

}

// floats


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct FloatId(pub TypeId);

impl FloatId {
    pub fn set(&self, cx: &mut Cx, value: f32) {
        cx.get_mut_style_top().floats.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> f32 {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].floats.get(self){
                return *value
            }
        }
        *cx.style_base.floats.get(&*self).expect("Cannot find FloatId")
    }
}

impl Into<FloatId> for UniqueId {
    fn into(self) -> FloatId {FloatId(self.0)}
}



// Colors


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct ColorId(pub TypeId);

impl ColorId {
    pub fn set(&self, cx: &mut Cx, value: Color) {
        cx.get_mut_style_top().colors.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> Color {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].colors.get(self){
                return *value
            }
        }
        *cx.style_base.colors.get(&*self).expect("Cannot find ColorId")
    }
}

impl Into<ColorId> for UniqueId {
    fn into(self) -> ColorId {ColorId(self.0)}
}


// Text



#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct TextStyleId(pub TypeId);

impl TextStyleId {
    pub fn set(&self, cx: &mut Cx, value: TextStyle) {
        cx.get_mut_style_top().text_styles.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> TextStyle {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].text_styles.get(self){
                return *value
            }
        }
        *cx.style_base.text_styles.get(&*self).expect("Cannot find TextStyleId")
    }
}

impl Into<TextStyleId> for UniqueId {
    fn into(self) -> TextStyleId {TextStyleId(self.0)}
}


// Layout

#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct LayoutId(pub TypeId);

impl LayoutId {
    pub fn set(&self, cx: &mut Cx, value: Layout) {
        cx.get_mut_style_top().layouts.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> Layout {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].layouts.get(self){
                return *value
            }
        }
        *cx.style_base.layouts.get(&*self).expect("Cannot find LayoutId")
    }
}

impl Into<LayoutId> for UniqueId {
    fn into(self) -> LayoutId {LayoutId(self.0)}
}


// Walks


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct WalkId(pub TypeId);

impl WalkId {
    pub fn set(&self, cx: &mut Cx, value: Walk) {
        cx.get_mut_style_top().walks.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> Walk {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].walks.get(self){
                return *value
            }
        }
        *cx.style_base.walks.get(&*self).expect("Cannot find WalkId")
    }
}


impl Into<WalkId> for UniqueId {
    fn into(self) -> WalkId {WalkId(self.0)}
}



// Animations


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct AnimId(pub TypeId);

impl AnimId {
    pub fn set(&self, cx: &mut Cx, value: Anim) {
        cx.get_mut_style_top().anims.insert(*self, value);
    }

    pub fn get(&self, cx: &Cx) -> Anim {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].anims.get(self){
                return value.clone()
            }
        }
        cx.style_base.anims.get(&*self).expect("Cannot find AnimId").clone()
    }
}

impl Into<AnimId> for UniqueId {
    fn into(self) -> AnimId {AnimId(self.0)}
}


// Shaders

#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct ShaderId(pub TypeId);

impl ShaderId {
    pub fn set(&self, cx: &mut Cx, sg: ShaderGen) {
        let shader = cx.add_shader(sg, &format!("{:?}", self.0));
        cx.get_mut_style_top().shaders.insert(*self, shader);
    }

    pub fn get(&self, cx: &Cx) -> Shader {
        for style_id in &cx.style_stack{
            if let Some(value) = cx.styles[*style_id].shaders.get(self){
                return *value
            }
        }
        *cx.style_base.shaders.get(&*self).expect("Cannot find AnimId")
    }
}


impl Into<ShaderId> for UniqueId {
    fn into(self) -> ShaderId {ShaderId(self.0)}
}


// Classes


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct StyleId(pub TypeId);

impl Into<StyleId> for UniqueId {
    fn into(self) -> StyleId {StyleId(self.0)}
}


impl StyleId {
    pub fn base() -> StyleId {uid!()}
}
