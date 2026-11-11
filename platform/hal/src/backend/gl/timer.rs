//! A frame's GPU time, from `GL_EXT_disjoint_timer_query`, for `FrameDone`. Results a driver
//! can't have timed right are left out (0), and the timer stays off where its queries don't
//! time whole frames.

use super::sys::{self, GLenum, GLuint, Gl};

pub(super) struct GpuTimer {
    result: unsafe extern "C" fn(GLuint, GLenum, *mut u64),
    free: Vec<GLuint>,
    /// the queries begun and not read yet, each with whether a disjoint event spoiled it
    in_flight: Vec<(GLuint, bool)>,
}

impl GpuTimer {
    pub fn new(gl: &Gl, extensions: &[String]) -> Option<Self> {
        let off = if !extensions.iter().any(|e| e == "GL_EXT_disjoint_timer_query") {
            Some("there's no GL_EXT_disjoint_timer_query")
        } else if gl.string(sys::VENDOR) == "ARM" {
            // A Mali-G71 (driver r19p0) flagged most frames disjoint and timed the rest at 1-2 us, far below their real GPU time.
            Some("Mali's timer queries don't time whole frames")
        } else {
            None
        };
        if let Some(why) = off {
            if crate::backend::log_enabled() { eprintln!("[hal-gl] the GPU timer is off, since {why}"); }
            return None;
        }
        let result = gl.glGetQueryObjectui64vEXT?;
        // Some drivers list the extension without a timer behind it.
        let mut bits = 0;
        unsafe { (gl.glGetQueryiv)(sys::TIME_ELAPSED_EXT, sys::QUERY_COUNTER_BITS_EXT, &mut bits) };
        if bits == 0 { return None; }
        // Reading the disjoint flag clears it, so an old event can't spoil our first queries.
        gl.integer(sys::GPU_DISJOINT_EXT);
        Some(Self { result, free: Vec::new(), in_flight: Vec::new() })
    }

    /// Starts timing what the GPU does for the frame being encoded (the query, and when it began).
    pub fn begin(&mut self, gl: &Gl) -> Option<(GLuint, u64)> {
        let query = self.free.pop().unwrap_or_else(|| {
            let mut query = 0;
            unsafe { (gl.glGenQueries)(1, &mut query) };
            query
        });
        if query == 0 { return None; }
        unsafe { (gl.glBeginQuery)(sys::TIME_ELAPSED_EXT, query) };
        self.in_flight.push((query, false));
        Some((query, crate::clock_ns()))
    }

    pub fn end(&self, gl: &Gl) {
        unsafe { (gl.glEndQuery)(sys::TIME_ELAPSED_EXT) };
    }

    /// The GPU nanoseconds of a frame the GPU finished, or 0 when we can't trust them.
    pub fn read(&mut self, gl: &Gl, query: GLuint, began: u64) -> u64 {
        let Some(at) = self.in_flight.iter().position(|(q, _)| *q == query) else { return 0 };
        let mut available = 0;
        unsafe { (gl.glGetQueryObjectuiv)(query, sys::QUERY_RESULT_AVAILABLE, &mut available) };
        // The query is done, so a disjoint event during it has been flagged by now.
        if gl.integer(sys::GPU_DISJOINT_EXT) != 0 {
            for (_, spoiled) in &mut self.in_flight { *spoiled = true; }
        }
        let (_, spoiled) = self.in_flight.remove(at);
        self.free.push(query);
        if available == 0 || spoiled { return 0; }
        let mut ns = 0;
        unsafe { (self.result)(query, sys::QUERY_RESULT, &mut ns) };
        // Mali can report a result that's empty or longer than the time since the query began, so we skip those.
        if ns > 0 && ns <= crate::clock_ns().saturating_sub(began) { ns } else { 0 }
    }

    pub fn delete(&mut self, gl: &Gl) {
        let queries: Vec<GLuint> = self.free.drain(..).chain(self.in_flight.drain(..).map(|(q, _)| q)).collect();
        if !queries.is_empty() { unsafe { (gl.glDeleteQueries)(queries.len() as i32, queries.as_ptr()) } }
    }
}
