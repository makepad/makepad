const WEB_RENDER: &str = include_str!("../src/os/web/web_render.rs");
const WEB: &str = include_str!("../src/os/web/web.rs");

#[test]
fn web_render_command_buffers_are_pass_owned() {
    assert!(WEB_RENDER.contains(
        "pub struct CxOsPass {\n    pub(crate) render_cmd_buf: Vec<u32>,\n}"
    ));
    assert_eq!(WEB_RENDER.matches(".os.render_cmd_buf").count(), 6);
    assert!(!WEB.contains("pub(crate) render_cmd_buf: Vec<u32>"));
}

#[test]
fn web_animation_frame_runs_guarded_gc_after_repaint() {
    let frame = WEB
        .split("if let Some(time) = is_animation_frame {")
        .nth(1)
        .unwrap()
        .split("if network_responses.len() != 0")
        .next()
        .unwrap();
    let repaint = frame.find("self.handle_repaint(time);").unwrap();
    let guard = frame.find("if vm.heap().needs_gc()").unwrap();
    let collect = frame.find("vm.gc();").unwrap();
    assert!(repaint < guard && guard < collect);
}
