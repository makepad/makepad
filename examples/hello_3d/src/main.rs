//! hello_3d: a lit cube that slowly turns above a ground slab. Drag to orbit
//! the camera, use the wheel to zoom. It is meant to be read top to bottom.
//!
//! How a 3D scene gets into a Makepad window
//!
//! Makepad's UI is 2D. Widgets draw quads and text into the window's pass with
//! a flat camera. A 3D scene uses the same shaders and instances, but it is
//! drawn into a pass of its own:
//!
//! - A pass (`DrawPass`) is one GPU render of some draw lists into some
//!   textures. The window has one. We add a child pass, so the GPU renders the
//!   scene first and the window can use its result in the same frame.
//! - The pass writes a colour texture, which is the picture, and a depth
//!   texture. For every pixel the depth texture holds how far away the nearest
//!   surface drawn so far is. The GPU keeps a new pixel only if it is nearer.
//!   That is how the front of the cube hides its back, whatever the draw order.
//! - The camera is two matrices. The view matrix moves the world so that the
//!   camera sits at the origin. The projection matrix adds perspective, so far
//!   things look smaller. `DrawPass::set_camera` stores both in the pass's
//!   uniforms, and every shader in the pass reads them as
//!   `draw_pass.camera_view` and `draw_pass.camera_projection`.
//! - The colour texture is then just an image. An ordinary 2D quad in the
//!   widget's rectangle shows it, so the 3D view sits in the layout like any
//!   other widget.
//!
//! The sections follow the order things happen: 1. the mesh, 2. the shader,
//! 3. the camera, 4. the widget and its pass, 5. drawing each frame,
//! 6. animation and input, 7. the app.
//!
//! Things to try
//! - Move the light: change `light_dir` in the shader.
//! - Change the field of view in `OrbitCamera::projection` (try 20 and 90).
//! - Swap the two draws in `draw_scene`: the picture stays the same, because
//!   the depth texture, not the draw order, decides what is in front.
//! - Add a second cube: set `model` and `color`, then call `draw` once more.
//!
//! Where to go next: examples/box3d drops a pile of bodies with Box3D
//! physics in this same kind of viewport, and examples/cad is a full CAD
//! viewport.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

// Makepad's script DSL, Splash, declares shaders and UI. This block holds the
// shader (section 2), the 3D view's settings (section 4) and the window
// (section 7). The Rust code for each part follows below.
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    // 2. The shader. A draw shader has two stages. `vertex` runs once per
    // vertex and says where on screen it lands. `pixel` runs once per covered
    // pixel and says what colour it is.
    mod.draw.DrawLitMesh = mod.std.set_type_default() do #(DrawLitMesh::script_shader(vm)){
        // Solid surfaces: no blending, and skip triangles that face away
        // from the camera. They are on the hidden far side.
        alpha_blend: false
        backface_culling: true

        // What the shader writes: the vertex's clip position, and the pixel's
        // colour (into the pass's colour texture).
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        // What it reads. `draw_pass` carries the camera matrices. The mesh
        // comes in as `geom`, in the IcoVertex layout (a position and a
        // normal per vertex); Rust swaps in our cube when it draws. `model`
        // and `color` come from the Rust struct `DrawLitMesh`.
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)
        // The direction towards the light, in world space, the same for
        // every instance (a uniform).
        light_dir: uniform(vec3(0.5, 0.8, 0.3))

        // Values handed from vertex to pixel, interpolated across each triangle.
        world_pos: varying(vec3f)
        world_normal: varying(vec3f)

        vertex: fn() {
            // Mesh space to world space: where this instance is and how it
            // is turned. A position has w = 1, so the move applies to it.
            // A normal has w = 0, so it only turns.
            let world = self.model * self.geom.pos
            self.world_pos = world.xyz
            self.world_normal = (self.model * self.geom.normal).xyz
            // World space to camera space, then to clip space. The GPU
            // divides by w to make far things smaller.
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            // Interpolation shortens the normal, so make it unit length again.
            let normal = normalize(self.world_normal)
            let to_light = normalize(self.light_dir)
            // The camera's position: the inverse view matrix maps the
            // camera-space origin back into the world.
            let camera_pos = (self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            let to_camera = normalize(camera_pos - self.world_pos)
            // Ambient: a little light everywhere, so shadowed faces are not black.
            let ambient = 0.25
            // Diffuse (Lambert): the more a face turns to the light, the brighter.
            let diffuse = max(dot(normal, to_light), 0.0) * 0.75
            // Specular (Blinn-Phong): a highlight where the surface reflects
            // the light towards the camera. The exponent sets its size.
            let half_dir = normalize(to_light + to_camera)
            let specular = pow(max(dot(normal, half_dir), 0.0), 40.0) * 0.35
            let rgb = self.color.xyz * (ambient + diffuse) + vec3(specular, specular, specular)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // 4. The 3D view's settings. `draw_image` is the plain 2D quad that shows
    // the scene: we give it a texture slot and a pixel stage that samples it.
    mod.widgets.Viewport3d = set_type_default() do #(Viewport3d::register_widget(vm)){
        width: Fill
        height: Fill
        draw_image +: {
            scene_texture: texture_2d(float)
            pixel: fn() {
                return self.scene_texture.sample(self.pos)
            }
        }
    }

    // 7. The window: a line of help text, and the 3D view filling the rest.
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 640)
                body +: {
                    flow: Down
                    Label{
                        margin: 10
                        text: "hello_3d    drag: orbit    wheel: zoom"
                    }
                    viewport := mod.widgets.Viewport3d{}
                }
            }
        }
    }
}

