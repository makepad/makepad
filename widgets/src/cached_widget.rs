use std::collections::HashMap;

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.CachedWidget = #(CachedWidget::register_widget(vm))
}

/// A Singleton wrapper widget that caches and reuses its child widget across multiple instances.
///
/// `CachedWidget` is designed to optimize performance and memory usage by ensuring
/// that only one instance of a child widget is created and shared across multiple
/// uses in the UI. This is particularly useful for complex widgets that are used
/// in different parts of the UI but should maintain a single state.
///
/// # Usage
///
/// In the DSL, you can use `CachedWidget` as follows:
///
/// ```ignore
/// CachedWidget {
///     my_widget = MyWidget {}
/// }
/// ```
///
/// The child widget will be created once and cached.
/// Subsequent uses of this `CachedWidget` with the same child id (`my_widget`) will reuse the cached instance.
/// Note that only one child is supported per `CachedWidget`.
///
/// CachedWidget supports Makepad's widget finding mechanism, allowing child widgets to be located as expected.
///
/// # Implementation Details
///
/// - Uses a global `WidgetWrapperCache` to store cached widgets
/// - Handles widget creation and caching in the `on_after_apply` hook
/// - Delegates most widget operations (like event handling and drawing) to the cached child widget
///
/// # Note
///
/// While `CachedWidget` can significantly improve performance for complex, frequently used widgets,
/// it should be used judiciously. Overuse of caching can lead to unexpected behavior if not managed properly.
#[derive(Script, WidgetRef, WidgetRegister)]
pub struct CachedWidget {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[rust]
    area: Area,

    /// The ID of the child widget template
    #[rust]
    template_id: LiveId,

    /// The cached child widget template value
    #[rust]
    template_value: Option<ScriptValue>,

    /// The cached child widget instance
    #[rust]
    widget: Option<WidgetRef>,
}

impl ScriptHook for CachedWidget {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.template_value = None;
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle children from the object's vec - get the first prefixed child
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |_vm, vec| {
                for kv in vec {
                    if let Some(id) = kv.key.as_id() {
                        if kv.key.as_id().is_some() {
                            if self.template_value.is_some() {
                                error!("CachedWidget only supports one child widget, skipping additional instances");
                                continue;
                            }
                            self.template_id = id;
                            self.template_value = Some(kv.value);
                        }
                    }
                }
            });
        }

        // If widget already exists, apply updates to it
        if let Some(widget) = &mut self.widget {
            if let Some(template_value) = self.template_value {
                widget.script_apply(vm, apply, scope, template_value);
            }
            let widget = widget.clone();
            vm.cx_mut().widget_tree_insert_child_deep(self.uid, self.template_id, widget);
            return;
        }

        // Ensure the global widget cache exists
        let cx = vm.cx_mut();
        if !cx.has_global::<WidgetWrapperCache>() {
            cx.set_global(WidgetWrapperCache::default())
        }

        // Try to retrieve the widget from the global cache
        if let Some(widget) = cx
            .get_global::<WidgetWrapperCache>()
            .map
            .get_mut(&self.template_id)
        {
            self.widget = Some(widget.clone());
        } else if let Some(template_value) = self.template_value {
            // If not in cache, create a new widget and add it to the cache
            let widget = WidgetRef::script_from_value_scoped(vm, scope, template_value);
            let cx = vm.cx_mut();
            cx.get_global::<WidgetWrapperCache>()
                .map
                .insert(self.template_id, widget.clone());
            self.widget = Some(widget);
        }
        if let Some(widget) = &self.widget {
            vm.cx_mut().widget_tree_insert_child_deep(self.uid, self.template_id, widget.clone());
        }
    }
}

impl WidgetNode for CachedWidget {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, cx: &mut Cx) -> Walk {
        if let Some(widget) = &self.widget {
            widget.walk(cx)
        } else {
            self.walk
        }
    }

    fn area(&self) -> Area {
        if let Some(widget) = &self.widget {
            widget.area()
        } else {
            self.area
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if let Some(widget) = &self.widget {
            visit(self.template_id, widget.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        if let Some(widget) = &self.widget {
            widget.redraw(cx);
        }
    }
}

impl Widget for CachedWidget {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(widget) = &self.widget {
            widget.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(widget) = &self.widget {
            widget.draw_walk(cx, scope, walk)
        } else {
            DrawStep::done()
        }
    }
}

impl CachedWidget {}

#[derive(Default)]
pub struct WidgetWrapperCache {
    map: HashMap<LiveId, WidgetRef>,
}
