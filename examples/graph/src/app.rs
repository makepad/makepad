use crate::makepad_live_id::*;
use makepad_widgets::*;
use crate::vectorline::*;
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    import crate::vectorline::VectorLine;
    import crate::vectorline::VectorArc;
    
    App = {{App}} {
        ui: <Window> {
            window: {inner_size: vec2(2000, 1024)},
            caption_bar = {visible: true, caption_label = {label = {text: "Graph"}}},
            body = <Dock> {
                height: Fill,
                width: Fill
                        
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(300.0),
                    a: list,
                    b: split1
                }
                        
                split1 = Splitter {
                    axis: Vertical,
                    align: FromB(200.0),
                    a: graph_view,
                    b: log_view
                }
                        
                list = Tab {
                    name: ""
                    kind: ListView
                }
                        
                graph_view = Tab {
                    name: ""
                    kind: Line1
                }
                        
                log_view = Tab {
                    name: ""
                    kind: LogView
                }
                        
                LogView = <RectView> {
                    draw_bg: {color: #2}
                    height: Fill,
                    width: Fill
                   
                }
                        
                Line1 = <View>{
                    height: Fill,
                    flow: Down,
                    <View>{
                        <VectorArc> {width: Fill, line_width:10., color: #f00, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:20., color: #ff0, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:30., color: #0f0, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:40., color: #00f, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:10., color: #f00, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:20., color: #ff0, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:30., color: #0f0, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, line_width:40., color: #00f, arc_start_corner: TopLeft, arc_end_corner: BottomRight}
                        }
                    <View>{
                    <VectorLine> {width: Fill, line_width:10., color: #f00, line_align: Bottom}
                    <VectorLine> {width: Fill, line_width:20., color: #ff0, line_align: Top}
                    <VectorLine> {width: Fill, line_width:30., color: #0f0, line_align: Left}
                    <VectorLine> {width: Fill, line_width:40., color: #00f, line_align: Right}
                    }
                    <View>
                    {
                        <VectorLine>{line_width: 5., color: #0ff, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 10., color: #ff0, line_align: VerticalCenter}
                        <VectorLine>{line_width: 15., color: #f0f, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 20., color: #8f0, line_align: VerticalCenter}
                        <VectorLine>{line_width: 25., color: #08f, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 30., color: #f08, line_align: VerticalCenter}
                        <VectorLine>{line_width: 35., color: #0ff, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 40., color: #ff0, line_align: VerticalCenter}
                        <VectorLine>{line_width: 45., color: #f0f, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 50., color: #8f0, line_align: VerticalCenter}
                        <VectorLine>{line_width: 55., color: #08f, line_align: HorizontalCenter}
                        <VectorLine>{line_width: 60., color: #f08, line_align: VerticalCenter}
                    }
                    <View>
                    {
                        <VectorLine>{line_width:  5., color: #0ff, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 10., color: #ff0, line_align: DiagonalTopLeftBottomRight}
                        <VectorLine>{line_width: 15., color: #f0f, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 20., color: #8f0, line_align: DiagonalTopLeftBottomRight}
                        <VectorLine>{line_width: 25., color: #08f, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 30., color: #f08, line_align: DiagonalTopLeftBottomRight}
                        <VectorLine>{line_width: 35., color: #0ff, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 40., color: #ff0, line_align: DiagonalTopLeftBottomRight}
                        <VectorLine>{line_width: 45., color: #f0f, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 50., color: #8f0, line_align: DiagonalTopLeftBottomRight}
                        <VectorLine>{line_width: 55., color: #08f, line_align: DiagonalBottomLeftTopRight}
                        <VectorLine>{line_width: 60., color: #f08, line_align: DiagonalTopLeftBottomRight}
                    }
                    <View>
                    {
                        <VectorLine>{line_width: 60., color: #fff, line_align: HorizontalCenter, draw_ls:{ fn stroke(self, side:float, progress: float) -> vec4
                            {
                                return vec4(sin(side+self.time), sin(progress),0,1);
                            }}}
                    }
                 }
                
                        
                ListView = <RectView> {
                    draw_bg: {color: #2}
                    height: Fill,
                    width: Fill
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::vectorline::live_design(cx);
        
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {

    }
}

impl App {
    
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(_next) = self.ui.draw_widget(cx).hook_widget() {
                
            }
            return
        }
        
       
    }
}