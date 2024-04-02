use makepad_widgets::{
    makepad_html::HtmlAttribute,
    *,
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 

    // ARIAL_FONT = {
    //     font_size: 9.4,
    //     top_drop: 1.2,
    //     font: {
    //         path: dep("crate://self/resources/Arial-Unicode.ttf")
    //     }
    // }
    // NOTO_SANS_SYMBOLS2_REGULAR = {
    //     font_size: 9.4,
    //     top_drop: 1.2,
    //     font: {
    //         path: dep("crate://self/resources/NotoSansSymbols2-Regular.ttf")
    //     }
    // }
    GO_NOTO_CURRENT_REGULAR = {
        font_size: 12,
        top_drop: 1.2,
        font: {
            path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")
        }
    }
    // GO_NOTO_KURRENT_REGULAR = {
    //     font_size: 9.4,
    //     top_drop: 1.2,
    //     font: {
    //         path: dep("crate://self/resources/GoNotoKurrent-Regular.ttf")
    //     }
    // }
    // APPLE_COLOR_EMOJI = {
    //     font_size: 9.4,
    //     top_drop: 1.2,
    //     font: {
    //         path: dep("crate://self/resources/Apple-Color-Emoji.ttc")
    //     }
    // }

    TextOrImage = {{TextOrImage}}<View> {
        width: Fit, height: Fit,
        flow: Overlay

        text_view = <View> {
            visible: true,
            tv_label = <Label> {
                width: Fit, height: Fit,
                draw_text: {
                    draw_call_group: other_tv_label
                    text_style: <GO_NOTO_CURRENT_REGULAR>{
                        font_size: 12, 
                    }
                    draw_text:{color: #00f}
                }
                text: "Loading image..."
            }
        }

        img_view = <View> {
            visible: true,
            iv_img = <Image> {
                fit: Size,
            }
        }
    }

    HtmlImageTemplate = {{HtmlImage}}<TextOrImage> {
    }

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
                    //return #000
                    // test
                    return mix(#7, #3, self.pos.y);
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
                    text: "Hello world"
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
                    // font_size: 13,
                    // flow: RightWrap,
                    // width: 300.0,
                    // height: Fit,
                    // padding: 5,
                    // line_spacing: 10,
                    Button = <Button> {
                        text: "Hello world"
                    }
                    img = <HtmlImageTemplate> {
                    }

                    body: "
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

/// A view that holds an image or text content, and can switch between the two.
///
/// This is useful for displaying alternate text when an image is not (yet) available
/// or fails to load. It can also be used to display a loading message while an image
/// is being fetched.
#[derive(Live, Widget)]
pub struct TextOrImage {
    #[deref] view: View,
}

impl LiveHook for TextOrImage{
    fn after_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        //_nodes.debug_print(_index, 100);
    }
}

impl Widget for TextOrImage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
       // log!("TextOrImage::draw_walk(): displaying: {:?}, walk: {:?}", self.status(), walk);
        self.view.draw_walk(cx, scope, walk)
    }
}

impl TextOrImage {
    /// Sets the text content, making the text visible and the image invisible.
    ///
    /// ## Arguments
    /// * `text`: the text that will be displayed in this `TextOrImage`, e.g.,
    ///   a message like "Loading..." or an error message.
    pub fn show_text<T: AsRef<str>>(&mut self, text: T) {
        //log!("TextOrImage::show_text(): text: {:?}", text.as_ref());
        self.view.view(id!(img_view)).set_visible(false);
        self.view.view(id!(text_view)).set_visible(true);
        self.view.label(id!(text_view.tv_label)).set_text(text.as_ref());
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
        let img_ref = self.view.image(id!(img_view.iv_img));
        self.view.debug_print_children();
        let res = image_set_function(img_ref);
        if res.is_ok() {
            self.view.view(id!(img_view)).set_visible(true);
            self.view.view(id!(text_view)).set_visible(false);
        }
        res
    }

    /// Returns whether this `TextOrImage` is currently displaying an image or text.
    pub fn status(&mut self) -> DisplayStatus {
        if self.view.view(id!(img_view)).is_visible() {
            return DisplayStatus::Image;
        } else {
            DisplayStatus::Text
        }
    }
}

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
}

/// Whether a `TextOrImage` instance is currently displaying text or an image.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DisplayStatus {
    Text,
    Image,
}


#[derive(Live, Widget)]
struct HtmlImage {
    #[deref] toi: TextOrImage,
    /// The URL of the image to display.
    #[rust] src: String,
    /// Alternate text for the image that should be displayed for
    /// accessibility purposes or in case the image cannot be loaded.
    #[rust] alt: String,
    /// The title of the image, which is displayed as a tooltip upon hover.
    #[rust] title: String,
}

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
}


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
