use std::collections::HashMap;

use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, widget_match_event::WidgetMatchEvent,
    WindowAction,
};

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;

    pub AdaptiveViewBase = {{AdaptiveView}} {}
    pub AdaptiveView = <AdaptiveViewBase> {
        width: Fill, height: Fill
    
        Mobile = <View> {}
        Desktop = <View> {}
    }
}

/// A widget that adapts its content based on the current context.
///
/// `AdaptiveView` allows you to define different layouts for various conditions, like display context,
/// parent size or platform variations, (e.g., desktop vs. mobile) and automatically switches
/// between them based on a selector function.
///
/// Optionally retains unused variants to preserve their state
///
/// # Example
///
/// ```rust

/// live_design! {
///     // ...
///     adaptive = <AdaptiveView> {
///         Desktop = <CustomView> {
///             label =  { text: "Desktop View" } // override specific values of the same widget
///         }
///         Mobile = <CustomView> {
///             label =  { text: "Mobile View" }
///         }
///     }
///  // ...
/// }
///
/// fn setup_adaptive_view(cx: &mut Cx) {;
///     self.adaptive_view(id!(adaptive)).set_variant_selector(|cx, parent_size| {
///         if cx.display_context.screen_size.x >= 1280.0 {
///             live_id!(Desktop)
///         } else {
///             live_id!(Mobile)
///         }
///     });
/// }
/// ```
///
/// In this example, the `AdaptiveView` switches between Desktop and Mobile layouts
/// based on the screen width. The `set_variant_selector` method allows you to define
/// custom logic for choosing the appropriate layout variant.
///
/// `AdaptiveView` implements a default variant selector based on the screen width for different
/// device layouts (Currently `Desktop` and `Mobile`). You can override this through the `set_variant_selector` method.
///
/// Check out [VariantSelector] for more information on how to define custom selectors, and what information is available to them.
#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct AdaptiveView {
    #[rust]
    area: Area,

    /// This widget's walk, it should always match the walk of the active widget.
    #[walk]
    walk: Walk,

    /// Wether to retain the widget variant state when it goes unused.
    /// While it avoids creating new widgets and keeps their state, be mindful of the memory usage and potential memory leaks.
    #[live]
    retain_unused_variants: bool,

    /// A map of previously active widgets that are not currently being displayed.
    /// Only used when `retain_unused_variants` is true.
    #[rust]
    previously_active_widgets: HashMap<LiveId, WidgetVariant>,

    /// A map of templates that are used to create the active widget.
    #[rust]
    templates: ComponentMap<LiveId, LivePtr>,

    /// The active widget that is currently being displayed.
    #[rust]
    active_widget: Option<WidgetVariant>,

    /// The current variant selector that determines which template to use.
    #[rust]
    variant_selector: Option<Box<VariantSelector>>,

    /// A flag to reapply the selector on the next draw call.
    #[rust]
    should_reapply_selector: bool,

    /// Whether the AdaptiveView has non-default templates.
    /// Used to determine if we should create a default widget.
    /// When there are no custom templates, the user of this AdaptiveView is likely not
    /// setting up a custom selector, so we should create a default widget.
    #[rust]
    has_custom_templates: bool,
}

pub struct WidgetVariant {
    pub template_id: LiveId,
    pub widget_ref: WidgetRef,
}

impl WidgetNode for AdaptiveView {
    fn walk(&mut self, cx: &mut Cx) -> Walk {
        if let Some(active_widget) = self.active_widget.as_ref() {
            active_widget.widget_ref.walk(cx)
        } else {
            // No active widget found, returning a default walk.
            self.walk
        }
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        if let Some(active_widget) = self.active_widget.as_ref() {
            active_widget.widget_ref.find_widgets(path, cached, results);
        }
    }

    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        if let Some(active_widget) = self.active_widget.as_ref() {
            active_widget.widget_ref.uid_to_widget(uid)
        } else {
            WidgetRef::empty()
        }
    }
}