// 1. The mesh. A mesh is a list of vertices and a list of triangles. Each
// vertex is eight floats in the IcoVertex layout: position x, y, z, w (w = 1)
// then normal x, y, z, w (w = 0). The normal is the direction the surface
// faces; the lighting needs it. Each triangle is three indices into the
// vertex list.
//
// A cube has 8 corners but we make 24 vertices, 4 per face. That way each
// face has its own normal and is shaded flat, with sharp edges. The cube
// spans -1 to 1 on every axis.
fn build_cube_mesh(cx: &mut Cx) -> Geometry {
    // Per face: the direction it faces, and two directions along it. They are
    // ordered so that the corners below go counter-clockwise when seen from
    // outside. That is how the GPU tells a triangle's front from its back.
    let faces = [
        (vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0)),
        (vec3(-1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 0.0)),
        (vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0)),
        (vec3(0.0, -1.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)),
        (vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)),
        (vec3(0.0, 0.0, -1.0), vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0)),
    ];
    let mut vertices: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (normal, along_u, along_v) in faces {
        let first = (vertices.len() / 8) as u32;
        // The face's centre is at `normal`. Each corner is one step from
        // the centre along u and one step along v.
        let corners: [(f32, f32); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        for (u, v) in corners {
            let corner = normal + along_u * u + along_v * v;
            vertices.extend_from_slice(&[corner.x, corner.y, corner.z, 1.0]);
            vertices.extend_from_slice(&[normal.x, normal.y, normal.z, 0.0]);
        }
        // Two triangles make the square face.
        indices.extend_from_slice(&[first, first + 1, first + 2]);
        indices.extend_from_slice(&[first, first + 2, first + 3]);
    }
    let mesh = Geometry::new(cx);
    mesh.update(cx, indices, vertices);
    mesh
}

// 2. The shader, Rust side. The fields after `#[deref]` are per-instance
// inputs, read in the shader as `self.model` and `self.color`. They are
// copied in this order into the GPU's instance buffer, so the struct is
// `#[repr(C)]`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLitMesh {
    #[deref]
    draw_vars: DrawVars,
    // Mesh space to world space: this instance's position, turn and size.
    #[live]
    model: Mat4f,
    #[live]
    color: Vec4f,
}

impl DrawLitMesh {
    // Adds one instance of `mesh` with the current `model` and `color`.
    // Instances of the same mesh in a row share one GPU draw call.
    fn draw(&mut self, cx: &mut CxDraw, mesh: &Geometry) {
        self.draw_vars.geometry_id = Some(mesh.geometry_id());
        if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&mut self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

// 3. The camera. An orbit camera circles the origin. Its state is two angles
// and a distance, and from those it makes the two matrices the shader uses.
struct OrbitCamera {
    // Around the vertical axis, in radians.
    yaw: f32,
    // Above (positive) or below the horizon, in radians.
    pitch: f32,
    // From the origin.
    distance: f32,
}

impl Default for OrbitCamera {
    // Where the camera starts: a little to the side and above the cube.
    fn default() -> Self {
        Self { yaw: 0.6, pitch: 0.4, distance: 11.0 }
    }
}

impl OrbitCamera {
    fn position(&self) -> Vec3f {
        // Two angles and a distance to x, y, z (spherical coordinates).
        vec3(
            self.distance * self.pitch.cos() * self.yaw.sin(),
            self.distance * self.pitch.sin(),
            self.distance * self.pitch.cos() * self.yaw.cos(),
        )
    }

    // World space to camera space: moves the world so that the camera is at
    // the origin, looking at the cube, with +y up.
    fn view(&self) -> Mat4f {
        Mat4f::look_at(self.position(), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0))
    }

    // Camera space to clip space: a 40 degree field of view from top to
    // bottom, the view's width/height ratio, and the nearest and farthest
    // distances that the depth texture covers.
    fn projection(&self, aspect: f32) -> Mat4f {
        Mat4f::perspective(40.0, aspect, 0.1, 100.0)
    }
}

// 4. The widget and its pass. The widget owns everything the scene needs.
#[derive(Script, Widget)]
pub struct Viewport3d {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    // The quad that shows the scene. Redraws and pointer hits use it.
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    draw_mesh: DrawLitMesh,
    #[live]
    draw_image: DrawQuad,
    #[new]
    pass: DrawPass,
    // The pass's own list of draw calls, separate from the window's.
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    cube_mesh: Option<Geometry>,
    #[rust]
    camera: OrbitCamera,
    // The cube's turn about the vertical axis, in radians.
    #[rust]
    spin: f32,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_drag_pos: Vec2d,
}

impl ScriptHook for Viewport3d {
    // Runs once, when the widget is created: build the mesh and set up the pass.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.cube_mesh = Some(build_cube_mesh(cx));
            // `#[new]` gave us placeholder textures; make the real ones. Both
            // follow the pass's size (`TextureSize::Auto`), which section 5 sets.
            // The colour texture has 8 bits each of blue, green, red, alpha.
            // The depth texture has one 32-bit float per pixel.
            self.color_texture = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
            );
            self.depth_texture = Texture::new_with_format(
                cx,
                TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
            );
            // Each frame starts by clearing: the colour to the background, and
            // the depth to 1.0, the farthest it can be, so the first surface
            // drawn at a pixel always wins.
            let background = vec4(0.07, 0.08, 0.1, 1.0);
            self.pass.set_color_texture(cx, &self.color_texture, DrawPassClearColor::ClearWith(background));
            self.pass.set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
            // Keep the camera that section 5 gives the pass. Otherwise the
            // pass would use the flat 2D camera that UI passes get.
            self.pass.set_keep_camera_matrix(cx, true);
            // Ask for the first animation frame (section 6).
            self.next_frame = cx.new_next_frame();
        });
    }
}

