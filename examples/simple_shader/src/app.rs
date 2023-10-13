use makepad_widgets::*;

// the bulk of the app code is identical with the simple example - see comments there
// this example demonstrates two things:
// * writing a simple custom shader
// * creating a custom widget

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;

    // import our custom widget
    // note: to get a custom shader on screen, we could also simply override Window's draw_bg.
    // instead, we go the more elaborate route of overriding the shaders of a custom widget.
    import makepad_example_simple_shader::my_widget::MyWidget;

    App = {{App}} {
        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill
            
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#7,#4,self.pos.y);
                }
            }

            body = <View>{
                 
                flow: Down,
                spacing: 20,
                padding: 30,
                align: {
                    x: 0.5,
                    y: 0.5
                },

                // MyWidget does not implement any shader code itself; instead the shader is overridden here
                quad = <MyWidget> {
                    draw: {
                        // this example shader is ported from kishimisu's tutorial
                        fn pixel(self) -> vec4 {
                            let uv = self.pos - 0.5;
                            let uv0 = uv;
                            let finalColor = vec3(0.0);

                            let i = 0;
                            for _i in 0..4 { // you cannot refer to _i inside the for loop; use i instead
                                uv = fract(uv * 1.5) - 0.5;
                                let d = length(uv) * exp(-length(uv0));
                                let col = Pal::iq2(length(uv0) + float(i) * .4 + self.time * .4);
                                d = sin(d*8. + self.time) / 8.;
                                d = abs(d);
                                d = pow(0.01 / d, 1.2);
                                finalColor += col * d;
                                i = i+1;
                            }

                            return vec4(finalColor ,1);
                        }
                    }
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
        // important: call live design for mywidget
        crate::my_widget::live_design(cx);
    }
}

impl App{
    async fn _do_network_request(_cx:CxRef, _ui:WidgetRef, _url:&str)->String{
        "".to_string()
    }
}

impl AppMain for App{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }

        self.ui.handle_widget_event(cx, event);
    }
}