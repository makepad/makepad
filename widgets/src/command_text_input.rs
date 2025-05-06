use crate::*;
use makepad_draw::text::selection::Cursor;
use unicode_segmentation::UnicodeSegmentation;

live_design! {
    link widgets;
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;

    List = {{List}} {
        flow: Down,
        width: Fill,
        height: Fill,
    }

    pub CommandTextInput = {{CommandTextInput}} {
        flow: Down,
        height: Fit,

        popup = <RoundedView> {
            flow: Down,
            height: Fit,
            visible: false,

            draw_bg: {
                color: (THEME_COLOR_FG_APP),
                border_size: (THEME_BEVELING),
                border_color: (THEME_COLOR_BEVEL),
                border_radius: (THEME_CORNER_RADIUS)

                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                    // External outline (entire component including border)
                    sdf.box_all(
                        0.0,
                        0.0,
                        self.rect_size.x,
                        self.rect_size.y,
                        self.border_radius,
                        self.border_radius,
                        self.border_radius,
                        self.border_radius
                    );
                    sdf.fill(self.border_color);  // Fill the entire area with border color

                    // Internal outline (content area)
                    sdf.box_all(
                        self.border_size,
                        self.border_size,
                        self.rect_size.x - self.border_size * 2.0,
                        self.rect_size.y - self.border_size * 2.0,
                        self.border_radius - self.border_size,
                        self.border_radius - self.border_size,
                        self.border_radius - self.border_size,
                        self.border_radius - self.border_size
                    );
                    sdf.fill(self.color);  // Fill content area with background color

                    return sdf.result;
                }
            }

            header_view = <View> {
                width: Fill,
                height: Fit,
                padding: {left: 12., right: 12., top: 12., bottom: 12.}
                show_bg: true
                visible: true,
                draw_bg: {
                    color: (THEME_COLOR_FG_APP),
                    instance top_radius: (THEME_CORNER_RADIUS),
                    instance border_color: (THEME_COLOR_BEVEL),
                    instance border_width: (THEME_BEVELING)
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        sdf.box_all(
                            0.0,
                            0.0,
                            self.rect_size.x,
                            self.rect_size.y,
                            self.top_radius,
                            self.top_radius,
                            1.0,
                            1.0
                        );
                        sdf.fill(self.color);
                        return sdf.result
                    }
                }

                header_label = <Label> {
                    draw_text: {
                        color: (THEME_COLOR_LABEL_INNER)
                        text_style: {
                            font_size: (THEME_FONT_SIZE_4)
                        }
                    }
                }
            }


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
    pub color_focus: Vec4,

    /// Weak color to highlight the item that the pointer is hovering over.
    #[live]
    pub color_hover: Vec4,

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
        self.ensure_popup_consistent(cx);

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
        if cx.has_key_focus(self.key_controller_text_input_ref().area()) {
            if let Event::KeyDown(key_event) = event {
                let popup_visible = self.view(id!(popup)).visible();

                if popup_visible {
                    let mut eat_the_event = true;

                    match key_event.key_code {
                        KeyCode::ArrowDown => {
                            // Clear mouse hover when using up/down keys
                            self.pointer_hover_index = None;
                            self.on_keyboard_move(cx, 1);
                        },
                        KeyCode::ArrowUp => {
                            // Clear mouse hover when using up/down keys
                            self.pointer_hover_index = None;
                            self.on_keyboard_move(cx, -1);
                        },
                        KeyCode::ReturnKey => {
                            self.on_keyboard_controller_input_submit(cx, scope);
                        }
                        KeyCode::Escape => {
                            self.is_text_input_focus_pending = true;
                            self.hide_popup(cx);
                            self.redraw(cx);
                        }
                        _ => {
                            eat_the_event = false;
                        }
                    };

                    if eat_the_event {
                        return;
                    }
                }
            }
        }

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
                        self.ensure_popup_consistent(cx);
                    }
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

                    // Clear keyboard focus when mouse is clicked
                    self.keyboard_focus_index = None;
                }

                if item.finger_hover_out(actions).is_some() && Some(idx) == self.pointer_hover_index
                {
                    self.pointer_hover_index = None;
                    should_redraw = true;
                }

                if item.finger_hover_in(actions).is_some() {
                    // When mouse enters item, clear keyboard focus and set mouse hover index
                    self.pointer_hover_index = Some(idx);
                    self.keyboard_focus_index = None;
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
                    if let TextInputAction::Changed(search) = action.cast() {
                        // disallow multiline input
                        self.search_input_ref()
                            .set_text(cx, search.lines().next().unwrap_or_default());

                        cx.widget_action(
                            self.widget_uid(),
                            &scope.path,
                            InternalAction::ShouldBuildItems,
                        );
                        self.ensure_popup_consistent(cx);
                    }
                }
            }
        }

        self.prev_cursor_position = get_head(&self.text_input_ref());
        self.ensure_popup_consistent(cx);
    }
}

