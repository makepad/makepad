//! Main-thread texture bindings shared by every live frame provider.
//! A binding names a declared output, never a numeric probe or a GPU pointer
//! in the artifact store. Providers own validation, demand and retirement.
//!
//! [`script_mod`] registers the frame view, then [`FrameStage`]. The host
//! calls [`FrameProducer::tick`] and draws that stage on the main thread.
use makepad_flow::document::{
    types::{Carrier, Rate, TypeKey, TypeSchema},
    Registry,
};
use makepad_widgets::*;
use std::{any::Any, rc::Rc};

mod stage;
mod view;
pub use stage::{
    FrameProducer, FrameStage, FrameStageRef, FrameStageWidgetRefExt, ProducerTick,
};
pub use view::{refresh_shown_frames, LiveFrameView, LiveFrameViewRef, LiveFrameViewWidgetRefExt};

/// Register the shared frame view, then [`FrameStage`].
/// Call once on a VM, after the widgets prelude.
pub fn script_mod(vm: &mut ScriptVm) {
    view::script_mod(vm);
    stage::script_mod(vm);
}

pub const FRAME: &str = "stage.frame";

pub fn register_frame_type(registry: &mut Registry) -> Result<(), String> {
    registry
        .register_type(TypeSchema {
            key: TypeKey::new(FRAME),
            schema_rev: 1,
            carrier: Carrier::Frame,
            rate: Rate::Frame,
            local_only: true,
            serial: None,
            audio_lowering: None,
            doc: "A main-thread rendered texture, accessed through its owning frame provider."
                .into(),
        })
        .map_err(|e| e.to_string())
}

/// Private per-provider identity. Clones name that exact provider instance.
/// Rc identity cannot be reused while an old binding holds the token.
#[derive(Clone, Default)]
pub struct FrameProviderId(Rc<()>);

#[derive(Clone)]
pub struct FrameBinding {
    provider: FrameProviderId,
    data: Rc<dyn Any>,
}

impl FrameBinding {
    /// Call only after validating the graph output. Payload should include
    /// session incarnation and qualified output identity; never a raw pointer.
    pub fn new<T: Any>(provider: &FrameProviderId, data: T) -> Self {
        Self {
            provider: provider.clone(),
            data: Rc::new(data),
        }
    }
    /// Provider identity is checked before exposing typed private data.
    pub fn data<'a, T: Any>(&'a self, provider: &FrameProviderId) -> Option<&'a T> {
        if !Rc::ptr_eq(&self.provider.0, &provider.0) {
            return None;
        }
        self.data.downcast_ref()
    }
}

/// A provider-defined lease. Its final drop is on the host thread.
pub struct FrameDemand {
    _lease: Box<dyn Any>,
}
impl FrameDemand {
    pub fn new<T: Any>(lease: T) -> Self {
        Self {
            _lease: Box::new(lease),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameBindError {
    NoSession,
    NoSuchNode,
    NotAFrameOutput,
    StaleBinding,
}

pub enum FrameRead {
    /// A caller-owned clone. Release it when the binding changes/expires;
    /// provider target budgets cannot account for arbitrary retained clones.
    Ready(Texture),
    Pending,
    Invalid(FrameBindError),
}

pub trait FrameSource {
    fn bind(&self, instance: &str, node: &str, port: &str) -> Result<FrameBinding, FrameBindError>;
    /// Refresh visible-view demand and fetch the current texture each draw.
    fn read(&self, binding: &FrameBinding) -> FrameRead;
    /// Observe the current output's publication stamp without creating demand
    /// or retaining its texture. A changed stamp means a new frame was
    /// uploaded or submitted for rendering, not that its pixels differ.
    /// `None` means unavailable or unsupported; cached frames keep their stamp.
    /// Compare stamps only within the same binding and installed generation.
    fn frame_stamp(&self, _binding: &FrameBinding) -> Option<u64> {
        None
    }
    fn hold(&self, binding: &FrameBinding) -> Result<FrameDemand, FrameBindError>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FrameViewStatus {
    #[default]
    Unbound,
    Pending,
    Ready,
    Invalid(FrameBindError),
}
