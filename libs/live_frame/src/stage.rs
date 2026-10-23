//! Host-driven frame producers and the widget that renders them.
//! The host calls [`FrameProducer::tick`] and passes the producers to
//! [`FrameStage`]. Drawing runs on the main thread, inside the window's
//! parent pass. The host chooses which producers exist and when to tick.

use crate::FrameSource;
use makepad_widgets::*;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FrameStage = set_type_default() do #(FrameStage::register_widget(vm)) {
        width: 1 height: 1
    }
}

/// What one tick of a frame producer asks of the host.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProducerTick {
    /// Something shown is animating: ask for another frame.
    pub wants_frame: bool,
    /// A generation is being installed or resources are being retired:
    /// keep ticking even if nothing shows.
    pub busy: bool,
}

/// A registered producer of frame pictures: a main-thread renderer the host
/// draws inside the window's parent pass and previews bind to.
pub trait FrameProducer {
    /// Once per UI tick, outside drawing: installation, retirement, clocks.
    fn tick(&self, cx: &mut Cx, now: f64) -> ProducerTick;
    /// Encode this producer's passes. False when nothing could be encoded
    /// because no pass is being drawn.
    fn render(&self, cx: &mut Cx2d) -> bool;
    /// The bindable frame outputs, for the views that show them.
    fn source(&self) -> Rc<dyn FrameSource>;
}

/// One render entry point, placed before all frame views in the window's
/// parent pass. The host schedules redraws from each producer's tick result;
/// this widget does not tick owners, advance simulation, or start workers.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FrameStage {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,
    #[rust]
    producers: Vec<Rc<dyn FrameProducer>>,
    #[rust]
    render_refused: bool,
}

impl WidgetNode for FrameStage {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for FrameStage {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = cx.walk_turtle_with_area(&mut self.area, walk);
        // Every producer encodes child passes. Each restores the parent's
        // pass before the following producer or preview is drawn.
        let mut refused = false;
        for producer in &self.producers {
            refused |= !producer.render(cx);
        }
        if refused && !self.render_refused {
            log!("FrameStage must draw inside the window's parent pass");
        }
        self.render_refused = refused;
        DrawStep::done()
    }
}

impl FrameStage {
    pub fn set_producers(&mut self, cx: &mut Cx, producers: Vec<Rc<dyn FrameProducer>>) {
        self.producers = producers;
        self.render_refused = false;
        if self.area.is_empty() {
            cx.redraw_all();
        } else {
            self.area.redraw(cx);
        }
    }
}

impl FrameStageRef {
    pub fn set_producers(&self, cx: &mut Cx, producers: Vec<Rc<dyn FrameProducer>>) {
        if let Some(mut stage) = self.borrow_mut() {
            stage.set_producers(cx, producers);
        }
    }
}
