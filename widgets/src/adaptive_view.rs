use std::collections::HashMap;

use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt,
    WidgetMatchEvent, WindowAction,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AdaptiveViewBase = #(AdaptiveView::register_widget(vm))

    mod.widgets.AdaptiveView = set_type_default() do mod.widgets.AdaptiveViewBase{
        width: Fill
        height: Fill

        Mobile := mod.widgets.ViewBase{}
        Desktop := mod.widgets.ViewBase{}
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
/// ```ignore

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
///     self.adaptive_view(ids!(adaptive)).set_variant_selector(|cx, parent_size| {
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
#[derive(Script, WidgetRegister, WidgetRef)]
pub struct AdaptiveView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

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
    templates: ComponentMap<LiveId, ScriptObjectRef>,

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

    /// The most recent size of the parent.
    #[rust]
    parent_size: Vec2d,
}

pub struct WidgetVariant {
    pub template_id: LiveId,
    pub widget_ref: WidgetRef,
}

impl WidgetNode for AdaptiveView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
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

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if let Some(active_widget) = self.active_widget.as_ref() {
            visit(active_widget.template_id, active_widget.widget_ref.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl ScriptHook for AdaptiveView {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle vec_key children from the object's vec
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if kv.key.as_id().is_some() {
                            if let Some(id) = kv.key.as_id() {
                                if let Some(template_obj) = kv.value.as_object() {
                                    self.templates
                                        .insert(id, vm.bx.heap.new_object_ref(template_obj));
                                }

                                if id != id!(Desktop) && id != id!(Mobile) {
                                    self.has_custom_templates = true;
                                }

                                if let Some(widget_variant) = self.active_widget.as_mut() {
                                    if widget_variant.template_id == id {
                                        widget_variant
                                            .widget_ref
                                            .script_apply(vm, apply, scope, kv.value);
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }

        // Do not override the current selector if we are updating from the doc
        if apply.is_reload() {
            vm.cx_mut().widget_tree_mark_dirty(self.uid);
            return;
        };

        // If there are no custom templates, create a default widget with the default variant Desktop
        // This is needed so that methods that run before drawing (find_widgets, walk) have something to work with
        if !self.has_custom_templates {
            let template_ref = self.templates.get(&id!(Desktop)).unwrap();
            let template_value: ScriptValue = template_ref.as_object().into();
            let widget_ref = WidgetRef::script_from_value_scoped(vm, scope, template_value);
            self.active_widget = Some(WidgetVariant {
                template_id: live_id!(Desktop),
                widget_ref,
            });
        }
        self.set_default_variant_selector();
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
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
        let parent_size = cx.peek_walk_turtle(walk).size;
        let parent_size_has_changed = parent_size != self.parent_size;

        if parent_size_has_changed || self.should_reapply_selector {
            self.parent_size = parent_size;
            self.apply_selector(cx);
            self.should_reapply_selector = false;
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
            // Window geometry has changed, reapply the selector.
            // Will use the most recent parent size, might be updated on next draw call.
            if let WindowAction::WindowGeomChange(_ce) = action.as_widget_action().cast() {
                self.apply_selector(cx);
            }
        }
    }
}

impl AdaptiveView {
    /// Apply the variant selector to determine which template to use.
    fn apply_selector(&mut self, cx: &mut Cx) {
        let Some(variant_selector) = self.variant_selector.as_mut() else {
            return;
        };

        let template_id = variant_selector(cx, &self.parent_size);

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
            let widget = widget_variant.widget_ref.clone();
            let tid = widget_variant.template_id;
            self.active_widget = Some(widget_variant);
            cx.widget_tree_insert_child_deep(self.uid, tid, widget);
            return;
        }

        // Invalidate widget query caches when changing the active variant.
        // Parent views need to rebuild their widget queries since the widget
        // hierarchy has changed. We use the event system to ensure all views
        // process this invalidation in the next event cycle.
        cx.widget_query_invalidation_event = Some(cx.event_id());

        // Otherwise create a new widget from the template
        let template_ref = self.templates.get(&template_id).unwrap();
        let template_value: ScriptValue = template_ref.as_object().into();
        let widget_ref = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));

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
            widget_ref: widget_ref.clone(),
        });
        cx.widget_tree_insert_child_deep(self.uid, template_id, widget_ref);
    }

    /// Set a variant selector for this widget.
    /// The selector is a closure that takes a `DisplayContext` and returns a `LiveId`, corresponding to the template to use.
    pub fn set_variant_selector(
        &mut self,
        selector: impl FnMut(&mut Cx, &Vec2d) -> LiveId + 'static,
    ) {
        self.variant_selector = Some(Box::new(selector));
        self.should_reapply_selector = true;
    }

    pub fn set_default_variant_selector(&mut self) {
        // TODO(Julian): setup a more comprehensive default, currently defaults to Desktop even if the screen size is unknown
        // (happens on startup for macOS due to a regression, first few WindowGeomChange events report size 0)
        self.set_variant_selector(|cx, _parent_size| {
            if cx.display_context.is_desktop() || !cx.display_context.is_screen_size_known() {
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
    pub fn set_variant_selector(&self, selector: impl FnMut(&mut Cx, &Vec2d) -> LiveId + 'static) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.set_variant_selector(selector);
    }
}

/// A closure that returns a `LiveId` corresponding to the template to use.
pub type VariantSelector = dyn FnMut(&mut Cx, &ParentSize) -> LiveId;

/// The size of the parent obtained from running `cx.peek_walk_turtle(walk)` before the widget is drawn.
type ParentSize = Vec2d;
