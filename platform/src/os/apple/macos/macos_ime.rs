use std::collections::HashMap;

#[derive(Default)]
struct ImeKeyEvent {
    view: Option<usize>,
    composition: bool,
    handled: bool,
    command: bool,
    cancel: bool,
}

impl ImeKeyEvent {
    fn consumed(&self) -> bool {
        self.composition
            && (self.handled || self.cancel)
            && (!self.command || self.cancel)
    }
}

#[derive(Default)]
pub(crate) struct MacosImeKeyboard {
    // AppKit can dispatch another event synchronously from an input callback.
    events: Vec<ImeKeyEvent>,
    // Whether the initial physical press reached Makepad. Keep its release paired
    // even if composition starts or ends during key repeat.
    presses: HashMap<u16, bool>,
}

impl MacosImeKeyboard {
    pub(crate) fn begin_key_down(&mut self) {
        self.events.push(ImeKeyEvent::default());
    }

    pub(crate) fn begin_input_context(&mut self, view: usize, marked: bool) {
        if let Some(event) = self.events.last_mut() {
            *event = ImeKeyEvent {
                view: Some(view),
                composition: marked,
                ..Default::default()
            };
        }
    }

    fn event_for_view(&mut self, view: usize) -> Option<&mut ImeKeyEvent> {
        self.events.last_mut().filter(|event| event.view == Some(view))
    }

    pub(crate) fn composition_changed(&mut self, view: usize) {
        if let Some(event) = self.event_for_view(view) {
            event.composition = true;
        }
    }

    pub(crate) fn command(&mut self, view: usize, cancel: bool) {
        if let Some(event) = self.event_for_view(view) {
            event.command = true;
            event.cancel |= cancel;
        }
    }

    pub(crate) fn end_input_context(&mut self, view: usize, handled: bool, marked: bool) {
        if let Some(event) = self.event_for_view(view) {
            event.handled = handled;
            event.composition |= marked;
        }
    }

    pub(crate) fn end_key_down(&mut self, key: u16, repeat: bool) -> bool {
        let consumed = self.events.pop().unwrap_or_default().consumed();
        let forwarded = if repeat {
            *self.presses.entry(key).or_insert(!consumed)
        } else {
            self.presses.insert(key, !consumed);
            !consumed
        };
        forwarded && !consumed
    }

    pub(crate) fn key_up(&mut self, key: u16) -> bool {
        self.presses.remove(&key).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::MacosImeKeyboard;

    const VIEW: usize = 1;
    const RETURN: u16 = 36;
    const ESCAPE: u16 = 53;
    const BACKSPACE: u16 = 51;

    fn begin(ime: &mut MacosImeKeyboard, marked: bool) {
        ime.begin_key_down();
        ime.begin_input_context(VIEW, marked);
    }

    #[test]
    fn inactive_or_other_views_do_not_consume_keys() {
        let mut ime = MacosImeKeyboard::default();
        ime.begin_key_down();
        ime.composition_changed(VIEW);
        assert!(ime.end_key_down(RETURN, false));
        assert!(ime.key_up(RETURN));

        begin(&mut ime, false);
        ime.composition_changed(VIEW + 1);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(RETURN, false));
    }

