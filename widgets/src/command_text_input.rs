use crate::*;
use unicode_segmentation::UnicodeSegmentation;

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;

    List = {{List}} {
        flow: Down,
        width: Fill,
        height: Fill,
    }

    pub CommandTextInput = {{CommandTextInput}} {
        flow: Down,
        height: Fit,

        keyboard_focus_color: (THEME_COLOR_CTRL_HOVER),
        pointer_hover_color: (THEME_COLOR_CTRL_HOVER * 0.85),

        popup = <RoundedView> {
            flow: Down,
            height: Fit,
            visible: false,
            search_input = <TextInput> {
                width: Fill,
                height: Fit,
            }

            list = <List> {
                height: Fit
            }
        }

        persistent = <RoundedView> {
            flow: Down,
            height: Fit,
            top = <View> { height: Fit }
            center = <RoundedView> {
                height: Fit,
                // `left` and `right` seems to not work with `height: Fill`.
                left = <View> { width: Fit, height: Fit }
                text_input = <TextInput> { width: Fill }
                right = <View> { width: Fit, height: Fit }
            }
            bottom = <View> { height: Fit }
        }
    }
}

#[derive(Debug, Copy, Clone, DefaultNone)]
enum InternalAction {
    ShouldBuildItems,
    ItemSelected,
    None,
}

/// `TextInput` wrapper glued to a popup list of options that is shown when a
/// trigger character is typed.
///
/// Limitation: Selectable items are expected to be `View`s.
#[derive(Widget, Live)]
pub struct CommandTextInput {
    #[deref]
    deref: View,

    /// The character that triggers the popup.
    ///
    /// If not set, popup can't be triggerd by keyboard.
    ///
    /// `char` type is not "live", so String is used instead.
    /// Behavior is undefined if this string contains anything other than a
    /// single character.
    #[live]
    pub trigger: Option<String>,

    /// Strong color to highlight the item that would be submitted if `Return` is pressed.
    #[live]
    pub keyboard_focus_color: Vec4,

    /// Weak color to highlight the item that the pointer is hovering over.
    #[live]
    pub pointer_hover_color: Vec4,

    /// Just used to detect `trigger`` insertion.
    #[rust]
    previous: String,

    /// To deal with focus requesting issues.
    #[rust]
    is_search_input_focus_pending: bool,

    /// To deal with focus requesting issues.
    #[rust]
    is_text_input_focus_pending: bool,

    /// Index from `selectable_widgets` that would be submitted if `Return` is pressed.
    /// `None` if there are no selectable widgets.
    #[rust]
    keyboard_focus_index: Option<usize>,

    /// Index from `selectable_widgets` that the pointer is hovering over.
    /// `None` if there are no selectable widgets.
    #[rust]
    pointer_hover_index: Option<usize>,

    /// Convenience copy of the selectable widgets on the popup list.
    #[rust]
    selectable_widgets: Vec<WidgetRef>,

    /// To deal with widgets not being `Send`.
    #[rust]
    last_selected_widget: WidgetRef,
}

impl Widget for CommandTextInput {
    fn set_text(&mut self, v: &str) {
        self.text_input_ref().set_text(v);
    }