// 5. Drawing each frame.
impl Viewport3d {
    fn draw_scene(&mut self, cx: &mut Cx2d) {
        let Some(cube_mesh) = &self.cube_mesh else {
            return;
        };
        // The cube: turned about the vertical axis by the animation.
        self.draw_mesh.model = Mat4f::rotation(vec3(0.0, self.spin, 0.0));
        self.draw_mesh.color = vec4(0.95, 0.55, 0.2, 1.0);
        self.draw_mesh.draw(cx, cube_mesh);
        // The ground: the same mesh, scaled to a wide thin slab below the
        // cube. We draw it after the cube on purpose. The depth test still
        // keeps the cube in front of the slab's far half.
        self.draw_mesh.model = Mat4f::nonuniform_scaled_translation(
            vec3(2.4, 0.08, 2.4),
            vec3(0.0, -1.3, 0.0),
        );
        self.draw_mesh.color = vec4(0.36, 0.42, 0.5, 1.0);
        self.draw_mesh.draw(cx, cube_mesh);
    }
}

impl Widget for Viewport3d {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Our rectangle in the layout, like any widget's.
        let rect = cx.walk_turtle(walk);
        if rect.size.x < 1.0 || rect.size.y < 1.0 {
            return DrawStep::done();
        }
        // Give the pass this frame's camera.
        let aspect = (rect.size.x / rect.size.y) as f32;
        self.pass.set_camera(cx, self.camera.view(), self.camera.projection(aspect));

        // Record the scene into the pass. `make_child_pass` makes the window
        // wait for it, so the scene is rendered before the window uses it.
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.draw_list.begin_always(cx);
        self.draw_scene(cx);
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);

        // Back in the window's 2D pass: show the colour texture on a quad
        // over our rectangle. Then size the pass, and so its textures, to
        // that quad.
        self.draw_image.draw_vars.set_texture(0, &self.color_texture);
        self.draw_image.draw_abs(cx, rect);
        self.area = self.draw_image.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }

    // 6. Animation and input.
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Each frame: turn the cube by the time since the app started, half a
        // radian per second, then ask for the next frame.
        if let Some(frame) = self.next_frame.is_event(event) {
            self.spin = (frame.time * 0.5) as f32;
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }
        // Pointer input over our quad. Dragging turns the camera about the
        // cube; it stops just short of straight up or down, where "up" for
        // the view matrix would be undefined.
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerDown(fe) => {
                self.last_drag_pos = fe.abs;
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                let delta = fe.abs - self.last_drag_pos;
                self.last_drag_pos = fe.abs;
                self.camera.yaw -= delta.x as f32 * 0.01;
                self.camera.pitch = (self.camera.pitch + delta.y as f32 * 0.01).clamp(-1.5, 1.5);
                self.area.redraw(cx);
            }
            Hit::FingerUp(_) => cx.set_cursor(MouseCursor::Grab),
            // The wheel zooms: each step moves the camera 10% closer or farther.
            Hit::FingerScroll(fe) if fe.scroll.y != 0.0 => {
                let step = if fe.scroll.y > 0.0 { 1.1 } else { 1.0 / 1.1 };
                self.camera.distance = (self.camera.distance * step).clamp(3.0, 40.0);
                self.area.redraw(cx);
            }
            _ => {}
        }
    }
}

// 7. The app: it registers the widget library and this file's script block,
// then passes every event to the UI tree.
#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