    #[test]
    fn ordinary_text_and_unhandled_shortcuts_are_forwarded() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, false);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(0, false));
        assert!(ime.key_up(0));

        begin(&mut ime, true);
        ime.end_input_context(VIEW, false, true);
        assert!(ime.end_key_down(9, false));
        assert!(ime.key_up(9));
    }

    #[test]
    fn unhandled_shortcut_can_commit_pending_composition() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.composition_changed(VIEW);
        ime.end_input_context(VIEW, false, false);
        assert!(ime.end_key_down(9, false));
        assert!(ime.key_up(9));
    }

    #[test]
    fn candidate_commit_consumes_return_but_next_press_submits() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.composition_changed(VIEW);
        ime.end_input_context(VIEW, true, false);
        assert!(!ime.end_key_down(RETURN, false));
        assert!(!ime.key_up(RETURN));

        begin(&mut ime, false);
        ime.command(VIEW, false);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(RETURN, false));
        assert!(ime.key_up(RETURN));
    }

    #[test]
    fn deleting_last_preedit_character_does_not_delete_committed_text() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.composition_changed(VIEW);
        ime.end_input_context(VIEW, true, false);
        assert!(!ime.end_key_down(BACKSPACE, false));
        assert!(!ime.key_up(BACKSPACE));

        begin(&mut ime, false);
        ime.command(VIEW, false);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(BACKSPACE, false));
    }

    #[test]
    fn composition_begin_update_and_same_event_end_are_consumed() {
        for (before, after) in [(false, true), (true, true), (false, false)] {
            let mut ime = MacosImeKeyboard::default();
            begin(&mut ime, before);
            ime.composition_changed(VIEW);
            ime.end_input_context(VIEW, true, after);
            assert!(!ime.end_key_down(0, false));
            assert!(!ime.key_up(0));
        }
    }

    #[test]
    fn korean_commit_still_forwards_requested_editing_command() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.composition_changed(VIEW);
        ime.command(VIEW, false);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(RETURN, false));
        assert!(ime.key_up(RETURN));
    }

    #[test]
    fn commands_during_composition_reach_application_handlers() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.command(VIEW, false);
        ime.end_input_context(VIEW, true, true);
        assert!(ime.end_key_down(9, false));
        assert!(ime.key_up(9));
    }

    #[test]
    fn escape_cancels_composition_without_dismissing_modal() {
        for command in [false, true] {
            let mut ime = MacosImeKeyboard::default();
            begin(&mut ime, true);
            if command {
                ime.command(VIEW, true);
            }
            ime.composition_changed(VIEW);
            ime.end_input_context(VIEW, true, false);
            assert!(!ime.end_key_down(ESCAPE, false));
            assert!(!ime.key_up(ESCAPE));

            begin(&mut ime, false);
            ime.command(VIEW, true);
            ime.end_input_context(VIEW, true, false);
            assert!(ime.end_key_down(ESCAPE, false));
            assert!(ime.key_up(ESCAPE));
        }
    }

    #[test]
    fn cancellation_handled_by_the_view_consumes_the_key() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.command(VIEW, true);
        ime.composition_changed(VIEW);
        ime.end_input_context(VIEW, false, false);
        assert!(!ime.end_key_down(ESCAPE, false));
        assert!(!ime.key_up(ESCAPE));
    }

    #[test]
    fn holding_commit_or_escape_does_not_leak_repeats_or_release() {
        for key in [RETURN, ESCAPE] {
            let mut ime = MacosImeKeyboard::default();
            begin(&mut ime, true);
            ime.composition_changed(VIEW);
            ime.end_input_context(VIEW, true, false);
            assert!(!ime.end_key_down(key, false));

            begin(&mut ime, false);
            ime.command(VIEW, key == ESCAPE);
            ime.end_input_context(VIEW, true, false);
            assert!(!ime.end_key_down(key, true));
            assert!(!ime.key_up(key));
        }
    }

    #[test]
    fn forwarded_press_keeps_release_when_repeat_enters_composition() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, false);
        ime.end_input_context(VIEW, true, false);
        assert!(ime.end_key_down(0, false));

        begin(&mut ime, false);
        ime.composition_changed(VIEW);
        ime.end_input_context(VIEW, true, true);
        assert!(!ime.end_key_down(0, true));
        assert!(ime.key_up(0));
    }

    #[test]
    fn nested_events_keep_their_own_composition_and_key_release() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.composition_changed(VIEW);

        ime.begin_key_down();
        ime.begin_input_context(VIEW + 1, false);
        ime.command(VIEW + 1, false);
        ime.end_input_context(VIEW + 1, true, false);
        assert!(ime.end_key_down(RETURN, false));

        ime.end_input_context(VIEW, true, false);
        assert!(!ime.end_key_down(ESCAPE, false));
        assert!(ime.key_up(RETURN));
        assert!(!ime.key_up(ESCAPE));
    }

    #[test]
    fn fresh_press_replaces_suppression_if_release_was_not_received() {
        let mut ime = MacosImeKeyboard::default();
        begin(&mut ime, true);
        ime.end_input_context(VIEW, true, true);
        assert!(!ime.end_key_down(ESCAPE, false));

        begin(&mut ime, false);
        ime.end_input_context(VIEW, false, false);
        assert!(ime.end_key_down(ESCAPE, false));
        assert!(ime.key_up(ESCAPE));
        assert!(ime.key_up(RETURN));
    }
}