    fn text(&self) -> String {
        self.text_input_ref().text()
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.update_highlights(cx);

        while !self.deref.draw_walk(cx, scope, walk).is_done() {}

        if self.is_search_input_focus_pending {
            self.is_search_input_focus_pending = false;
            self.search_input_ref().set_key_focus(cx);
        }

        if self.is_text_input_focus_pending {
            self.is_text_input_focus_pending = false;
            self.text_input_ref().set_key_focus(cx);
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.deref.handle_event(cx, event, scope);

        // since popup is hidden on blur, this is enough
        if self.view(id!(popup)).visible() {
            if let Event::KeyDown(key_event) = event {
                let delta = match key_event.key_code {
                    KeyCode::ArrowDown => 1,
                    KeyCode::ArrowUp => -1,
                    _ => 0,
                };

                if delta != 0 {
                    self.on_keyboard_move(cx, delta);
                }
            }
        }

        if let Event::Actions(actions) = event {
            let mut selected_by_click = None;
            let mut should_redraw = false;

            for (idx, item) in self.selectable_widgets.iter().enumerate() {
                let item = item.as_view();

                if item
                    .finger_down(actions)
                    .map(|fe| fe.tap_count == 1)
                    .unwrap_or(false)
                {
                    selected_by_click = Some((&*item).clone());
                }

                if item.finger_hover_out(actions).is_some() && Some(idx) == self.pointer_hover_index
                {
                    self.pointer_hover_index = None;
                    should_redraw = true;
                }

                if item.finger_hover_in(actions).is_some() {
                    self.pointer_hover_index = Some(idx);
                    should_redraw = true;
                }
            }

            if should_redraw {
                self.redraw(cx);
            }

            if let Some(selected) = selected_by_click {
                self.select_item(cx, scope, selected);
            }

            let text_input = self.text_input_ref();
            let search_input = self.search_input_ref();

            for action in actions.iter().filter_map(|a| a.as_widget_action()) {
                if action.widget_uid == text_input.widget_uid() {
                    match action.cast::<TextInputAction>() {
                        TextInputAction::Change(_) => {
                            self.on_text_input_changed(cx, scope);
                        }
                        _ => {}
                    }
                }

                if action.widget_uid == search_input.widget_uid() {
                    match action.cast::<TextInputAction>() {
                        TextInputAction::Change(search) => {
                            // disallow multiline input
                            search_input.set_text(search.lines().next().unwrap_or_default());

                            cx.widget_action(
                                self.widget_uid(),
                                &scope.path,
                                InternalAction::ShouldBuildItems,
                            );
                        }
                        TextInputAction::Return(_) => {
                            self.on_search_input_submit(cx, scope);
                        }
                        TextInputAction::Escape => {
                            self.is_text_input_focus_pending = true;
                            self.hide_popup();
                            self.redraw(cx);
                        }
                        TextInputAction::KeyFocusLost => {
                            self.hide_popup();
                            self.redraw(cx);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

impl CommandTextInput {
    fn on_text_input_changed(&mut self, cx: &mut Cx, scope: &mut Scope) {
        if self.was_trigger_inserted() {
            self.show_popup(cx);
            self.is_search_input_focus_pending = true;
            cx.widget_action(
                self.widget_uid(),
                &scope.path,
                InternalAction::ShouldBuildItems,
            );
        }

        let current = self.text_input_ref().text();
        self.previous = current;
    }

    fn on_search_input_submit(&mut self, cx: &mut Cx, scope: &mut Scope) {
        let Some(idx) = self.keyboard_focus_index else {
            return;
        };

        self.select_item(cx, scope, self.selectable_widgets[idx].clone());
    }

    fn select_item(&mut self, cx: &mut Cx, scope: &mut Scope, selected: WidgetRef) {
        self.last_selected_widget = selected;
        cx.widget_action(self.widget_uid(), &scope.path, InternalAction::ItemSelected);
        self.hide_popup();
        self.is_text_input_focus_pending = true;
        self.try_remove_trigger_grapheme();
        self.redraw(cx);
    }

    fn try_remove_trigger_grapheme(&mut self) {
        let head = get_head(&self.text_input_ref());

        if head == 0 {
            return;
        }

        let text = self.text();
        let Some((inserted_grapheme_pos, inserted_grapheme)) =
            inserted_grapheme_with_pos(&text, head)
        else {
            return;
        };

        if Some(inserted_grapheme) == self.trigger_grapheme() {
            let at_removed = graphemes_with_pos(&text)
                .filter_map(|(p, g)| {
                    if p == inserted_grapheme_pos {
                        None
                    } else {
                        Some(g)
                    }
                })
                .collect::<String>();

            self.set_text(&at_removed);
            self.previous = at_removed;
        }
    }

    fn show_popup(&mut self, cx: &mut Cx) {
        self.view(id!(popup)).set_visible(true);
        self.view(id!(popup)).redraw(cx);
    }

    fn hide_popup(&mut self) {
        self.clear_popup();
        self.view(id!(popup)).set_visible(false);
    }

    /// Clear all text and hide the popup going back to initial state.
    pub fn reset(&mut self) {
        self.clear_popup();
        self.hide_popup();
        self.text_input_ref().set_text("");
        self.previous = String::new();
    }

    fn clear_popup(&mut self) {
        self.search_input_ref().set_text("");
        self.search_input_ref().set_cursor(0, 0);
        self.clear_items();
    }

    /// Empty the list of items.
    ///
    /// Normally called as response to `should_build_items`.
    pub fn clear_items(&mut self) {
        self.list(id!(list)).clear();
        self.selectable_widgets.clear();
        self.keyboard_focus_index = None;
        self.pointer_hover_index = None;
    }

    /// Add a custom selectable item to the list.
    ///
    /// Normally called after clearing the previous items.
    pub fn add_item(&mut self, widget: WidgetRef) {
        self.list(id!(list)).add(widget.clone());
        self.selectable_widgets.push(widget);
        self.keyboard_focus_index = self.keyboard_focus_index.or(Some(0));
    }

    /// Add a custom unselectable item to the list.
    ///
    /// Ex: Headers, dividers, etc.
    ///
    /// Normally called after clearing the previous items.
    pub fn add_unselectable_item(&mut self, widget: WidgetRef) {
        self.list(id!(list)).add(widget);
    }

    /// Get the current search query.
    ///
    /// You probably want this for filtering purposes when updating the items.
    pub fn search_text(&self) -> String {
        self.search_input_ref().text()
    }

    /// Check if an item has been selected returning the widget reference.
    pub fn item_selected(&self, actions: &Actions) -> Option<WidgetRef> {
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .filter(|a| a.widget_uid == self.widget_uid())
            .find_map(|a| {
                if let InternalAction::ItemSelected = a.cast() {
                    Some(self.last_selected_widget.clone())
                } else {
                    None
                }
            })
    }

    /// If you should compute the items to display again.
    ///
    /// For ex: If trigger is typed, or if search filter changes.
    pub fn should_build_items(&self, actions: &Actions) -> bool {
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .filter(|a| a.widget_uid == self.widget_uid())
            .any(|a| matches!(a.cast(), InternalAction::ShouldBuildItems))
    }

    /// Exposed the internal main `TextInput` widget.
    // `_ref` is added to avoid naming conflicts with Makepad's autogenerated methods.
    pub fn text_input_ref(&self) -> TextInputRef {
        self.text_input(id!(text_input))
    }

    /// Exposes the internal search `TextInput` widget.
    // `_ref` is added for consistency with `text_input_ref`.
    pub fn search_input_ref(&self) -> TextInputRef {
        self.text_input(id!(search_input))
    }

    fn was_trigger_inserted(&self) -> bool {
        let text_input = self.text_input_ref();
        let prev = self.previous.as_str();
        let current = &self.text();

        let prev_graphemes_count = graphemes(prev).count();
        let current_graphemes_count = graphemes(current).count();

        if current_graphemes_count != prev_graphemes_count + 1 {
            return false;
        }

        // not necessarily the cursor head, but works for this single character use case
        let head = get_head(&text_input);

        if head == 0 {
            return false;
        }

        let Some(inserted_grapheme) = inserted_grapheme(current, head) else {
            return false;
        };

        let Some(trigger) = self.trigger_grapheme() else {
            return false;
        };

        inserted_grapheme == trigger
    }

    fn trigger_grapheme(&self) -> Option<&str> {
        self.trigger.as_ref().and_then(|t| graphemes(t).next())
    }

    fn on_keyboard_move(&mut self, cx: &mut Cx, delta: i32) {
        let Some(idx) = self.keyboard_focus_index else {
            return;
        };

        let new_index = idx
            .saturating_add_signed(delta as isize)
            .clamp(0, self.selectable_widgets.len() - 1);

        if idx != new_index {
            self.keyboard_focus_index = Some(new_index);
        }

        self.redraw(cx);
    }

    fn update_highlights(&mut self, cx: &mut Cx) {
        for (idx, item) in self.selectable_widgets.iter().enumerate() {
            item.apply_over(cx, live! { show_bg: true, cursor: Hand });

            if Some(idx) == self.keyboard_focus_index {
                item.apply_over(
                    cx,
                    live! {
                        draw_bg: {
                            color: (self.keyboard_focus_color),
                        }
                    },
                );
            } else if Some(idx) == self.pointer_hover_index {
                item.apply_over(
                    cx,
                    live! {
                        draw_bg: {
                            color: (self.pointer_hover_color),
                        }
                    },
                );
            } else {
                item.apply_over(
                    cx,
                    live! {
                        draw_bg: {
                            color: (Vec4::all(0.)),
                        }
                    },
                );
            }
        }
    }

    /// Obtain focus in the main `TextInput` widget as soon as possible.
    pub fn request_text_input_focus(&mut self) {
        self.is_text_input_focus_pending = true;
    }
}

impl LiveHook for CommandTextInput {}

impl CommandTextInputRef {
    /// Calls `should_build_items` on the inner widget. See docs there for more info.
    pub fn should_build_items(&self, actions: &Actions) -> bool {
        self.borrow()
            .map_or(false, |inner| inner.should_build_items(actions))
    }

    /// Calls `clear_items` on the inner widget. See docs there for more info.
    pub fn clear_items(&mut self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_items();
        }
    }

    /// Calls `add_item` on the inner widget. See docs there for more info.
    pub fn add_item(&mut self, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add_item(widget);
        }
    }

    /// Calls `add_unselectable_item` on the inner widget. See docs there for more info.
    pub fn add_unselectable_item(&mut self, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add_unselectable_item(widget);
        }
    }

    /// Calls `item_selected` on the inner widget. See docs there for more info.
    pub fn item_selected(&self, actions: &Actions) -> Option<WidgetRef> {
        self.borrow().and_then(|inner| inner.item_selected(actions))
    }

    /// Calls `text_input_ref` on the inner widget. See docs there for more info.
    pub fn text_input_ref(&self) -> TextInputRef {
        self.borrow()
            .map_or(WidgetRef::empty().as_text_input(), |inner| {
                inner.text_input_ref()
            })
    }

    /// Calls `search_input_ref` on the inner widget. See docs there for more info.
    pub fn search_input_ref(&self) -> TextInputRef {
        self.borrow()
            .map_or(WidgetRef::empty().as_text_input(), |inner| {
                inner.search_input_ref()
            })
    }

    /// Calls `reset` on the inner widget. See docs there for more info.
    pub fn reset(&mut self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset();
        }
    }

    /// Calls `request_text_input_focus` on the inner widget. See docs there for more info.
    pub fn request_text_input_focus(&mut self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.request_text_input_focus();
        }
    }

    /// Calls `search_text` on the inner widget. See docs there for more info.
    pub fn search_text(&self) -> String {
        self.borrow()
            .map_or(String::new(), |inner| inner.search_text())
    }
}

fn graphemes(text: &str) -> impl DoubleEndedIterator<Item = &str> {
    text.graphemes(true)
}

fn graphemes_with_pos(text: &str) -> impl DoubleEndedIterator<Item = (usize, &str)> {
    text.grapheme_indices(true)
}

fn inserted_grapheme_with_pos(text: &str, cursor_pos: usize) -> Option<(usize, &str)> {
    // TODO: Should be < ?
    graphemes_with_pos(text).rfind(|(i, _)| *i <= cursor_pos)
}

fn inserted_grapheme(text: &str, cursor_pos: usize) -> Option<&str> {
    inserted_grapheme_with_pos(text, cursor_pos).map(|(_, g)| g)
}

fn get_head(text_input: &TextInputRef) -> usize {
    text_input.borrow().map_or(0, |p| p.get_cursor().head.index)
}

/// Reduced and adapted copy of the `List` widget from Moly.
#[derive(Live, Widget, LiveHook)]
struct List {
    #[walk]
    walk: Walk,

    #[layout]
    layout: Layout,

    #[redraw]
    #[rust]
    area: Area,

    #[rust]
    items: Vec<WidgetRef>,
}

impl Widget for List {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.items.iter().for_each(|item| {
            item.handle_event(cx, event, scope);
        });
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        self.items.iter().for_each(|item| {
            item.draw_all(cx, scope);
        });
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl List {
    fn clear(&mut self) {
        self.items.clear();
    }

    fn add(&mut self, widget: WidgetRef) {
        self.items.push(widget);
    }
}

impl ListRef {
    fn clear(&mut self) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };

        inner.clear();
    }

    fn add(&mut self, widget: WidgetRef) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };

        inner.add(widget);
    }
}
