use makepad_widgets::{
    makepad_html::{HtmlDoc},
    *,
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 

    // config copied from Robrix
    HTML_LINE_SPACING = 8.0
    HTML_TEXT_HEIGHT_FACTOR = 1.3
    MESSAGE_FONT_SIZE = 12.0
    MESSAGE_TEXT_COLOR = #x777
    MESSAGE_TEXT_LINE_SPACING = 1.35
    MESSAGE_TEXT_HEIGHT_FACTOR = 1.5
    // This font should only be used for plaintext labels. Don't use this for Html content,
    // as the Html widget sets different fonts for different text styles (e.g., bold, italic).
    MESSAGE_TEXT_STYLE = {
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
        font_size: (MESSAGE_FONT_SIZE),
        height_factor: (MESSAGE_TEXT_HEIGHT_FACTOR),
        line_spacing: (MESSAGE_TEXT_LINE_SPACING),
    }

    GO_NOTO_CURRENT_REGULAR = {
        font_size: 12,
        top_drop: 1.1,
        font: {
            path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")
        }
    }


    TextOrImage = {{TextOrImage}}{
        margin:{left:10, right:10}
        text_view: <View>{ 
            width: Fill,
            height: Fill,
            label = <Label> {
                width: Fit, height: Fit,
                draw_text: {
                    text_style: <GO_NOTO_CURRENT_REGULAR>{
                        font_size: 12, 
                    }
                }
            }
        }
        image_view:  <View>{
            width: Fill,
            height: Fill,
            image = <Image> {
                width: Fill,
                height: Fill,
                fit: Stretch,
            }
        }
    }
    
    HtmlImage = {{HtmlImage}}<TextOrImage>{}

    // other blue hyperlink colors: #1a0dab, // #0969da  // #0c50d1
    const LINK_COLOR = #x155EEF

    HtmlLink = <HtmlLink> { }

    Html = <Html> { }

    App = {{App}} {

        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill
            
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return #999
                    // test
                    // return mix(#7, #3, self.pos.y);
                }
            }
            
            body = <ScrollXYView>{
                flow: Down,
                spacing:10,
                align: {
                    x: 0.5,
                    y: 0.5
                },

                simple_img = <Image> {
                    width: 272,
                    height: 92,
                    source: dep("crate://self/resources/img/google_logo.png"),
                }
                button1 = <Button> {
                    text: "Hello world "
                }
                input1 = <TextInput> {
                    width: 100, height: 30
                    text: "Click to count"
                }
                label1 = <Label> {
                    draw_text: {
                        draw_call_group: label1_dc
                        color: #f
                    },
                    text: "Counter: 0"
                }

                html = <Html> {
                    // padding: 0.0,
                    line_spacing: (HTML_LINE_SPACING),
                    width: Fill, height: Fit,
                    font_size: (MESSAGE_FONT_SIZE),
                    draw_normal:      { color: (MESSAGE_TEXT_COLOR), text_style: { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) } }
                    draw_italic:      { color: (MESSAGE_TEXT_COLOR), text_style: { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) } }
                    draw_bold:        { color: (MESSAGE_TEXT_COLOR), text_style: { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) } }
                    draw_bold_italic: { color: (MESSAGE_TEXT_COLOR), text_style: { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) } }
                    draw_fixed:       {                              text_style: { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) } }
                    draw_block:{ 
                        line_color: (MESSAGE_TEXT_COLOR)
                        sep_color: (MESSAGE_TEXT_COLOR)
                        quote_bg_color: (#4)
                        quote_fg_color: (#7)
                        block_color: (#3)
                    }
                    list_item_layout: { line_spacing: 5.0, padding: {top: 1.0, bottom: 1.0}, }

                    Button = <Button> {
                        text: "Hello world"
                    }

                    img = <HtmlImage> {
                    }

                    body: "
                    Testing plaintext link: <a href=\"https://www.google.com\">Google multi-word link text here</a>
                    
                    <h2>Header 2 link to <a href=\"https://www.google.com\">Google link is long and might need to wrap</a> </h2>

                    <b>Bold and <i>italic</i> link to <a href=\"https://www.google.com\">Google but the link is really really long and needs to be wrapped multiple times across so many lines that it's hard to handle this</a></b>

                    <h4>Header 4 <b> bold link to <a href=\"https://www.google.com\">Google test1 test2 test3 test4 test5 test6 test7 test8 test9 test10 </a></b></h4>

                    Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed venenatis, lorem sodales lacinia ornare, nisi lorem congue urna, eget dictum urna lacus ut quam. Duis elementum vehicula tellus vel mollis. Vivamus ut orci sed lorem aliquet posuere. Ut sem augue, gravida vitae luctus placerat, vulputate at ligula. Ut sit amet commodo massa. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Pellentesque sodales, eros id dictum bibendum, nisi neque iaculis augue, ac suscipit dolor nisi et velit. Integer dignissim interdum metus. Etiam ultricies nibh eu bibendum ultricies. Maecenas dictum maximus mollis.

                    Morbi sit amet placerat metus. Vivamus eleifend elementum lectus, in dignissim elit pulvinar sed. Nam eleifend a dui condimentum vestibulum. Nam nec orci pretium, sodales orci quis, aliquam dolor. Maecenas id neque tempor, sollicitudin nisi at, bibendum turpis. Donec consectetur tellus a ornare dictum. Mauris mollis laoreet consequat. Pellentesque habitant morbi tristique senectus et netus et malesuada fames ac turpis egestas. Nunc in fermentum velit.
                    
                    Curabitur aliquam lectus id erat sagittis vulputate. Sed aliquet et nisl vel rutrum. Morbi imperdiet condimentum pellentesque. Sed tincidunt facilisis tortor a tincidunt. Nullam congue consequat finibus. Etiam placerat, ipsum vel faucibus commodo, metus velit efficitur felis, sit amet viverra neque sem quis neque. Vivamus convallis tellus aliquet, aliquet turpis non, hendrerit dolor. Mauris vestibulum dictum sem, vel rutrum metus pretium id. Praesent vehicula arcu gravida tortor interdum, mattis tincidunt ante ornare. Quisque sed magna convallis, pretium nisi a, mattis diam. Donec et arcu venenatis, dapibus eros eu, finibus quam. Nullam et luctus tellus, sit amet vehicula neque. Integer pulvinar vitae nunc in placerat. Aliquam sollicitudin sed tellus eu eleifend.
                    
                    Mauris vitae lobortis libero, sed euismod augue. Sed varius pulvinar consectetur. Sed blandit luctus dignissim. Fusce tortor neque, scelerisque ac elit at, scelerisque hendrerit odio. Nullam nec pulvinar nisi, id dignissim est. Fusce aliquet diam libero, eu malesuada nibh fermentum in. Integer tempor auctor magna, a varius quam placerat ut. Aenean sed ex sed velit elementum condimentum. Maecenas ac lobortis ipsum, fermentum auctor felis. Duis sollicitudin id felis ullamcorper euismod. Ut quis ante sed urna semper sodales. Donec sit amet est sagittis, auctor est elementum, consectetur augue. Mauris rhoncus mi a neque malesuada ultrices. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Quisque a erat vel orci rhoncus condimentum vitae vitae odio. Vestibulum facilisis nunc neque, ut porta quam commodo eu.
                    "

                }
            }
        }
    }
}  
              
