
use makepad_widgets::*;
use std::collections::VecDeque;
use std::time::Instant;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
    
    DrawBlock = {{DrawBlock}}{
        
    }
    
    SnakeGame = {{SnakeGame}}{
        width: Fill,
        height: Fill,
        draw_bg:{
            fn pixel(self)->vec4{
                return #113311;
            }
        }
        draw_snake:{
            fn pixel(self)->vec4{
                let fade_factor = self.data1;
                return mix(#33ff33, #33aa00, fade_factor);
            }
        }
        draw_head:{
            fn pixel(self)->vec4{
                return #66ffff;
            }
        }
        draw_wall: {
            fn pixel(self) -> vec4 {
                return #ff0000;
            }
        }
        draw_food: {
            fn pixel(self) -> vec4 {
                return #ffff00;
            }
        }
    }
            
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                window: {inner_size: vec2(800, 600)},
                body = <View>{
                    show_bg: true,
                    flow: Down,
                    game = <SnakeGame>{
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawBlock {
    #[deref] draw_super: DrawQuad,
    #[live] data1: f32,
}

#[derive(Clone, PartialEq)]
pub enum Field{
    Empty,
    Wall,
    Snake,
    Head,
    Food,
}

#[derive(Live, Widget)]
struct SnakeGame{
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_wall: DrawQuad,
    #[live] draw_snake: DrawBlock,
    #[live] draw_head: DrawQuad,
    #[live] draw_food: DrawQuad,
            
    #[rust] field: Vec<Field>,
    #[rust] snake_body: VecDeque<(usize, usize)>,
    #[rust] snake_head: (usize, usize),
    #[rust] snake_direction: (isize, isize),
    #[rust((32,32))] grid_size: (usize, usize),
    #[rust] game_timer: Timer,
    #[rust] game_over: bool,
    #[rust(Instant::now())] last_food_place_time: Instant,
    #[rust(0u64)] rng_state: u64,
}

impl SnakeGame{
    
    fn simple_rng(&mut self) -> u64 {
        self.rng_state = self.rng_state.wrapping_add(0xdeadbeefdeadbeef);
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        return x.wrapping_mul(0x2545F4914F6CDD1D);
    }
    
    fn place_food(&mut self){
        let (grid_w, grid_h) = self.grid_size;
        let max_attempts = grid_w * grid_h;
        for _ in 0..max_attempts {
            let rand_val = self.simple_rng();
            let x = (rand_val % grid_w as u64) as usize;
            let y = ((rand_val / grid_w as u64) % grid_h as u64) as usize;
            let idx = y * grid_w + x;
            if self.field[idx] == Field::Empty {
                self.field[idx] = Field::Food;
                return;
            }
        }
    }
    
    fn next_tick(&mut self, cx: &mut Cx){
        if self.game_over {
            return;
        }
                
        let (grid_w, grid_h) = self.grid_size;
        let (head_x, head_y) = self.snake_head;
        let (dir_x, dir_y) = self.snake_direction;
                
        let next_x = (head_x as isize + dir_x + grid_w as isize) as usize % grid_w;
        let next_y = (head_y as isize + dir_y + grid_h as isize) as usize % grid_h;
                
        let next_idx = next_y * grid_w + next_x;
                
        let mut ate_food = false;
        
        match self.field[next_idx] {
            Field::Wall | Field::Snake => {
                self.game_over = true;
                self.redraw(cx);
                return;
            }
            Field::Food => {
                ate_food = true;
                self.place_food();
            }
            Field::Empty | Field::Head => {} 
        }
        
        let old_head_idx = head_y * grid_w + head_x;
        self.field[old_head_idx] = Field::Snake;
                
        self.snake_head = (next_x, next_y);
        self.snake_body.push_front(self.snake_head);
        self.field[next_idx] = Field::Head;
                
        if !ate_food {
            if let Some(tail) = self.snake_body.pop_back() {
                if tail != self.snake_head {
                    let tail_idx = tail.1 * grid_w + tail.0;
                    if self.field[tail_idx] != Field::Head {
                        self.field[tail_idx] = Field::Empty;
                    }
                } else {
                    self.snake_body.push_back(tail);
                }
            }
        }
        
        self.redraw(cx);
    }
        
    fn restart_game(&mut self) {
        self.field.clear();
        self.field.resize(self.grid_size.0 * self.grid_size.1, Field::Empty);
        self.snake_body.clear();
                
        self.snake_head = (self.grid_size.0 / 2, self.grid_size.1 / 2);
        self.snake_body.push_front(self.snake_head);
        let head_idx = self.snake_head.1 * self.grid_size.0 + self.snake_head.0;
        self.field[head_idx] = Field::Head;
                

        self.rng_state = Instant::now().duration_since(Instant::now()).as_nanos() as u64;
        
        self.place_food(); 
                
        self.snake_direction = (1, 0);
        self.game_over = false;
        self.last_food_place_time = Instant::now();
        
    }
}

impl LiveHook for SnakeGame{
    fn after_new_from_doc(&mut self, cx:&mut Cx){
        self.restart_game();
        self.game_timer = cx.start_interval(0.1);
    }
}

impl Widget for SnakeGame{
    fn draw_walk(&mut self, cx:&mut Cx2d, _scope:&mut Scope, walk:Walk)->DrawStep{
        self.draw_bg.begin(cx, walk, self.layout);
        let bg_rect = cx.turtle().rect();
        let cell_w = bg_rect.size.x / self.grid_size.0 as f64;
        let cell_h = bg_rect.size.y / self.grid_size.1 as f64;
        let cell_size = dvec2(cell_w, cell_h);
                
        let snake_len = self.snake_body.len();
        
        for y in 0..self.grid_size.1{
            for x in 0..self.grid_size.0{
                let field = &self.field[y * self.grid_size.0 + x];
                let rect = Rect{
                    pos: bg_rect.pos + dvec2(x as f64 * cell_w, y as f64 * cell_h),
                    size: cell_size
                };
                match field{
                    Field::Empty => {}
                    Field::Snake => {
                        let mut fade_factor = 0.0;
                        if snake_len > 1 {
                            if let Some(index) = self.snake_body.iter().position(|&pos| pos == (x,y)) {
                                // Index 0 is head, len-1 is tail tip. We want fade near 1 for tail.
                                // Index 1 (first body part) should have low fade, index len-1 high fade.
                                if index > 0 { // Don't fade the head (it's drawn separately)
                                    fade_factor = (index - 1) as f32 / (snake_len - 2) as f32;
                                }
                            }
                        }
                        self.draw_snake.data1 = fade_factor.max(0.0).min(1.0); // Clamp to [0, 1]
                        self.draw_snake.draw_abs(cx, rect);
                    }
                    Field::Head => {
                        self.draw_head.draw_abs(cx, rect);
                    }
                    Field::Wall => {
                        self.draw_wall.draw_abs(cx, rect);
                    }
                    Field::Food => {
                        self.draw_food.draw_abs(cx, rect);
                    }
                }
            }
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }
            
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, _scope:&mut Scope){
        if self.game_timer.is_event(event).is_some(){
            self.next_tick(cx);
        }
                
        match event.hits(cx, self.draw_bg.area()){
            Hit::KeyDown(ke) => {
                if self.game_over && ke.key_code == KeyCode::Space {
                    self.restart_game();
                    self.game_timer = cx.start_interval(0.1); 
                    self.redraw(cx);
                } else if !self.game_over {
                    let current_dir = self.snake_direction;
                    match ke.key_code {
                        KeyCode::ArrowUp | KeyCode::KeyW if current_dir != (0, 1) => {
                            self.snake_direction = (0,-1);
                        }
                        KeyCode::ArrowDown | KeyCode::KeyS if current_dir != (0, -1) => {
                            self.snake_direction = (0,1);
                        }
                        KeyCode::ArrowLeft | KeyCode::KeyA if current_dir != (1, 0) => {
                            self.snake_direction = (-1,0);
                        }
                        KeyCode::ArrowRight | KeyCode::KeyD if current_dir != (-1, 0) => {
                            self.snake_direction = (1,0);
                        }
                        _=>()
                    }
                }
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
        makepad_widgets::live_design(cx);
        cx.link(live_id!(theme), live_id!(theme_desktop_light));
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
