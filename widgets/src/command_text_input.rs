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

            // Wrapper workaround to hide search input when inline search is enabled
            // as we currently can't hide the search input avoiding events.
            search_input_wrapper = <RoundedView> {
                height: Fit,
                search_input = <TextInput> {
                    width: Fill,
                    height: Fit,
                }
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
    /// Behavior is undefined if this string contains anything other than a
    /// single grapheme.
    #[live]
    pub trigger: Option<String>,

    /// Handle search within the main text input instead of using a separate
    /// search input.
    ///
    /// Note: Any kind of whitespace will terminate search.
    #[live]
    pub inline_search: bool,

    /// Strong color to highlight the item that would be submitted if `Return` is pressed.
    #[live]
    pub keyboard_focus_color: Vec4,

    /// Weak color to highlight the item that the pointer is hovering over.
    #[live]
    pub pointer_hover_color: Vec4,

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

    /// Remember where trigger was inserted to support `inline_search`.
    #[rust]
    trigger_position: Option<usize>,

    /// Remmeber which was the last cursor position handled, to support `inline_search`.
    #[rust]
    prev_cursor_position: usize,
}

impl Widget for CommandTextInput {
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text_input_ref().set_text(cx, v);
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

        if cx.has_key_focus(self.text_input_ref().area()) {
            if let Event::TextInput(input_event) = event {
                self.on_text_inserted(cx, scope, &input_event.input);
            }

            if self.inline_search {
                if let Some(trigger_pos) = self.trigger_position {
                    let current_pos = get_head(&self.text_input_ref());
                    let current_search = self.search_text();

                    if current_pos < trigger_pos || graphemes(&current_search).any(is_whitespace) {
                        self.hide_popup(cx);
                        self.redraw(cx);
                    } else if self.prev_cursor_position != current_pos {
                        // mimic how discord updates the filter when moving the cursor
                        cx.widget_action(
                            self.widget_uid(),
                            &scope.path,
                            InternalAction::ShouldBuildItems,
                        );
                    }
                }
            }
        }

        if cx.has_key_focus(self.key_controller_text_input_ref().area()) {
            if let Event::KeyDown(key_event) = event {
                match key_event.key_code {
                    KeyCode::ArrowDown => self.on_keyboard_move(cx, 1),
                    KeyCode::ArrowUp => self.on_keyboard_move(cx, -1),
                    KeyCode::ReturnKey => {
                        self.on_keyboard_controller_input_submit(cx, scope);
                    }
                    KeyCode::Escape => {
                        self.is_text_input_focus_pending = true;
                        self.hide_popup(cx);
                        self.redraw(cx);
                    }
                    _ => {}
                };
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

            for action in actions.iter().filter_map(|a| a.as_widget_action()) {
                if action.widget_uid == self.key_controller_text_input_ref().widget_uid() {
                    if let TextInputAction::KeyFocusLost = action.cast() {
                        self.hide_popup(cx);
                        self.redraw(cx);
                    }
                }

                if action.widget_uid == self.search_input_ref().widget_uid() {
                    if let TextInputAction::Change(search) = action.cast() {
                        // disallow multiline input
                        self.search_input_ref()
                            .set_text(cx, search.lines().next().unwrap_or_default());

                        cx.widget_action(
                            self.widget_uid(),
                            &scope.path,
                            InternalAction::ShouldBuildItems,
                        );
                    }
                }
            }
        }

        self.prev_cursor_position = get_head(&self.text_input_ref());
    }
}

impl CommandTextInput {
    fn on_text_inserted(&mut self, cx: &mut Cx, scope: &mut Scope, inserted: &str) {
        if graphemes(inserted).last() == self.trigger_grapheme() {
            self.show_popup(cx);
            self.trigger_position = Some(get_head(&self.text_input_ref()));

            if self.inline_search {
                self.view(id!(search_input_wrapper)).set_visible(cx, false);
            } else {
                self.view(id!(search_input_wrapper)).set_visible(cx, true);
                self.is_search_input_focus_pending = true;
            }

            cx.widget_action(
                self.widget_uid(),
                &scope.path,
                InternalAction::ShouldBuildItems,
            );
        }
    }

    fn on_keyboard_controller_input_submit(&mut self, cx: &mut Cx, scope: &mut Scope) {
        let Some(idx) = self.keyboard_focus_index else {
            return;
        };

        self.select_item(cx, scope, self.selectable_widgets[idx].clone());
    }

    fn select_item(&mut self, cx: &mut Cx, scope: &mut Scope, selected: WidgetRef) {
        self.last_selected_widget = selected;
        cx.widget_action(self.widget_uid(), &scope.path, InternalAction::ItemSelected);
        self.hide_popup(cx);
        self.is_text_input_focus_pending = true;
        self.try_remove_trigger_grapheme(cx);
        self.redraw(cx);
    }