impl CommandTextInput {
    // Ensure popup state consistency
    fn ensure_popup_consistent(&mut self, cx: &mut Cx) {
        if self.view(id!(popup)).visible() {
            if self.inline_search {
                self.view(id!(search_input_wrapper)).set_visible(cx, false);
            } else {
                self.view(id!(search_input_wrapper)).set_visible(cx, true);
            }
        }
    }

    pub fn keyboard_focus_index(&self) -> Option<usize> {
        self.keyboard_focus_index
    }

    /// Sets the keyboard focus index for the list of selectable items
    /// Only updates the visual highlight state of the dropdown items
    pub fn set_keyboard_focus_index(&mut self, idx: usize) {
        // Only process if popup is visible and we have items
        if !self.selectable_widgets.is_empty() {
            // Simply update the focus index within valid bounds
            self.keyboard_focus_index = Some(idx.clamp(0, self.selectable_widgets.len() - 1));
        }
    }

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
            self.ensure_popup_consistent(cx);
        }
    }

    fn on_keyboard_controller_input_submit(&mut self, cx: &mut Cx, scope: &mut Scope) {
        let Some(idx) = self.keyboard_focus_index else {
            return;
        };

        self.select_item(cx, scope, self.selectable_widgets[idx].clone());
    }

    fn select_item(&mut self, cx: &mut Cx, scope: &mut Scope, selected: WidgetRef) {
        self.try_remove_trigger_and_inline_search(cx);
        self.last_selected_widget = selected;
        cx.widget_action(self.widget_uid(), &scope.path, InternalAction::ItemSelected);
        self.hide_popup(cx);
        self.is_text_input_focus_pending = true;
        self.redraw(cx);
    }

    fn try_remove_trigger_and_inline_search(&mut self, cx: &mut Cx) {
        let mut to_remove = self.trigger_grapheme().unwrap_or_default().to_string();

        if self.inline_search {
            to_remove.push_str(&self.search_text());
        }

        let text = self.text();
        let end = get_head(&self.text_input_ref());
        // Use graphemes instead of byte indices
        let text_graphemes: Vec<&str> = text.graphemes(true).collect();
        let mut byte_index = 0;
        let mut end_grapheme_idx = 0;

        // Find the grapheme index corresponding to the end position
        for (i, g) in text_graphemes.iter().enumerate() {
            if byte_index <= end && byte_index + g.len() > end {
                end_grapheme_idx = i;
                break;
            }
            byte_index += g.len();
        }

        // Calculate the start grapheme index
        let start_grapheme_idx = if end_grapheme_idx >= to_remove.graphemes(true).count() {
            end_grapheme_idx - to_remove.graphemes(true).count()
        } else {
            return;
        };

        // Rebuild the string
        let new_text = text_graphemes[..start_grapheme_idx].join("") +
                        &text_graphemes[end_grapheme_idx..].join("");

        // Calculate the new cursor position (grapheme)
        let new_cursor_pos = text_graphemes[..start_grapheme_idx].join("").graphemes(true).count();

        self.text_input_ref().set_cursor(
            cx,
            Cursor {
                index: new_cursor_pos,
                prefer_next_row: false,
            },
            false
        );
        self.set_text(cx, &new_text);
    }

    fn show_popup(&mut self, cx: &mut Cx) {
        if self.inline_search {
            self.view(id!(search_input_wrapper)).set_visible(cx, false);
        } else {
            self.view(id!(search_input_wrapper)).set_visible(cx, true);
        }
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
        self.search_input_ref().set_cursor(
            cx,
            Cursor {
                index: 0,
                prefer_next_row: false,
            },
            false
        );
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
        // Define maximum search text length to prevent performance issues with very long search texts
        const MAX_SEARCH_TEXT_LENGTH: usize = 100;

        if self.inline_search {
            if let Some(trigger_pos) = self.trigger_position {
                let text = self.text();
                let head = get_head(&self.text_input_ref());

                if head > trigger_pos {
                    // Parse text into graphemes (Unicode grapheme clusters)
                    let text_graphemes: Vec<&str> = text.graphemes(true).collect();
                    let mut byte_pos = 0;
                    let mut trigger_grapheme_idx = None;
                    let mut head_grapheme_idx = None;
                    let mut last_grapheme_end = 0;

                    // Single-pass traversal to calculate all grapheme indices
                    for (i, g) in text_graphemes.iter().enumerate() {
                        // Check if the trigger character is within this grapheme
                        if byte_pos <= trigger_pos && byte_pos + g.len() > trigger_pos {
                            trigger_grapheme_idx = Some(i);
                        }
                        // Check if the trigger character is exactly at the end of this grapheme
                        else if byte_pos + g.len() == trigger_pos {
                            // Special case: trigger at grapheme boundary, point to the next grapheme
                            trigger_grapheme_idx = Some(i + 1);
                        }

                        // Check if the cursor is within this grapheme
                        if byte_pos <= head && byte_pos + g.len() > head {
                            head_grapheme_idx = Some(i);
                        }
                        // Check if the cursor is exactly at the end of this grapheme
                        else if byte_pos + g.len() == head {
                            // Special case: cursor at grapheme boundary, point to the next grapheme
                            head_grapheme_idx = Some(i + 1);
                        }

                        byte_pos += g.len();
                        last_grapheme_end = byte_pos;
                    }

                    // Handle edge cases at the end of text symmetrically for both positions
                    if head_grapheme_idx.is_none() && head >= last_grapheme_end {
                        head_grapheme_idx = Some(text_graphemes.len());
                    }

                    if trigger_grapheme_idx.is_none() && trigger_pos >= last_grapheme_end {
                        trigger_grapheme_idx = Some(text_graphemes.len());
                    }

                    // Safety check and use indices only if they're valid
                    if let (Some(t_idx), Some(h_idx)) = (trigger_grapheme_idx, head_grapheme_idx) {
                        // Additional range check to prevent index errors
                        if t_idx >= text_graphemes.len() || h_idx > text_graphemes.len() {
                            log!("Error: Grapheme indices out of range: t_idx={}, h_idx={}, graphemes_len={}",
                                 t_idx, h_idx, text_graphemes.len());
                            return String::new();
                        }

                        if t_idx < h_idx {
                            // Check length limit
                            let length = h_idx - t_idx;
                            if length > MAX_SEARCH_TEXT_LENGTH {
                                log!("Warning: Search text length({}) exceeds maximum limit({})", length, MAX_SEARCH_TEXT_LENGTH);
                                // Still return text but truncated to the maximum length
                                return text_graphemes[t_idx..t_idx + MAX_SEARCH_TEXT_LENGTH].join("");
                            }

                            // Optimized string building with pre-allocated capacity
                            let mut result = String::with_capacity(
                                text_graphemes[t_idx..h_idx].iter().map(|g| g.len()).sum()
                            );
                            for g in &text_graphemes[t_idx..h_idx] {
                                result.push_str(g);
                            }
                            return result;
                        } else if t_idx == h_idx {
                            // Edge case: trigger character and cursor in the same grapheme
                            return String::new();
                        } else {
                            // Abnormal case: trigger character is after the cursor
                            log!("Warning: Trigger character is after cursor: trigger_idx={}, head_idx={}, trigger_pos={}, head={}",
                                 t_idx, h_idx, trigger_pos, head);
                            return String::new();
                        }
                    } else {
                        // Comprehensive diagnostic information
                        log!("Warning: Unable to find valid grapheme indices: trigger_idx={:?}, head_idx={:?}, trigger_pos={}, head={}, text_len={}, graphemes_len={}",
                             trigger_grapheme_idx, head_grapheme_idx, trigger_pos, head, text.len(), text_graphemes.len());
                        return String::new();
                    }
                }

                // Cursor is at or before the trigger position
                String::new()
            } else {
                // No trigger position
                String::new()
            }
        } else {
            // Non-inline search mode
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
            // If no keyboard focus exists but user pressed arrow keys, focus on first item
            if !self.selectable_widgets.is_empty() {
                if delta > 0 {
                    self.keyboard_focus_index = Some(0);
                } else {
                    self.keyboard_focus_index = Some(self.selectable_widgets.len() - 1);
                }
            }
            return;
        };

        let new_index = idx
            .saturating_add_signed(delta as isize)
            .clamp(0, self.selectable_widgets.len() - 1);

        if idx != new_index {
            self.keyboard_focus_index = Some(new_index);
        }

        // Clear mouse hover state when using keyboard navigation
        // This ensures keyboard navigation and mouse hover don't appear simultaneously
        self.pointer_hover_index = None;

        self.redraw(cx);
    }

    fn update_highlights(&mut self, cx: &mut Cx) {
        // Check if currently there is a keyboard-focused item
        let has_keyboard_focus = self.keyboard_focus_index.is_some();

        for (idx, item) in self.selectable_widgets.iter().enumerate() {
            item.apply_over(cx, live! { show_bg: true, cursor: Hand });

            // If there is a keyboard focus, prioritize it over mouse hover
            // If there is no keyboard focus, show mouse hover
            if Some(idx) == self.keyboard_focus_index {
                // Keyboard-selected item is highlighted in blue
                item.apply_over(
                    cx,
                    live! {
                        draw_bg: {
                            color: (self.color_focus),
                        }
                    },
                );
            } else if Some(idx) == self.pointer_hover_index && !has_keyboard_focus {
                // Mouse-hovered item is highlighted in gray, but only when there is no keyboard focus
                item.apply_over(
                    cx,
                    live! {
                        draw_bg: {
                            color: (self.color_hover),
                        }
                    },
                );
            } else {
                // Default state
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

fn get_head(text_input: &TextInputRef) -> usize {
    text_input.borrow().map_or(0, |p| p.cursor().index)
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