impl LiveHook for AdaptiveView {
    fn before_apply(
        &mut self,
        _cx: &mut Cx,
        apply: &mut Apply,
        _index: usize,
        _nodes: &[LiveNode],
    ) {
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            self.templates.clear();
        }
    }

    fn after_apply_from(&mut self, cx: &mut Cx, apply: &mut Apply) {
        // Do not override the current selector if we are updating from the doc
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            return;
        };

        // If there are no custom templates, create a default widget with the default variant Desktop
        // This is needed so that methods that run before drawing (find_widgets, walk) have something to work with
        if !self.has_custom_templates {
            let template = self.templates.get(&live_id!(Desktop)).unwrap();
            let widget_ref = WidgetRef::new_from_ptr(cx, Some(*template));
            self.active_widget = Some(WidgetVariant {
                template_id: live_id!(Desktop),
                widget_ref: widget_ref.clone(),
            });
        }
        self.set_default_variant_selector();
    }

    fn apply_value_instance(
        &mut self,
        cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize {
        if nodes[index].is_instance_prop() {
            if let Some(live_ptr) = apply.from.to_live_ptr(cx, index) {
                let id = nodes[index].id;
                self.templates.insert(id, live_ptr);

                if id != live_id!(Desktop) && id != live_id!(Mobile) {
                    self.has_custom_templates = true;
                }

                if let Some(widget_variant) = self.active_widget.as_mut() {
                    if widget_variant.template_id == id {
                        widget_variant.widget_ref.apply(cx, apply, index, nodes);
                    }
                }
            }
        } else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
}

impl Widget for AdaptiveView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.widget_match_event(cx, event, scope);
        if let Some(active_widget) = self.active_widget.as_mut() {
            active_widget.widget_ref.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.should_reapply_selector {
            let parent_size = cx.peek_walk_turtle(walk).size;
            self.apply_selector(cx, &parent_size);
        }

        if let Some(active_widget) = self.active_widget.as_mut() {
            active_widget.widget_ref.draw_walk(cx, scope, walk)?;
        }

        DrawStep::done()
    }
}

impl WidgetMatchEvent for AdaptiveView {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            // Handle window geom change events, this is triggered at startup and on window resize.
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                let event_id = cx.event_id();

                // Skip if the display context was already updated on this event
                if cx.display_context.updated_on_event_id == event_id { return }
                // Update the current context if the screen size has changed
                if cx.display_context.screen_size != ce.new_geom.inner_size {
                    cx.display_context.updated_on_event_id = event_id;
                    cx.display_context.screen_size = ce.new_geom.inner_size;

                    self.should_reapply_selector = true;
                }

                cx.redraw_all();
            }
        }
    }
}

impl AdaptiveView {
    /// Apply the variant selector to determine which template to use.
    fn apply_selector(&mut self, cx: &mut Cx, parent_size: &DVec2) {
        let Some(variant_selector) = self.variant_selector.as_mut() else {
            return;
        };

        let template_id = variant_selector(cx, parent_size);

        // If the selector resulted in a widget that is already active, do nothing
        if let Some(active_widget) = self.active_widget.as_mut() {
            if active_widget.template_id == template_id {
                return;
            }
        }

        // If the selector resulted in a widget that was previously active, restore it
        if self.retain_unused_variants && self.previously_active_widgets.contains_key(&template_id)
        {
            let widget_variant = self.previously_active_widgets.remove(&template_id).unwrap();

            self.walk = widget_variant.widget_ref.walk(cx);
            self.active_widget = Some(widget_variant);
            return;
        }

        // Invalidate widget query caches when changing the active variant.
        // Parent views need to rebuild their widget queries since the widget
        // hierarchy has changed. We use the event system to ensure all views
        // process this invalidation in the next event cycle.
        cx.widget_query_invalidation_event = Some(cx.event_id());

        // Otherwise create a new widget from the template
        let template = self.templates.get(&template_id).unwrap();
        let widget_ref = WidgetRef::new_from_ptr(cx, Some(*template));

        // Update this widget's walk to match the walk of the active widget,
        // this ensures that the new widget is not affected by `Fill` or `Fit` constraints from this parent.
        self.walk = widget_ref.walk(cx);

        if let Some(active_widget) = self.active_widget.take() {
            if self.retain_unused_variants {
                self.previously_active_widgets
                    .insert(active_widget.template_id, active_widget);
            }
        }

        self.active_widget = Some(WidgetVariant {
            template_id,
            widget_ref,
        });
    }

    /// Set a variant selector for this widget.
    /// The selector is a closure that takes a `DisplayContext` and returns a `LiveId`, corresponding to the template to use.
    pub fn set_variant_selector(
        &mut self,
        selector: impl FnMut(&mut Cx, &DVec2) -> LiveId + 'static,
    ) {
        self.variant_selector = Some(Box::new(selector));
        self.should_reapply_selector = true;
    }

    pub fn set_default_variant_selector(&mut self) {
        // TODO(Julian): setup a more comprehensive default
        self.set_variant_selector(|cx, _parent_size| {
            if cx.display_context.is_desktop() {
                live_id!(Desktop)
            } else {
                live_id!(Mobile)
            }
        });
    }
}

impl AdaptiveViewRef {
    /// Set a variant selector for this widget.
    /// The selector is a closure that takes a `DisplayContext` and returns a `LiveId`, corresponding to the template to use.
    pub fn set_variant_selector(
        &self,
        selector: impl FnMut(&mut Cx, &DVec2) -> LiveId + 'static,
    ) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.set_variant_selector(selector);
    }
}

/// A closure that returns a `LiveId` corresponding to the template to use.
pub type VariantSelector = dyn FnMut(&mut Cx, &ParentSize) -> LiveId;

/// The size of the parent obtained from running `cx.peek_walk_turtle(walk)` before the widget is drawn.
type ParentSize = DVec2;