app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            log!("BUTTON CLICKED {}", self.counter); 
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
        }

        for action in actions {
            if let HtmlLinkAction::Clicked { url, .. } = action.as_widget_action().cast() {
                log!("got HtmlLinkAction::Clicked: url: {:?}", url);
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

////////////////////////////////////////////////////////////////////////////////////
////////////////////////////////////////////////////////////////////////////////////
//////////////////////////// NEW HTML WIDGET STUFF /////////////////////////////////
////////////////////////////////////////////////////////////////////////////////////
////////////////////////////////////////////////////////////////////////////////////

/// A view that holds an image or text content, and can switch between the two.
///
/// This is useful for displaying alternate text when an image is not (yet) available
/// or fails to load. It can also be used to display a loading message while an image
/// is being fetched.

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TextOrImageStatus {
    Text,
    Image,
}

#[derive(Live, Widget, LiveHook)]
pub struct TextOrImage {
    /// The URL of the image to display.
    #[redraw] #[live] text_view: View,
    #[redraw] #[live] image_view: View,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[rust(TextOrImageStatus::Text)] status: TextOrImageStatus,
    #[rust] pixel_width: f64,
    #[rust] pixel_height: f64,    
}

impl Widget for TextOrImage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.image_view.handle_event(cx, event, scope);
        self.text_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, mut walk: Walk) -> DrawStep {
        walk.width = Size::Fixed(self.pixel_width / cx.current_dpi_factor());
        walk.height = Size::Fixed(self.pixel_height / cx.current_dpi_factor());
        cx.begin_turtle(walk, self.layout);
        match self.status{
            TextOrImageStatus::Image=>self.image_view.draw_all(cx, scope),
            TextOrImageStatus::Text=>self.text_view.draw_all(cx, scope)
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

#[derive(Live, Widget)]
pub struct HtmlImage {
    /// The URL of the image to display.
    #[deref] toi: TextOrImage,
    #[rust] src: String,
    #[rust] alt: String,
    #[rust] title: String,    
}

impl Widget for HtmlImage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.toi.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.toi.draw_walk(cx, scope, walk)
    }
}

impl LiveHook for HtmlImage {
    // After an HtmlImage instance has been instantiated ("applied"),
    // populate its struct fields from the `<img>` tag's attributes.
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
                
        //log!("HtmlImage::after_apply(): apply.from: {:?}, apply.scope exists: {:?}", apply.from, apply.scope.is_some());
        match apply.from {
            ApplyFrom::NewFromDoc {..} => {
                // lets get the scope props
                let scope = apply.scope.as_ref().unwrap();
                let doc =  scope.props.get::<HtmlDoc>().unwrap();
                let mut walker = doc.new_walker_with_index(scope.index + 1);
                while let Some((lc, attr)) = walker.while_attr_lc(){
                    match lc {
                        live_id!(src) => self.src = attr.into(),
                        live_id!(alt) => self.alt = attr.into(),
                        live_id!(title) => self.title = attr.into(),
                        live_id!(width) => {
                            if let Ok(width) = attr.parse::<f64>() {
                                self.pixel_width = width
                            }
                        }
                        live_id!(height) => {
                            if let Ok(height) = attr.parse::<f64>() {
                                self.pixel_height = height
                            }
                        }
                        _ => ()
                    }
                }
                // At first, set the image to display the alternate/title text
                // until the image has been fetched and is ready to be displayed.
                self.status = TextOrImageStatus::Text;
                
                let text = if !self.alt.is_empty() {
                    self.alt.as_str()
                } else if !self.title.is_empty() {
                    self.title.as_str()
                } else {
                    "Loading image..."
                };
                
                self.toi.text_view.label(id!(label)).set_text(text);
                
                if !self.src.is_empty() {
                    // temp: just assume a local path URL only for now
                    let mut path = std::env::current_dir().unwrap();
                    path.push(&self.src);
                    //log!("HtmlImage::after_apply(): loading image from path: {:?}", path.to_str().unwrap());
                    let image_ref = self.image_view.image(id!(image));
                    image_ref.load_image_file_by_path(cx, path.to_str().unwrap()).unwrap();
                    self.status = TextOrImageStatus::Image;
                }
            }
            _ => ()
        }
    }
}

