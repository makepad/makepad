//! Main-thread frame bindings for surfaces and graph faces. Frames stay in
//! their provider; a UI names an output with the same `show` declaration it
//! uses for other readouts.

use crate::faces::FaceHost;
use makepad_live_frame::{
    FrameBindError, FrameBinding, FrameSource, LiveFrameView, LiveFrameViewWidgetRefExt,
};
pub use makepad_live_frame::FrameStageWidgetRefExt;
use makepad_widgets::*;
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct FrameSources {
    sources: Vec<Rc<dyn FrameSource>>,
}

impl FrameSources {
    pub fn add(&mut self, source: Rc<dyn FrameSource>) {
        self.sources.push(source);
    }

    fn bind(
        &self,
        instance: &str,
        node: &str,
        port: &str,
    ) -> Result<(Rc<dyn FrameSource>, FrameBinding), FrameBindError> {
        let mut refusal = FrameBindError::NoSession;
        for source in &self.sources {
            match source.bind(instance, node, port) {
                Ok(binding) => return Ok((source.clone(), binding)),
                Err(error) => {
                    // A provider that found the node can give a more useful
                    // reason than an unrelated provider with no such session.
                    if error_rank(&error) > error_rank(&refusal) {
                        refusal = error;
                    }
                }
            }
        }
        Err(refusal)
    }

    /// Called after mounting a face or publishing a live generation. Merely
    /// binding a hidden view does not request rendering: its draw does that.
    pub fn bind_host(&self, cx: &mut Cx, host: &FaceHost) -> Vec<String> {
        let mut errors = Vec::new();
        let prefix = crate::hierarchy::prefix(&host.instance_path);
        for face in host.faces.values().chain(host.flow_face.iter()) {
            for show in &face.shows {
                if !is_frame_view(&show.widget) {
                    continue;
                }
                let node = format!("{prefix}{}", show.node);
                let view = show.widget.as_live_frame_view();
                match self.bind(&host.instance, &node, &show.port) {
                    Ok((source, binding)) => {
                        show.widget.set_disabled(cx, false);
                        view.set_frame(cx, source, binding);
                    }
                    Err(error) => {
                        view.set_bind_error(cx, error.clone());
                        errors.push(format!("{node}.{} ({})", show.port, error_text(&error)));
                    }
                }
            }
        }
        errors
    }
}

pub fn is_frame_view(widget: &WidgetRef) -> bool {
    widget.borrow::<LiveFrameView>().is_some()
}

pub fn clear_host(cx: &mut Cx, host: &FaceHost) {
    for face in host.faces.values().chain(host.flow_face.iter()) {
        for show in &face.shows {
            if is_frame_view(&show.widget) {
                show.widget.as_live_frame_view().clear_frame(cx);
            }
        }
    }
}

fn error_rank(error: &FrameBindError) -> u8 {
    match error {
        FrameBindError::NoSession => 0,
        FrameBindError::NoSuchNode => 1,
        FrameBindError::StaleBinding => 2,
        FrameBindError::NotAFrameOutput => 3,
    }
}

fn error_text(error: &FrameBindError) -> &'static str {
    match error {
        FrameBindError::NoSession => "frame source is preparing or stopped",
        FrameBindError::NoSuchNode => "no such frame node",
        FrameBindError::NotAFrameOutput => "not a frame output",
        FrameBindError::StaleBinding => "frame binding belongs to a retired instance",
    }
}
