// This is Makepad, a work-in-progress livecoding IDE for 2D Design
// Find the repo and more explanation at github.com/makepad/makepad.
// We are developing the UI kit and code-editor as MIT, but the full stack
// will be a cloud/native app product in a few months.

// Its very early still, but it will soon get hybrid live manipulation
// UI with colorpickers and vector editors.
// All of the code is written in Rust, and it compiles to native and Wasm
// Its built on a novel immediate-mode UI architecture
// The styling is done with shaders written in Rust, transpiled to metal/glsl

// The 2D-graphics livecoding bit is not here yet, but for now enjoy
// how smooth a full GPU editor can scroll (try select scrolling the edge)
// Also the tree fold-animates and the docking panel system works.
// Multi cursor/grid cursor also works with cmd+click / cmd+shift+click

use widgets::*;

struct App{
    view:View<ScrollBar>,
    buttons:Elements<u64, Button, Button>,
    text:Text,
    quad:Quad,
    clickety:u64
}

main_app!(App, "Example");
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        set_dark_style(cx);
        let quad_sh = App::def_quad_shader(cx);
        Self{
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            quad:Quad{
                shader_id:cx.add_shader(quad_sh, "App.quad"),
                ..Style::style(cx)
            },
            text:Text{
                ..Style::style(cx)
            },
            buttons:Elements::new(Button{
                ..Style::style(cx)
            }),
            clickety:0
        }
    }
}

impl App{
    fn def_quad_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({
            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                df_rect(0.,0.,w,h);
                df_fill(vec4(pos.x, pos.y, sin(pos.x),1.));
                df_move_to(0.,0.);
                df_line_to(w,h);
                return df_stroke(color, 4.);
            }
        }));
        sh
    }

    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        self.view.handle_scroll_bars(cx, event);

        for btn in self.buttons.iter(){
            match btn.handle_button(cx, event){
                ButtonEvent::Clicked=>{
                    // boop
                    self.clickety += 1;
                    cx.redraw_area(Area::All);
                },
                _=>()
            }
        }
    }

    fn draw_app(&mut self, cx:&mut Cx){
        
        self.view.begin_view(cx, &Layout{
            padding:Padding{l:10.,t:10.,r:0.,b:0.},
            ..Default::default()
        });


        for i in 0..100{
            self.buttons.get_draw(cx, i, |_cx, templ|{
                templ.clone()
            }).draw_button_with_label(cx, &format!("Btn {}", i));
            if i%10 == 9{
                cx.turtle_new_line()
            }
        }

        cx.turtle_new_line();

        self.text.draw_text(cx, &format!("Hello World {}",self.clickety));
        self.quad.draw_quad_walk(cx, Bounds::Fix(100.),Bounds::Fix(100.), Margin{l:15.,t:0.,r:0.,b:0.});

        self.view.end_view(cx);
    }
}