
use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    <SnakeGame>{
                        draw_bg:{
                            fn pixel(self)->vec4{
                                return #030
                            }
                        }
                        draw_snake:{
                            fn pixel(self)->vec4{
                                return #077
                            }
                        }
                        draw_head:{
                            fn pixel(self)->vec4{
                                return #0ff
                            }
                        }
                    }
                }
            }
        }
    }
    
    SnakeGame = {{SnakeGame}}{
    }    
}

#[derive(Clone)]
pub enum Field{
    Empty,
    Wall,
    Snake,
    Head
}

#[derive(Live, Widget)]
struct SnakeGame{
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_wall: DrawQuad,
    #[live] draw_snake: DrawQuad,
    #[live] draw_head: DrawQuad,
    #[rust] field: Vec<Field>,
    #[rust] snake_head: (usize,usize),
    #[rust] snake_direction: (isize,isize),
    #[rust(64,64)] grid_size: (usize, usize),
}

impl SnakeGame{
    fn next_tick(&mut self){
        
    }
}

impl LiveHook for SnakeGame{
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        self.field.resize(self.grid_size.0 * self.grid_size.1, Field::Empty);
    }
}

impl Widget for SnakeGame{
    fn draw_walk(&mut self, cx:&mut Cx2d, _scope:&mut Scope, walk:Walk)->DrawStep{
        self.draw_bg.begin(cx, walk, self.layout);
        let bg_rect = cx.turtle().padded_rect();
        // lets draw a snake body
        let cell_size = dvec2(self.grid_size.0 as f64, self.grid_size.1 as f64);
        for y in 0..self.grid_size.1{
            for x in 0..self.grid_size.0{
                let field = &self.field[y* self.grid_size.1 + x];
                let rect = Rect{
                    pos: bg_rect.pos + cell_size * dvec2(x as f64, y as f64),
                    size: cell_size
                };
                match field{
                    Field::Empty=>{}   
                    Field::Snake=>{
                        self.draw_snake.draw_abs(cx, rect);
                    }
                    Field::Head=>{
                        self.draw_head.draw_abs(cx, rect);
                    }
                    Field::Wall=>{
                        self.draw_wall.draw_abs(cx, rect);
                    }
                }
            }
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, _cope:&mut Scope){
        match event.hits(cx, self.draw_bg.area()){
            Hit::KeyDown(ke) if ke.key_code == KeyCode::ArrowUp=>{
                self.snake_direction = (0,-1);
            }
            Hit::KeyDown(ke) if ke.key_code == KeyCode::ArrowDown=>{
                self.snake_direction = (0,1);
            }
            Hit::KeyDown(ke) if ke.key_code == KeyCode::ArrowLeft=>{
                self.snake_direction = (-1,0);
            }
            Hit::KeyDown(ke) if ke.key_code == KeyCode::ArrowRight=>{
                self.snake_direction = (1,0);
            }
            _=>()
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
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
