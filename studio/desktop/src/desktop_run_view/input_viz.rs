use super::*;

impl DesktopRunView {
    pub(crate) fn show_input_viz(
        &mut self,
        cx: &mut Cx,
        kind: RunViewInputVizKind,
        x: Option<f64>,
        y: Option<f64>,
    ) {
        let has_target_size = self.last_rect.size.x > 0.0 && self.last_rect.size.y > 0.0;
        let event = match kind {
            RunViewInputVizKind::ClickDown | RunViewInputVizKind::ClickUp => {
                self.awaiting_focus_rect = true;
                self.input_focus_rect = None;
                let local_pos = match (x, y) {
                    (Some(x), Some(y)) => dvec2(x, y),
                    _ if has_target_size => {
                        dvec2(self.last_rect.size.x * 0.5, self.last_rect.size.y * 0.5)
                    }
                    _ => self.ai_viz_pos,
                };
                let local_pos = dvec2(
                    local_pos.x.clamp(0.0, self.last_rect.size.x.max(0.0)),
                    local_pos.y.clamp(0.0, self.last_rect.size.y.max(0.0)),
                );
                InputVizEvent {
                    kind,
                    pos: local_pos,
                    size: None,
                }
            }
            RunViewInputVizKind::TypeText | RunViewInputVizKind::Return => {
                if self.awaiting_focus_rect {
                    self.pending_focus_viz_queue.push_back(kind);
                    return;
                }
                let Some(focus_rect) = self.input_focus_rect else {
                    return;
                };
                InputVizEvent {
                    kind,
                    pos: focus_rect.pos,
                    size: Some(focus_rect.size),
                }
            }
        };
        self.enqueue_or_start_input_viz(event);
        self.redraw(cx);
    }

    pub(crate) fn start_input_viz(&mut self, event: InputVizEvent) {
        let total_frames = match event.kind {
            // Old studio model: quick down pulse, then longer up ripple.
            RunViewInputVizKind::ClickDown => 4,
            RunViewInputVizKind::ClickUp => 30,
            RunViewInputVizKind::TypeText => 16,
            RunViewInputVizKind::Return => 20,
        };
        self.ai_viz_kind = Some(event.kind);
        self.ai_viz_pos = event.pos;
        self.ai_viz_size = event.size;
        self.ai_viz_frames_left = total_frames;
        self.ai_viz_total_frames = total_frames;
    }

    pub(crate) fn enqueue_or_start_input_viz(&mut self, event: InputVizEvent) {
        if self.ai_viz_kind.is_some() {
            self.ai_viz_queue.push_back(event);
        } else {
            self.start_input_viz(event);
        }
    }

    pub(crate) fn set_input_focus_rect(
        &mut self,
        cx: &mut Cx,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
    ) {
        self.input_focus_rect = match (x, y, width, height) {
            (Some(x), Some(y), Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                Some(Rect {
                    pos: dvec2(x, y),
                    size: dvec2(width, height),
                })
            }
            _ => None,
        };
        self.awaiting_focus_rect = false;
        if let Some(focus_rect) = self.input_focus_rect {
            while let Some(kind) = self.pending_focus_viz_queue.pop_front() {
                self.enqueue_or_start_input_viz(InputVizEvent {
                    kind,
                    pos: focus_rect.pos,
                    size: Some(focus_rect.size),
                });
            }
        } else {
            self.pending_focus_viz_queue.clear();
        }
        self.redraw(cx);
    }
}
