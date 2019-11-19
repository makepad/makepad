use render::*;
use widget::*;
use editor::*;

#[derive(Clone)]
pub struct HomePage {
    pub view: ScrollView,
    pub text: Text,
    pub editor: RustEditor,
    pub text_buffer: TextBuffer
}

impl HomePage {
    pub fn proto(cx: &mut Cx) -> Self {
        let mut text_buffer = TextBuffer::default();
        text_buffer.load_from_utf8(cx, &(0..100).map(|_| "Hello World\n").collect::<String>());
        Self {
            view: ScrollView::proto(cx),
            text: Text::proto(cx),
            editor: RustEditor::proto(cx),
            text_buffer: text_buffer
        }
    }
    
    pub fn my_code_editor() -> ClassId {uid!()}
    pub fn color_heading() -> ColorId {uid!()}
    pub fn color_body() -> ColorId {uid!()}
    pub fn layout_main() -> LayoutId {uid!()}
    pub fn text_style_heading() -> TextStyleId {uid!()}
    pub fn text_style_body() -> TextStyleId {uid!()}
    pub fn walk_paragraph() -> WalkId {uid!()}
    
    pub fn theme(cx: &mut Cx) {
        CodeEditor::color_bg().set_class(cx, Self::my_code_editor(), color("#3"));
        CodeEditor::color_gutter_bg().set_class(cx, Self::my_code_editor(), color("#4"));
        
        Self::text_style_heading().set_base(cx, TextStyle {
            font_size: 28.0,
            ..Theme::text_style_normal().base(cx)
        });
        Self::text_style_body().set_base(cx, TextStyle {
            font_size: 10.0,
            height_factor: 2.0,
            ..Theme::text_style_normal().base(cx)
        });
        Self::color_heading().set_base(cx, color("#e"));
        Self::color_body().set_base(cx, color("#b"));
        Self::layout_main().set_base(cx, Layout {
            padding: Padding {l: 10., t: 10., r: 10., b: 10.},
            new_line_padding: 15.,
            line_wrap: LineWrap::NewLine,
            ..Layout::default()
        });
    }
    
    pub fn handle_home_page(&mut self, cx: &mut Cx, event: &mut Event) {
        self.view.handle_scroll_bars(cx, event);
        self.editor.handle_rust_editor(cx, event, &mut self.text_buffer);
    }
    
    pub fn draw_home_page(&mut self, cx: &mut Cx) {
        if self.view.begin_view(cx, Self::layout_main().base(cx)).is_err() {return};
        
        self.text.color = Self::color_heading().base(cx);
        self.text.text_style = Self::text_style_heading().base(cx);
        self.text.draw_text(cx, "Introducing Makepad");
        cx.turtle_new_line();
        
        self.text.color = Self::color_body().base(cx);
        self.text.text_style = Self::text_style_body().base(cx);
        self.text.draw_text(cx, "Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications.");
        cx.turtle_new_line();
        /*
        cx.begin_turtle(Layout{
            walk:Walk::wh(Width::Fix(250.0), Height::Fix(250.)),
            ..Layout::default()
        }, Area::Empty);
        self.editor.code_editor.class = Self::my_code_editor();
        self.editor.draw_rust_editor(cx, &mut self.text_buffer);
        cx.end_turtle(Area::Empty);
        */
        self.text.draw_text(cx, "Today, we launch an early alpha of Makepad Basic. This version shows off the development platform, but does not include the visual design tools or library ecosystem yet. It is intended as a starting point for feedback from you!. Although Makepad is primarily a native application, its UI is perfectly capable of running on the web. If you want to get a taste of what Makepad looks like, play around with the web version, you are looking at it right now. To compile code, you have to install the native version.");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "The Makepad development platform and library ecosystem are MIT licensed, and will be available for free as part of Makepad Basic. In the near future, we will also introduce Makepad Pro, which will be available as a subscription model. Makepad Pro will include the visual design tools, and and live VR environment. Because the library ecosystem is MIT licensed, all applications made with the Pro version are entirely free licensed.");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "Are you excited about Makepad? Do you want to see where we are going? Then sign up <HERE> to receive regular updates.");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "Install makepad locally so you can compile code!:");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "- Step 1. Installing Rust <we could copy the rust site instructions and not link there> ");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "- Step 2. type cargo install <something>");
        cx.turtle_new_line();
        
        self.text.draw_text(cx, "- Step 3. Type your first Rust program in makepad!");
        cx.turtle_new_line();

        self.view.end_view(cx);
    }
}