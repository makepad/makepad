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

    // This is an HTML subwidget used to handle `<font>` and `<span>` tags,
    // specifically: foreground text color, background color, and spoilers.
    MatrixHtmlSpan = {{MatrixHtmlSpan}}<Label> {
        width: Fit,
        height: Fit,

        draw_text: {
            color: (MESSAGE_TEXT_COLOR),
            text_style: <MESSAGE_TEXT_STYLE> { height_factor: (HTML_TEXT_HEIGHT_FACTOR), line_spacing: (HTML_LINE_SPACING) },
            fn get_color(self) -> vec4 {
                return self.color
            }
        }
        text: "MatrixHtmlSpan placeholder",

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
                    return #fff
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

                    img = <HtmlImage> { }
                    font = <MatrixHtmlSpan> { }
                    span = <MatrixHtmlSpan> { }

                    body: "
                        testing direct font color: <font color=#00FF00>green font</font> <font color=\"#FF0000\">red font</font> <br />
                        testing rainbow: <font color=\"#ff00be\">t</font><font color=\"#ff0082\">e</font><font color=\"#ff0047\">s</font><font color=\"#ff5800\">t</font><font color=\"#ff8400\">i</font><font color=\"#ffa300\">n</font><font color=\"#d2ba00\">g</font> <font color=\"#3ed500\">r</font><font color=\"#00dd00\">a</font><font color=\"#00e251\">i</font><font color=\"#00e595\">n</font><font color=\"#00e7d6\">b</font><font color=\"#00e7ff\">o</font><font color=\"#00e6ff\">w</font> <font color=\"#00dbff\">m</font><font color=\"#00ceff\">e</font><font color=\"#00baff\">s</font><font color=\"#769eff\">s</font><font color=\"#f477ff\">a</font><font color=\"#ff3aff\">g</font><font color=\"#ff00fb\">e</font> <br />
                        testing span with colors, purple text on aqua background: <span data-mx-bg-color=\"#00FFFF\" data-mx-color=\"#800080\">this is my funny text color</span> <br />
                        testing spoiler: <span data-mx-spoiler=\"reason here\">hidden spoiler content</span> <br />
                        <br />
                        <sep>
                        test underline: <u>Underlined Text</u> <br />
                        test strikethrough: <del>Strikethrough Text</del> <br />
                        testing nested subscript zero<sub>one<sub>two<sub>three</sub>two</sub>one</sub>zero<br />
                        <br />
                        another test nested subscript 0<sub>1<sub>2<sub>3</sub>2</sub>1</sub>0<br />
                        <br />
                        testing nested superscript one<sup>two<sup>three<sup>four</sup></sup></sup> <br />

                        <sep>
                        text up top with inline image
                        <img src=\"experiments/html_experiment/resources/img/google_logo.png\" width=272 height=92 alt=\"Google Logo\" title=\"Google Logo\" />
                        text after image <br />
                        <ol>
                            <li> list item one </li>
                            <li> list item two </li>
                            <li> list item three</li>
                            <li> list item four </li>
                        </ol>

                        <ol>
                            <li> list item one </li>
                            <li> list item two </li>
                            <li> list item three</li>
                            <li> list item four </li>
                        </ol>

                        <h1> Header 1 </h1>
                        
                        <ol>
                            <li> list item one </li>
                            <li> list item two </li>
                            <li> list item three</li>
                            <li> list item four </li>
                        </ol>
                        plain text at the top <sub> subscript </sub> <sup> superscript </sup> <br />

                        text before link <a href=\"https://www.google.com/\" rel=noopener target=_blank>Go to Google...</a>

                        <ol>
                            <li> list item one </li>
                            <li> list item two </li>
                            <li> list item three</li>
                            <li> list item four </li>

                            <li> 
                                Level 1 list item one
                                <ul>
                                    <li> Level 2 list item one </li>
                                    <li>
                                        Level 2 list item two
                                        <ul>
                                            <li> Level 3 list item one </li>
                                            <li> 
                                                Level 3 list item two 
                                                <ul>
                                                    <li> Level 4 list item one </li>
                                                    <li> Level 4 list item two </li>
                                                </ul>
                                            </li>
                                        </ul>
                                    </li>
                                </ul>
                            </li>
                            <li> list item six </li>
                            <li> list item seven 
                                <ul>
                                    <li> list item ul 7 </li>
                                    <li> list item ul 7 </li>
                                </ul>
                                <ol>
                                    <li> list item ol 7 </li>
                                    <li>
                                        list item ol 7 before
                                        <ul>
                                            <li> list item indent again </li>
                                            <li> list item indent again 2 </li>
                                        </ul>
                                        list item old 7 after
                                        <blockquote>Quoted Text</blockquote>
                                    </li>
                                    <li> list item ol 7, should be 3</li>
                                </ol>
                            </li>

                            <li> list item one </li>
                            <li> text <br />
                                <ol>
                                    <li> list item two </li>
                                    <li> text here
                                        <ul>
                                            <li> list item three </li>
                                            <li> list item four </li>
                                            <li> sub item
                                                <ol>
                                                    <li> list item five </li>
                                                    <li> list item six </li>
                                                </ol>
                                            </li>
                                        </ul>                                    
                                    </li>
                                    <li> list item four </li>
                                </ol>
                            </li>
                            <li> list item three</li>
                            <li> list item four </li>
                        </ol>

                        <h2> Header 2 </h2>

                        <ol type=1 start=16>
                            <li> list item one </li>
                            <li> list item two </li>
                            <li> list item three</li>
                            <li> list item four </li> 
                        </ol>
                        
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


#[derive(Live, Widget)]
pub struct MatrixHtmlSpan {
    /// The URL of the image to display.
    #[deref] ll: Label,
    /// Background color: the `data-mx-bg-color` attribute.
    #[rust] bg_color: Option<Vec4>,
    /// Foreground (text) color: the `data-mx-color` or `color` attributes.
    #[rust] fg_color: Option<Vec4>,
    /// There are three possible spoiler variants:
    /// 1. `None`: no spoiler attribute was present at all.
    /// 2. `Some(empty)`: there was a spoiler but no reason was given.
    /// 3. `Some(reason)`: there was a spoiler with a reason given for it being hidden.
    #[rust] spoiler: Option<String>,
}

impl Widget for MatrixHtmlSpan {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ll.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ll.draw_walk(cx, scope, walk)
    }

    fn set_text(&mut self, v: &str) {
        self.ll.set_text(v);
    }
}