    fn try_remove_trigger_grapheme(&mut self, cx: &mut Cx) {
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

        if self.trigger_grapheme() == Some(inserted_grapheme) {
            let at_removed = graphemes_with_pos(&text)
                .filter_map(|(p, g)| {
                    if p == inserted_grapheme_pos {
                        None
                    } else {
                        Some(g)
                    }
                })
                .collect::<String>();

            self.set_text(cx, &at_removed);
        }
    }

    fn show_popup(&mut self, cx: &mut Cx) {
        self.view(id!(popup)).set_visible(cx, true);
        self.view(id!(popup)).redraw(cx);
    }

    fn hide_popup(&mut self, cx: &mut Cx) {
        self.clear_popup(cx);
        self.view(id!(popup)).set_visible(cx, false);
    }

    /// Clear all text and hide the popup going back to initial state.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.hide_popup(cx);
        self.text_input_ref().set_text(cx, "");
    }

    fn clear_popup(&mut self, cx: &mut Cx) {
        self.trigger_position = None;
        self.search_input_ref().set_text(cx, "");
        self.search_input_ref().set_cursor(0, 0);
        self.clear_items();
    }

    /// Clears the list of items.
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
        if self.inline_search {
            if let Some(trigger_pos) = self.trigger_position {
                let text = self.text();
                let head = get_head(&self.text_input_ref());

                if head > trigger_pos {
                    text[trigger_pos..head].to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            self.search_input_ref().text()
        }
    }

    /// Checks if any item has been selected in the given `actions`
    /// and returns a reference to the selected item as a widget.
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

    /// Returns `true` if an action in the given `actions` indicates that
    /// the items to display need to be recomputed again.
    ///
    /// For example, this returns true if the trigger character was typed,
    /// if the search filter changes, etc.
    pub fn should_build_items(&self, actions: &Actions) -> bool {
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .filter(|a| a.widget_uid == self.widget_uid())
            .any(|a| matches!(a.cast(), InternalAction::ShouldBuildItems))
    }

    /// Returns a reference to the inner `TextInput` widget.
    pub fn text_input_ref(&self) -> TextInputRef {
        self.text_input(id!(text_input))
    }

    /// Returns a reference to the inner `TextInput` widget used for search.
    pub fn search_input_ref(&self) -> TextInputRef {
        self.text_input(id!(search_input))
    }

    fn trigger_grapheme(&self) -> Option<&str> {
        self.trigger.as_ref().and_then(|t| graphemes(t).next())
    }

    fn key_controller_text_input_ref(&self) -> TextInputRef {
        if self.inline_search {
            self.text_input_ref()
        } else {
            self.search_input_ref()
        }
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
    /// See [`CommandTextInput::should_build_items()`].
    pub fn should_build_items(&self, actions: &Actions) -> bool {
        self.borrow()
            .map_or(false, |inner| inner.should_build_items(actions))
    }

    /// See [`CommandTextInput::clear_items()`].
    pub fn clear_items(&mut self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_items();
        }
    }

    /// See [`CommandTextInput::add_item()`].
    pub fn add_item(&self, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add_item(widget);
        }
    }

    /// See [`CommandTextInput::add_unselectable_item()`].
    pub fn add_unselectable_item(&self, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.add_unselectable_item(widget);
        }
    }

    /// See [`CommandTextInput::item_selected()`].
    pub fn item_selected(&self, actions: &Actions) -> Option<WidgetRef> {
        self.borrow().and_then(|inner| inner.item_selected(actions))
    }

    /// See [`CommandTextInput::text_input_ref()`].
    pub fn text_input_ref(&self) -> TextInputRef {
        self.borrow()
            .map_or(WidgetRef::empty().as_text_input(), |inner| {
                inner.text_input_ref()
            })
    }

    /// See [`CommandTextInput::search_input_ref()`].
    pub fn search_input_ref(&self) -> TextInputRef {
        self.borrow()
            .map_or(WidgetRef::empty().as_text_input(), |inner| {
                inner.search_input_ref()
            })
    }

    /// See [`CommandTextInput::reset()`].
    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    /// See [`CommandTextInput::request_text_input_focus()`].
    pub fn request_text_input_focus(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.request_text_input_focus();
        }
    }

    /// See [`CommandTextInput::search_text()`].
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
    graphemes_with_pos(text).rfind(|(i, _)| *i < cursor_pos)
}

fn get_head(text_input: &TextInputRef) -> usize {
    text_input.borrow().map_or(0, |p| p.get_cursor().head.index)
}

fn is_whitespace(grapheme: &str) -> bool {
    grapheme.chars().all(char::is_whitespace)
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
    fn clear(&self) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };

        inner.clear();
    }

    fn add(&self, widget: WidgetRef) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };

        inner.add(widget);
    }
}
