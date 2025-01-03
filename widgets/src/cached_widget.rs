use std::collections::HashMap;

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

live_design! {
    link widgets;
    
    pub CachedWidget = {{CachedWidget}} {}
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
/// ```
/// <CachedWidget> {
///     my_widget = <MyWidget> {}
/// }
/// ```
///
/// The child widget will be created once and cached.
/// Subsequent uses of this `CachedWidget` with the same child id (`mid_widget`) will reuse the cached instance.
/// Note that only one child is supported per `CachedWidget`.
/// 
/// CachedWidget supports Makepad's widget finding mechanism, allowing child widgets to be located as expected.
///
/// # Implementation Details
///
/// - Uses a global `WidgetWrapperCache` to store cached widgets
/// - Handles widget creation and caching in the `after_apply` hook
/// - Delegates most widget operations (like event handling and drawing) to the cached child widget
///
/// # Note
///
/// While `CachedWidget` can significantly improve performance for complex, frequently used widgets,
/// it should be used judiciously. Overuse of caching can lead to unexpected behavior if not managed properly.
#[derive(Live, LiveRegisterWidget, WidgetRef)]
pub struct CachedWidget {
    #[walk]
    walk: Walk,

    /// The ID of the child widget template
    #[rust]
    template_id: LiveId,

    /// The cached child widget template
    #[rust]
    template: Option<LivePtr>,

    /// The cached child widget instance
    #[rust]
    widget: Option<WidgetRef>,
}

impl LiveHook for CachedWidget {
    fn before_apply(
        &mut self,
        _cx: &mut Cx,
        apply: &mut Apply,
        _index: usize,
        _nodes: &[LiveNode],
    ) {
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            self.template = None;
        }
    }

    /// Handles the application of instance properties to this CachedWidget.
    ///
    /// In the case of `CachedWidget` This method is responsible for setting up the template 
    /// for the child widget, and applying any changes to an existing widget instance.
    fn apply_value_instance(
        &mut self,
        cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize {
        if nodes[index].is_instance_prop() {
            if let Some(live_ptr) = apply.from.to_live_ptr(cx, index) {
                if self.template.is_some() {
                    nodes.skip_node(index);
                    error!("CachedWidget only supports one child widget, skipping additional instances");
                }
                let id = nodes[index].id;
                self.template_id = id;
                self.template = Some(live_ptr);

                if let Some(widget) = &mut self.widget {
                    widget.apply(cx, apply, index, nodes);
                }
            }
        } else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }

    /// Handles the creation or retrieval of the cached widget after applying changes.
    ///
    /// This method is called after all properties have been applied to the widget.
    /// It ensures that the child widget is properly cached and retrieved from the global cache.
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        // Ensure the global widget cache exists
        if !cx.has_global::<WidgetWrapperCache>() {
            cx.set_global(WidgetWrapperCache::default())
        }

        if self.widget.is_none() {
            // Try to retrieve the widget from the global cache
            if let Some(widget) = cx
                .get_global::<WidgetWrapperCache>()
                .map
                .get_mut(&self.template_id)
            {
                self.widget = Some(widget.clone());
            } else {
                // If not in cache, create a new widget and add it to the cache
                let widget = WidgetRef::new_from_ptr(cx, self.template);
                cx.get_global::<WidgetWrapperCache>()
                    .map
                    .insert(self.template_id, widget.clone());
                self.widget = Some(widget);
            }
        }
    }
}

impl WidgetNode for CachedWidget {
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
            Area::default()
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        if let Some(widget) = &self.widget {
            widget.redraw(cx);
        }
    }

    // Searches for widgets within this CachedWidget based on the given path.
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        let Some(widget) = self.widget.as_ref() else { return };
        if self.template_id == path[0] {
            if path.len() == 1 {
                // If the child widget is the target widget, add it to the results
                results.push(widget.clone());
            } else {
                // If not, continue searching in the child widget
                widget.find_widgets(&path[1..], cached, results);
            }
        }
    }

    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        if let Some(widget) = &self.widget {
            return widget.uid_to_widget(uid);
        }
        WidgetRef::empty()
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
            return widget.draw_walk(cx, scope, walk);
        }

        DrawStep::done()
    }
}

impl CachedWidget {}

#[derive(Default)]
pub struct WidgetWrapperCache {
    map: HashMap<LiveId, WidgetRef>,
}