impl LiveHook for MatrixHtmlSpan {
    // After an MatrixHtmlSpan instance has been instantiated ("applied"),
    // populate its struct fields from the `<span>` tag's attributes.
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        // The attributes we care about in `<font>` and `<span>` tags are:
        // * data-mx-bg-color, data-mx-color, data-mx-spoiler, color.

        if let ApplyFrom::NewFromDoc {..} = apply.from {
            if let Some(scope) = apply.scope.as_ref() {
                if let Some(doc) = scope.props.get::<HtmlDoc>() {
                    let mut walker = doc.new_walker_with_index(scope.index + 1);
                    while let Some((lc, attr)) = walker.while_attr_lc(){
                        log!("Looking at attribute: {:?} = {:?}, trimmed {:?}", lc, attr, attr.trim_matches(&['"', '\'']));
                        let attr = attr.trim_matches(&['"', '\'']);
                        match lc {
                            live_id!(color)
                            | live_id!(data-mx-color) => self.fg_color = Vec4::from_hex_str(attr).ok(),
                            live_id!(data-mx-bg-color) => self.bg_color = Vec4::from_hex_str(attr).ok(),
                            live_id!(data-mx-spoiler) => self.spoiler = Some(attr.into()),
                            _ => ()
                        }
                    }

                    log!("MatrixHtmlSpan::after_apply(): fg_color: {:?}, bg_color: {:?}, spoiler: {:?}",
                        self.fg_color, self.bg_color, self.spoiler,
                    );

                    // Set the Label's foreground text color and background color
                    if let Some(fg_color) = self.fg_color {
                        self.ll.apply_over(cx, live!{ draw_text: { color: (fg_color) } });
                    };
                    if let Some(bg_color) = self.bg_color {
                        self.ll.apply_over(cx, live!{ draw_bg: { color: (bg_color) } });
                    };

                    // TODO: need to use a link label to handle the spoiler, so we can toggle it upon click.
                }
            } else {
                warning!("MatrixHtmlSpan::after_apply(): scope not found, cannot set attributes.");
            }
        }
    }
}



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
