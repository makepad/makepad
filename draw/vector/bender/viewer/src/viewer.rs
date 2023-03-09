use crate::constants::*;
use gl::types::{GLenum, GLuint};
use glutin::dpi::LogicalSize;
use glutin::event::{Event, WindowEvent};
use glutin::event_loop::{ControlFlow, EventLoop};
use glutin::window::WindowBuilder;
use glutin::{ContextBuilder, PossiblyCurrent, WindowedContext};
use std::ffi::{CStr, CString};
use std::mem;
use std::ptr;

pub struct Viewer {
    event_loop: EventLoop<()>,
    windowed_context: WindowedContext<PossiblyCurrent>,
    program: GLuint,
}

impl Viewer {
    pub fn new() -> Self {
        let event_loop = EventLoop::new();
        let windowed_context = ContextBuilder::new()
            .with_vsync(true)
            .build_windowed(
                WindowBuilder::new()
                    .with_title("viewer")
                    .with_inner_size(LogicalSize::new(512.0, 512.0)),
                &event_loop,
            )
            .unwrap();
	let windowed_context = unsafe { windowed_context.make_current().unwrap() };
        unsafe {
            gl::load_with(|symbol| windowed_context.get_proc_address(symbol) as *const _);
            gl::Enable(gl::BLEND);
            gl::BlendEquationSeparate(gl::FUNC_ADD, gl::FUNC_ADD);
            gl::BlendFuncSeparate(gl::SRC_ALPHA, gl::DST_ALPHA, gl::SRC_ALPHA, gl::DST_ALPHA);
            gl::ClearColor(0.0, 0.0, 0.0, 1.0);
            gl::PointSize(8.0);
            gl::LineWidth(2.0);
        }
        Self {
            event_loop,
            windowed_context,
            program: link_program(
                compile_shader(gl::VERTEX_SHADER, VERTEX_SHADER),
                compile_shader(gl::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ),
        }
    }

    pub fn run(self, mut render: impl FnMut() + 'static) {
	let windowed_context = self.windowed_context;
	let program = self.program;
	self.event_loop.run(move |event, _, control_flow| match event {
	    Event::WindowEvent { event, .. } => match event {
		WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
		WindowEvent::Resized(size) => windowed_context.resize(size),
		_ => (),
	    },
            Event::RedrawRequested(_) => {
		unsafe {
		    gl::Clear(gl::COLOR_BUFFER_BIT);
		    gl::UseProgram(program);
		    render();
		}
		windowed_context.swap_buffers().unwrap();
		windowed_context.window().request_redraw();
	    }
	    _ => (),
	});
    }
}

const VERTEX_SHADER: &str = r#"
    #version 100

    precision highp float;
    precision highp int;

    attribute vec2 aPosition;
    attribute vec4 aColor;

    varying vec4 vColor;

    void main() {
        vColor = aColor;
        gl_Position = vec4(aPosition, 0.0, 1.0);
    }
"#;

const FRAGMENT_SHADER: &str = r#"
    #version 100

    precision highp float;
    precision highp int;

    varying vec4 vColor;

    void main() {
        gl_FragColor = vColor;
    }
"#;

fn link_program(vertex_shader: GLuint, fragment_shader: GLuint) -> GLuint {
    unsafe {
        let program = gl::CreateProgram();
        gl::BindAttribLocation(
            program,
            POSITION_ATTRIBUTE,
            CString::new("aPosition").unwrap().as_ptr(),
        );
        gl::BindAttribLocation(
            program,
            COLOR_ATTRIBUTE,
            CString::new("aColor").unwrap().as_ptr(),
        );
        gl::AttachShader(program, vertex_shader);
        gl::AttachShader(program, fragment_shader);
        gl::LinkProgram(program);
        let mut status = mem::uninitialized();
        gl::GetProgramiv(program, gl::LINK_STATUS, &mut status);
        if status == 0 {
            let mut length = mem::uninitialized();
            gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut length);
            let mut log = Vec::with_capacity(length as usize);
            gl::GetProgramInfoLog(program, length, ptr::null_mut(), log.as_mut_ptr());
            log.set_len(length as usize);
            panic!(CStr::from_ptr(log.as_ptr()).to_str().unwrap());
        }
        program
    }
}

fn compile_shader(shader_type: GLenum, string: &str) -> GLuint {
    unsafe {
        let shader = gl::CreateShader(shader_type);
        gl::ShaderSource(
            shader,
            1,
            &CString::new(string).unwrap().as_ptr(),
            ptr::null(),
        );
        gl::CompileShader(shader);
        let mut status = mem::uninitialized();
        gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut status);
        if status == 0 {
            let mut length = mem::uninitialized();
            gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut length);
            let mut log = Vec::with_capacity(length as usize);
            gl::GetShaderInfoLog(shader, length, ptr::null_mut(), log.as_mut_ptr());
            log.set_len(length as usize);
            panic!(CStr::from_ptr(log.as_ptr()).to_str().unwrap());
        }
        shader
    }
}
