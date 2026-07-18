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