/*
impl HtmlImage {
    /// Sets the text content, making the text visible and the image invisible.
    ///
    /// ## Arguments
    /// * `text`: the text that will be displayed in this `TextOrImage`, e.g.,
    ///   a message like "Loading..." or an error message.
    pub fn show_text<T: AsRef<str>>(&mut self, text: T) {
        self.status = DisplayStatus::Text;
        self.text_view.label(id!(tv_label)).set_text(text.as_ref());
    }

    /// Sets the image content, making the image visible and the text invisible.
    ///
    /// ## Arguments
    /// * `image_set_function`: this function will be called with an [ImageRef] argument,
    ///    which refers to the image that will be displayed within this `TextOrImage`.
    ///    This allows the caller to set the image contents in any way they want.
    ///    If `image_set_function` returns an error, no change is made to this `TextOrImage`.
    pub fn show_image<F, E>(&mut self, image_set_function: F) -> Result<(), E>
        where F: FnOnce(ImageRef) -> Result<(), E>
    {
        let img_ref = self.img_view.image(id!(iv_img));
        let res = image_set_function(img_ref);
        if res.is_ok() {
            self.status = DisplayStatus::Image;
        }
        res
    }

    /// Returns whether this `TextOrImage` is currently displaying an image or text.
    pub fn status(&mut self) -> DisplayStatus {
        self.status
    }
}*/
/*
impl TextOrImageRef {
    /// See [TextOrImage::show_text()].
    pub fn show_text<T: AsRef<str>>(&self, text: T) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_text(text);
        }
    }

    /// See [TextOrImage::show_image()].
    pub fn show_image<F, E>(&self, image_set_function: F) -> Result<(), E>
        where F: FnOnce(ImageRef) -> Result<(), E>
    {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_image(image_set_function)
        } else {
            Ok(())
        }
    }

    /// See [TextOrImage::status()].
    pub fn status(&self) -> DisplayStatus {
        if let Some(mut inner) = self.borrow_mut() {
            inner.status()
        } else {
            DisplayStatus::Text
        }
    }
}*/

