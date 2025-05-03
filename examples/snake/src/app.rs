/* WARNING. AI GENERATED */
use makepad_widgets::*;
use std::collections::VecDeque;
use std::time::Instant;
use std::f32::consts::PI;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
        
    DrawBlock = {{DrawBlock}}{
        data1: 0.0,
        data2: 0.0
    }
        
    SnakeGame = {{SnakeGame}}{
        width: Fill,
        height: Fill,
                
        draw_bg:{
                        
            fn plasma_height(self, p: vec2) -> float {
                let v = 0.0;
                v = v + sin((p.x * 1.5 + self.time) * 2.0);
                v = v + sin((p.y * 1.0 + self.time * 0.5) * 3.0);
                v = v + sin((p.x * 0.8 + p.y * 1.2 + self.time * 0.8) * 1.0);
                v = v + sin(sqrt(p.x * p.x + p.y * p.y) * 2.5 + self.time * 1.2);
                return v / 4.0;
            }
            
            fn plasma_normal(self, p: vec2) -> vec3 {
                let epsilon = 0.005;
                let h = self.plasma_height(p);
                let hx = self.plasma_height(p + vec2(epsilon, 0.0));
                let hy = self.plasma_height(p + vec2(0.0, epsilon));
                let n = normalize(vec3(h - hx, h - hy, epsilon * 2.0));
                return n;
            }
            
            fn pastel_color(self, v: float) -> vec3 {
                let pi = 3.1415926535;
                let r = 0.8 + 0.2 * sin(pi * v);
                let g = 0.8 + 0.2 * sin(pi * v + 2.0 * pi / 3.0);
                let b = 0.8 + 0.2 * sin(pi * v + 4.0 * pi / 3.0);
                return vec3(r, g, b);
            }
            
            fn pixel(self)->vec4{
                let p = self.pos * 8.0;
                let height = self.plasma_height(p);
                let normal = self.plasma_normal(p);
                let light_dir = normalize(vec3(0.7, 0.6, 0.4));
                                
                let diffuse = max(0.0, dot(normal, light_dir));
                let bump_factor = pow(diffuse, 4.0); 
                                
                let base_color = self.pastel_color(height);
                let final_color = base_color * (0.3 + bump_factor * 1.5);
                                
                return vec4(clamp(final_color, 0.0, 1.0), 1.0);
            }
        }
                
        draw_snake:{
                        
            fn sdf_capsule( self, p: vec2, a: vec2, b: vec2, r: float ) -> float{
                let pa = p - a;
                let ba = b - a;
                let h = clamp( dot(pa,ba)/dot(ba,ba), 0.0, 1.0 );
                return length( pa - ba*h ) - r;
            }
            
            fn capsule_normal(self, p: vec2, a: vec2, b: vec2, r: float) -> vec3 {
                let epsilon = 0.001; // Adjusted epsilon
                let dist = self.sdf_capsule(p, a, b, r);
                let grad_x = self.sdf_capsule(p + vec2(epsilon, 0.0), a, b, r) - dist;
                let grad_y = self.sdf_capsule(p + vec2(0.0, epsilon), a, b, r) - dist;
                let n2 = normalize(vec2(grad_x, grad_y));
                let z = sqrt(max(0.0, 1.0 - dot(n2, n2)));
                return normalize(vec3(n2.x, n2.y, z));
            }
            
            fn phong_lighting(self, normal: vec3, view_dir: vec3, light_dir: vec3, albedo: vec3, ambient: vec3, specular_power: float) -> vec3 {
                let diffuse_intensity = max(dot(normal, light_dir), 0.0);
                let diffuse = diffuse_intensity * albedo;
                                
                let reflect_dir = reflect(-light_dir, normal);
                let spec_intensity = pow(max(dot(view_dir, reflect_dir), 0.0), specular_power);
                let specular = spec_intensity * vec3(1.0, 1.0, 1.0); 
                                
                let final_color = ambient + diffuse + specular;
                return final_color;
            }
            
            fn pixel(self)->vec4{
                let p = self.pos;
                                
                let tube_radius = 0.4; // Made the tube thicker
                let half_length = 0.5 - tube_radius; // Ensure ends connect within the cell
                let center = vec2(0.5, 0.5);
                                
                let angle_rad = self.data2; 
                let dir_vec = vec2(cos(angle_rad), sin(angle_rad));
                                
                // Calculate endpoints slightly shorter than the cell diagonal
                let a = center - dir_vec * half_length;
                let b = center + dir_vec * half_length;
                
                let dist = self.sdf_capsule(p, a, b, tube_radius);
                                
                let alpha = smoothstep(0.02, 0.0, dist);
                if alpha < 0.01 {
                    return vec4(0.0, 0.0, 0.0, 0.0);
                }
                
                let normal = self.capsule_normal(p, a, b, tube_radius);
                let view_dir = normalize(vec3(0.0, 0.0, 1.0)); 
                                
                let base_angle = self.time * 0.5;
                let light_dir = normalize(vec3(cos(base_angle), sin(base_angle), 0.8));
                                
                let ambient = vec3(0.1, 0.1, 0.1);
                let specular_power = 32.0;
                
                let color1 = vec3(0.2, 1.0, 0.2);
                let color2 = vec3(0.2, 0.6, 0.0);
                let fade_factor = self.data1;
                let albedo = mix(color1, color2, fade_factor);
                
                let shaded_color = self.phong_lighting(normal, view_dir, light_dir, albedo, ambient, specular_power);
                
                return vec4(shaded_color, alpha);
            }
        }
                
        draw_head:{
            fn sdf_rounded_box(self, p: vec2, b: vec2, r: float) -> float {
                let center = vec2(0.5, 0.5);
                let q = abs(p - center) - b + r;
                return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
            }
                        
            fn rounded_box_normal(self, p: vec2, b: vec2, r: float) -> vec3 {
                let epsilon = 0.0001;
                let dist = self.sdf_rounded_box(p, b, r);
                let grad_x = self.sdf_rounded_box(p + vec2(epsilon, 0.0), b, r) - dist;
                let grad_y = self.sdf_rounded_box(p + vec2(0.0, epsilon), b, r) - dist;
                let n2 = normalize(vec2(grad_x, grad_y));
                let z = sqrt(max(0.0, 1.0 - dot(n2, n2)));
                return normalize(vec3(n2.x, n2.y, z));
            }
            
            fn phong_lighting(self, normal: vec3, view_dir: vec3, light_dir: vec3, albedo: vec3, ambient: vec3, specular_power: float) -> vec3 {
                let diffuse_intensity = max(dot(normal, light_dir), 0.0);
                let diffuse = diffuse_intensity * albedo;
                let reflect_dir = reflect(-light_dir, normal);
                let spec_intensity = pow(max(dot(view_dir, reflect_dir), 0.0), specular_power);
                let specular = spec_intensity * vec3(1.0, 1.0, 1.0); 
                let final_color = ambient + diffuse + specular;
                return final_color;
            }
            
            fn sdf_line(self, p: vec2, a: vec2, b: vec2) -> float {
                let pa = p - a;
                let ba = b - a;
                let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
                return length(pa - ba * h);
            }
            
            fn sdf_circle(self, p: vec2, center: vec2, radius: float) -> float {
                return length(p - center) - radius;
            }
            
            fn pixel(self)->vec4{
                let p = self.pos;
                let box_half_size = vec2(0.48, 0.48);
                let corner_radius = 0.15;
                let dist = self.sdf_rounded_box(p, box_half_size, corner_radius);
                                
                let alpha = smoothstep(0.02, 0.0, dist);
                if alpha < 0.01 {
                    return vec4(0.0, 0.0, 0.0, 0.0);
                }
                
                let normal = self.rounded_box_normal(p, box_half_size, corner_radius);
                let view_dir = normalize(vec3(0.0, 0.0, 1.0)); 
                let angle = self.time * 0.5;
                let light_dir = normalize(vec3(cos(angle), sin(angle), 0.8));
                let ambient = vec3(0.1, 0.1, 0.1);
                let specular_power = 32.0;
                let albedo = vec3(0.4, 1.0, 1.0);
                
                let shaded_color = self.phong_lighting(normal, view_dir, light_dir, albedo, ambient, specular_power);
                
                let eye1_pos = vec2(0.35, 0.4);
                let eye2_pos = vec2(0.65, 0.4);
                let eye_radius = 0.08;
                let eye_dist1 = self.sdf_circle(p, eye1_pos, eye_radius);
                let eye_dist2 = self.sdf_circle(p, eye2_pos, eye_radius);
                                
                let smile_start = vec2(0.3, 0.65);
                let smile_end = vec2(0.7, 0.65);
                let smile_dist = self.sdf_line(p, smile_start, smile_end);
                let smile_thickness = 0.03;
                
                let eye_mask = min(smoothstep(0.01, 0.0, eye_dist1), smoothstep(0.01, 0.0, eye_dist2));
                let smile_mask = smoothstep(smile_thickness + 0.01, smile_thickness, smile_dist);
                
                let final_color = mix(shaded_color, vec3(0.0, 0.0, 0.0), eye_mask);
                let final_color_with_smile = mix(final_color, vec3(0.0, 0.0, 0.0), smile_mask);
                
                return vec4(final_color_with_smile, alpha);
            }
        }
                
        draw_wall: {
            fn pixel(self) -> vec4 {
                return #ff0000;
            }
        }
                
        draw_food: {
            fn sdf_rounded_box(self, p: vec2, b: vec2, r: float) -> float {
                let center = vec2(0.5, 0.5);
                let q = abs(p - center) - b + r;
                return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
            }
            
            fn rounded_box_normal(self, p: vec2, b: vec2, r: float) -> vec3 {
                let epsilon = 0.0001;
                let dist = self.sdf_rounded_box(p, b, r);
                let grad_x = self.sdf_rounded_box(p + vec2(epsilon, 0.0), b, r) - dist;
                let grad_y = self.sdf_rounded_box(p + vec2(0.0, epsilon), b, r) - dist;
                let n2 = normalize(vec2(grad_x, grad_y));
                let z = sqrt(max(0.0, 1.0 - dot(n2, n2)));
                return normalize(vec3(n2.x, n2.y, z));
            }
                         
            fn phong_lighting(self, normal: vec3, view_dir: vec3, light_dir: vec3, albedo: vec3, ambient: vec3, specular_power: float) -> vec3 {
                let diffuse_intensity = max(dot(normal, light_dir), 0.0);
                let diffuse = diffuse_intensity * albedo;
                let reflect_dir = reflect(-light_dir, normal);
                let spec_intensity = pow(max(dot(view_dir, reflect_dir), 0.0), specular_power);
                let specular = spec_intensity * vec3(1.0, 1.0, 1.0); 
                let final_color = ambient + diffuse + specular;
                return final_color;
            }
                        
            fn pixel(self) -> vec4 {
                let p = self.pos;
                let box_half_size = vec2(0.4, 0.4);
                let corner_radius = 0.2;
                let dist = self.sdf_rounded_box(p, box_half_size, corner_radius);
                                
                let alpha = smoothstep(0.02, 0.0, dist);
                if alpha < 0.01 {
                    return vec4(0.0, 0.0, 0.0, 0.0);
                }
                
                let normal = self.rounded_box_normal(p, box_half_size, corner_radius);
                let view_dir = normalize(vec3(0.0, 0.0, 1.0)); 
                let angle = self.time * 0.5;
                let light_dir = normalize(vec3(cos(angle), sin(angle), 0.8));
                let ambient = vec3(0.1, 0.1, 0.0);
                let specular_power = 32.0;
                let albedo = vec3(1.0, 1.0, 0.0);
                
                let shaded_color = self.phong_lighting(normal, view_dir, light_dir, albedo, ambient, specular_power);
                
                return vec4(shaded_color, alpha);
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
                    game = <SnakeGame>{}
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
    #[live] data2: f32,
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
    
    fn find_food_pos(&self) -> Option<(usize, usize)> {
        let (grid_w, _) = self.grid_size;
        self.field.iter().position(|f| *f == Field::Food).map(|idx| (idx % grid_w, idx / grid_w))
    }
    
    fn manhattan_distance(p1: (usize, usize), p2: (usize, usize)) -> usize {
        ((p1.0 as isize - p2.0 as isize).abs() + (p1.1 as isize - p2.1 as isize).abs()) as usize
    }
    
    fn determine_next_direction(&mut self) {
        if self.game_over { return; }
        
        let food_pos_opt = self.find_food_pos();
        if food_pos_opt.is_none() { return; }
        let food_pos = food_pos_opt.unwrap();
        
        let (grid_w, grid_h) = self.grid_size;
        let (head_x, head_y) = self.snake_head;
        let current_dir = self.snake_direction;
        
        let potential_dirs = [(0, -1), (0, 1), (-1, 0), (1, 0)];
        let mut valid_moves = Vec::new();
        
        for dir in potential_dirs {
            if dir.0 == -current_dir.0 && dir.1 == -current_dir.1 {
                if self.snake_body.len() > 1 { continue; }
            }
            
            let next_x = (head_x as isize + dir.0 + grid_w as isize) as usize % grid_w;
            let next_y = (head_y as isize + dir.1 + grid_h as isize) as usize % grid_h;
            let next_idx = next_y * grid_w + next_x;
            
            match self.field[next_idx] {
                Field::Empty | Field::Food => {
                    let distance = Self::manhattan_distance((next_x, next_y), food_pos);
                    valid_moves.push((dir, distance));
                }
                _ => {}
            }
        }
        
        if let Some(best_move) = valid_moves.iter().min_by_key(|(_, dist)| *dist) {
            self.snake_direction = best_move.0;
        } else {
            for dir in potential_dirs {
                if dir.0 == -current_dir.0 && dir.1 == -current_dir.1 {
                    if self.snake_body.len() > 1 { continue; }
                }
                let next_x = (head_x as isize + dir.0 + grid_w as isize) as usize % grid_w;
                let next_y = (head_y as isize + dir.1 + grid_h as isize) as usize % grid_h;
                let next_idx = next_y * grid_w + next_x;
                match self.field[next_idx] {
                    Field::Empty | Field::Food => {
                        self.snake_direction = dir;
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
        
    fn next_tick(&mut self, cx: &mut Cx){
        if self.game_over {
            return;
        }
        
        self.determine_next_direction();
                        
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
                        
        self.rng_state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
        
        self.place_food(); 
                        
        self.snake_direction = (1, 0);
        self.game_over = false;
        self.last_food_place_time = Instant::now();
                
    }
        
    fn angle_diff(a1: f32, a2: f32) -> f32 {
        let mut diff = a1 - a2;
        while diff <= -PI { diff += 2.0 * PI; }
        while diff > PI { diff -= 2.0 * PI; }
        return diff;
    }
    
    fn average_angle(a1: f32, a2: f32) -> f32 {
        let diff = Self::angle_diff(a1, a2);
        let avg = a2 + diff * 0.5;
        return avg.rem_euclid(2.0 * PI);
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
                        let mut segment_angle_rad = 0.0f32;
                                                
                        if let Some(index) = self.snake_body.iter().position(|&pos| pos == (x,y)) {
                            if snake_len > 1 && index > 0 {
                                fade_factor = (index - 1) as f32 / ((snake_len - 1) as f32).max(1.0);
                            }
                            
                            let next_pos = if index > 0 {
                                self.snake_body.get(index - 1).unwrap_or(&self.snake_head)
                            } else {
                                &self.snake_head
                            };
                                                         
                            let current_pos = (x, y);
                            
                            let dx_out = next_pos.0 as f32 - current_pos.0 as f32;
                            let dy_out = next_pos.1 as f32 - current_pos.1 as f32;
                            let angle_out = dy_out.atan2(dx_out);
                            
                            if index < snake_len - 1 {
                                let prev_pos = self.snake_body.get(index + 1).unwrap();
                                let dx_in = current_pos.0 as f32 - prev_pos.0 as f32;
                                let dy_in = current_pos.1 as f32 - prev_pos.1 as f32;
                                let angle_in = dy_in.atan2(dx_in);
                                                                
                                if (Self::angle_diff(angle_in, angle_out)).abs() > 0.1 { 
                                    segment_angle_rad = Self::average_angle(angle_out, angle_in);
                                } else {
                                    segment_angle_rad = angle_out;
                                }
                            } else {
                                segment_angle_rad = angle_out;
                            }
                        }
                                                
                        self.draw_snake.data1 = fade_factor.max(0.0).min(1.0);
                        self.draw_snake.data2 = segment_angle_rad; 
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
                        
        match event{
            Event::KeyDown(ke) => {
                if ke.key_code == KeyCode::Space && self.game_over {
                    self.restart_game();
                    self.game_timer = cx.start_interval(0.1);
                    self.redraw(cx);
                }
            }
            _=>()
        }
        
        match event.hits(cx, self.draw_bg.area()){
            Hit::FingerDown(_fd)=>{
                cx.set_key_focus(self.draw_bg.area());
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
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

