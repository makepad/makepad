use crate::makepad_live_id::*;
use makepad_widgets::*;

live_design!{
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_draw::shader::std::*;
    use crate::drawarc::VectorArc;  
    use crate::drawarc::VectorCornerArc;
    use makepad_widgets::vectorline::*;
    
    App = {{App}} {
        ui: <Window> {
            margin: 0
            padding: 0
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
                    a: graph_tabs,
                    b: log_view
                }
                        
                list = Tab {
                    name: ""
                    kind: ListView
                }
                        
                graph_tabs = Tabs {
                    tabs: [arctest, boxtest, linesoverview],
                    selected: 1
                }

                linesoverview = Tab {
                    name: "Overview"
                    kind: Line1
                }

                arctest = Tab {
                    name: "Arc Test"
                    kind: Line2
                }   
                boxtest = Tab {
                    name: "Box Test"
                    kind: Line3
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
                Line3 = <View>{
                    height: Fill,
                    width: 400.,
                    
                    flow: Down,
                    <View>{
                        width:400.,
                 
                        height: 40.

<VectorCornerArc> { line_width:10., color: #ff0, corner: TopLeft}
<VectorLine>{line_width:10. color: #ff0, line_align:Top}
<VectorCornerArc> { line_width:10., color: #ff0, corner: TopRight}
                    }
                    <View>{
                        <VectorLine>{height: Fill, line_width: 10., color: #ff0, line_align: Left}
                        <View>{width: Fill, height: Fill}
                        <VectorLine>{height: Fill, line_width: 10., color: #ff0, line_align: Right}
                        
                    }

                    <View>{
                        width:Fill
                        height: 40.
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomLeft}
<VectorLine>{width:Fill, line_width:10. color: #ff0, line_align:Bottom  }
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomRight}
                    }
                }
                Line2 = <View>{
                    height: Fill,
                    width: Fill;
                    flow: Down,
                    <View>{
                        height: Fill,
                        width: Fill,
                        <VectorArc>
                         {

                            <Animator> {
                                hover = {
                                    default: off
                                    off = {
                                        from: {all: Snap}
                                        apply: {
                                            line_width: 10.0
                                            
                                        }
                                    }
                                    on = {
                                        cursor: Hand
                                        from: {all: Snap}
                                        apply: {
                                            line_width: 20.0
                                        }
                                    }
                                }
                                
                                select = {
                                    default: off
                                    off = {
                                        from: {all: Snap}
                                        apply: {
                                            line_width: 10.0
                                        }
                                    }
                                    on = {
                                        from: {all: Snap}
                                        apply: {
                                            line_width: 50.0
                                        }
                                    }
                                }
                            }

                            width: Fill, height: Fill, line_width:20., color: #8f0, arc_start_corner: TopLeft, arc_end_corner: BottomRight
                        }
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #08f, arc_start_corner: TopRight, arc_end_corner: BottomLeft}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f08, arc_start_corner: BottomRight, arc_end_corner: TopLeft}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f80, arc_start_corner: BottomLeft, arc_end_corner: TopRight}
                        
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #8f0, arc_start_corner: TopLeft, arc_end_corner: TopRight}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #08f, arc_start_corner: TopRight, arc_end_corner: BottomRight}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f08, arc_start_corner: BottomRight, arc_end_corner: BottomLeft}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f80, arc_start_corner: BottomLeft, arc_end_corner: TopLeft}
                   }
                    <View>{
                        height: Fill,
                        width: Fill;
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #8f0, arc_start_corner: TopLeft, arc_end_corner: BottomRight, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #08f, arc_start_corner: TopRight, arc_end_corner: BottomLeft, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f08, arc_start_corner: BottomRight, arc_end_corner: TopLeft, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f80, arc_start_corner: BottomLeft, arc_end_corner: TopRight, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #8f0, arc_start_corner: TopLeft, arc_end_corner: TopRight, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #08f, arc_start_corner: TopRight, arc_end_corner: BottomRight, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f08, arc_start_corner: BottomRight, arc_end_corner: BottomLeft, arc_winding: CounterClockWise}
                        <VectorArc> {width: Fill, height: Fill, line_width:20., color: #f80, arc_start_corner: BottomLeft, arc_end_corner: TopLeft, arc_winding: CounterClockWise}
                    
                    }
                    <VectorLine>{line_width: 60., color: #fff, line_align: HorizontalCenter, draw_ls:{ fn stroke(self, side:float, progress: float) -> vec4
                    {
                        return vec4(1., sin(progress),0,1)* (0.5+0.5*sin(side/(6.283/3.)-self.time*6.283)) + vec4(0.1,0.1,0.1,1.);
                    }}}
                }
                Line1 =
<View>
{
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
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomRight}
}
<View>{

<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
}
<View>{

<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
}
<View>{

<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
}
<View>{

<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
}
<View>{

<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #00f, corner: BottomLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #f00, corner: BottomRight}
<VectorCornerArc> {width: Fill, line_width:10., color: #ff0, corner: TopLeft}
<VectorCornerArc> {width: Fill, line_width:10., color: #0f0, corner: TopRight}
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
                        padding: 30,
                        <VectorLine>{line_width:  1., color: #080, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width:  2., color: #181, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width:  3., color: #282, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width:  4., color: #383, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width:  5., color: #484, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width:  8., color: #585, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width: 15., color: #686, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width: 20., color: #787, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width: 25., color: #888, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width: 30., color: #989, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width: 35., color: #a8a, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width: 40., color: #b8b, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width: 45., color: #c8c, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width: 50., color: #d8d, line_align: DiagonalTopLeftBottomRight, contained: false}
                        <VectorLine>{line_width: 55., color: #e8e, line_align: DiagonalBottomLeftTopRight, contained: false}
                        <VectorLine>{line_width: 60., color: #f8f, line_align: DiagonalTopLeftBottomRight, contained: false}
                    }
                    <View>
                    {
                        <VectorLine>{line_width: 60., color: #fff, line_align: HorizontalCenter, draw_ls:{ fn stroke(self, side:float, progress: float) -> vec4
                            {
                                return vec4(1., sin(progress),0,1)* sin(side/6.283+self.time);
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

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::drawarc::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}