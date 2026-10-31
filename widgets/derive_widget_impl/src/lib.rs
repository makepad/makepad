//! The expansion logic of makepad-derive-widget as a plain library (see
//! makepad-script-derive-impl for the pattern).

mod derive_animator;
mod derive_widget;

pub use derive_animator::derive_animator_impl;
pub use derive_widget::{
    camel_case_to_snake_case, derive_widget_impl, derive_widget_ref_impl, derive_widget_register_impl,
    derive_widget_set_impl,
};