// Whether a `TextOrImage` instance is currently displaying text or an image.

/*
impl LiveHook for HtmlImage {
    // After an HtmlImage instance has been instantiated ("applied"),
    // populate its struct fields from the `<img>` tag's attributes.
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {

        //log!("HtmlImage::after_apply(): apply.from: {:?}, apply.scope exists: {:?}", apply.from, apply.scope.is_some());
        match apply.from {
            ApplyFrom::NewFromDoc {..} => {
                let scope_attrs: Option<&Vec<HtmlAttribute>> = apply.scope.as_ref()
                    .and_then(|scope| scope.props.get());
                // log!("HtmlImage::after_apply(): SCOPE ATTRS: {:?}", scope_attrs);
                if let Some(attrs) = scope_attrs {
                    for attr in attrs {
                        //log!("HtmlImage::after_apply(): found attr: {:?}", attr);
                        match attr.lc {
                            live_id!(src) => self.src = String::from(&attr.value),
                            live_id!(alt) => self.alt = String::from(&attr.value),
                            live_id!(title) => self.title = String::from(&attr.value),
                            live_id!(width) => {
                                if let Ok(width) = attr.value.parse::<usize>() {
                                    self.toi.apply_over(cx, live!{
                                        width: (width),
                                    });
                                }
                            }
                            live_id!(height) => {
                                if let Ok(height) = attr.value.parse::<usize>() {
                                    self.toi.apply_over(cx, live!{
                                        height: (height),
                                    });
                                }
                            }
                            _ => ()
                        }
                    }
                }
                // At first, set the image to display the alternate/title text
                // until the image has been fetched and is ready to be displayed.
                let text = if !self.alt.is_empty() {
                    self.alt.as_str()
                } else if !self.title.is_empty() {
                    self.title.as_str()
                } else {
                    "Loading image..."
                };
                //log!("setting ImageOrText text: {:?}", text);
                self.toi.show_text(text);

                if !self.src.is_empty() {
                    // temp: just assume a local path URL only for now
                    let mut path = std::env::current_dir().unwrap();
                    path.push(&self.src);
                    //log!("HtmlImage::after_apply(): loading image from path: {:?}", path.to_str().unwrap());
                    let _res = self.toi.show_image(|image_ref|{
                        image_ref.load_image_file_by_path(cx, path.to_str().unwrap())
                    });
                    //log!("HtmlImage::after_apply(): image loaded: {:?}", _res);
                }
            }
            _ => ()
        }
    }
}


impl Widget for HtmlImage {
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        scope: &mut Scope,
    ) {
        // log!("HtmlImage::handle_event(): event: {:?}", event);
        self.toi.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // for now, just draw the alt text in the TextOrImage container
        if false {
            let mut path = std::env::current_dir().unwrap();
            path.push(&self.src);
            let _ = self.toi.show_image(|image_ref|
                image_ref.load_image_dep_by_path(cx, path.to_str().unwrap())
            );
        }

        //log!("HtmlImage::draw_walk(): displaying: {:?}, walk: {:?}", self.toi.status(), walk);
        self.toi.draw_walk(cx, _scope, walk)
    }
    
    fn text(&self)->String{
        self.toi.text()
    }
    
    fn set_text(&mut self, v:&str){
        if !v.is_empty() {
            log!("Error: an HTML <img> tag should not have any text value, but we got {v:?}.");
        }
    }
}*/


    /*
    * TODO: Implement the following tags and their attributes:
    *  font
        * data-mx-bg-color, data-mx-color, color
    *  a
        * name, target, href
        * href value must not be relative, must have a scheme matching one of: https, http, ftp, mailto, magnet
        * web clients should add a `rel="noopener"``attribute
    *  sup
    *  sub
    *  div
    *  table
    *  thead
    *  tbody
    *  tr
    *  th
    *  td
    *  caption
    *  span
        * data-mx-bg-color, data-mx-color, data-mx-spoiler
    *  img
        * width, height, alt, title, src
    *  details
    *  summary.
    *  mx-reply (custom)
    */
