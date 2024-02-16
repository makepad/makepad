use {
    crate::{
        makepad_html::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    },
    std::rc::Rc,
};

live_design!{
    HtmlBase = {{Html}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
        
    }
}

// this widget has a retained and an immediate mode api
#[derive(Live, Widget)]
pub struct Html {
    #[redraw] #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    #[live] text: Rc<String>,
    #[rust] html: HtmlDoc
} 

// alright lets parse the HTML
impl LiveHook for Html{
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        
    }
}

impl Widget for Html {

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk:Walk)->DrawStep{
        //self.draw_text.draw_walk(cx, walk.with_add_padding(self.padding), self.align, self.text.as_ref());
        DrawStep::done()
    }
    
    fn text(&self)->String{
        "".into()
        //self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, _v:&str){
        //self.text.as_mut_empty().push_str(v);
    }
}

impl Html{
    pub fn push_bold(&mut self){
    }
    
    pub fn pop_bold(&mut self){
    }
    
    pub fn push_italic(&mut self){
    }
    
    pub fn pop_italic(&mut self){
    }
    
    pub fn apply_widget(&mut self, _cx:&mut Cx, _id:&[LiveId;1], _apply:&[LiveNode]){
    }
    
    pub fn draw_text(&mut self, _cx:&mut Cx, _text:&str){
    }
    
    // parse it
    pub fn default_interpreter(&mut self, _cx:&mut Cx, _html:Vec<HtmlNode>){
        /*
        let node = html.walker();
        while let Some(node) = node.next(){
            match node.open_tag(){
                some_id!(a)=>{
                    let href = node.find_attr(id!(href));
                    self.apply_widget(cx, id!(MyWidget), live!{
                        label:(node.find_text().unwrap_or(""))
                    });
                }
                some_id!(strong)=>self.push_bold(),
                some_id!(em)=>self.push_italic(),
                _=>()
            }
            match node.close_tag(){
                some_id!(strong)=>self.pop_bold(),
                some_id!(em)=>self.pop_italic(),
                _=>()
            }
            if let Some(text) = node.text(){
                self.draw_text(cx, text);
            }
        }*/
    }
    
}


/*
#[derive(Clone)]
pub struct HtmlId(pub HtmlString);
impl HtmlId{
    pub fn new(rc:&Rc<String>, start:usize, end:usize)->Self{
        Self(HtmlString(rc.clone(), start, end))
    }
    pub fn as_id(&self)->LiveId{
        LiveId::from_str(&self.0.0[self.0.1..self.0.2])
    }
}

impl fmt::Debug for HtmlId{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}
*/

// HTML Dom tree-flat vector
