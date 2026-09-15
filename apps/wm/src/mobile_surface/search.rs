//! The launcher uses the same editor as applications: caret, selection,
//! clipboard and native IME stay with TextInput. Only its soft keys are hosted.
use super::*;

fn matching_apps<'a>(apps: &'a [(String, String)], query: &str) -> Vec<&'a (String, String)> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<_> = apps
        .iter()
        .filter(|(id, label)| {
            let name = format!("{id} {label}").to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .collect();
    found.sort_by_cached_key(|(_, label)| label.to_lowercase());
    found
}

impl PhoneSurface {
    pub fn dismiss_search(&mut self, cx: &mut Cx, phone: &mut PhoneState, clear: bool) {
        let input = self.search.text_input(cx, ids!(input));
        if !input.area().is_empty() && cx.has_key_focus(input.area()) {
            cx.set_key_focus(Area::Empty);
            cx.hide_text_ime();
        }
        phone.search_focused = false;
        self.search_pointer = false;
        if clear && !phone.search_query.is_empty() {
            input.set_text(cx, "");
            phone.search_query.clear();
            phone.search_scroll = 0.0;
        }
    }

    pub fn clear_search(&mut self, cx: &mut Cx, phone: &mut PhoneState) {
        let input = self.search.text_input(cx, ids!(input));
        input.set_text(cx, "");
        input.take_key_focus(cx);
        phone.search_query.clear();
        phone.search_scroll = 0.0;
        phone.search_focused = true;
    }

    pub fn focus_search(&mut self, cx: &mut Cx, phone: &mut PhoneState) {
        self.search.text_input(cx, ids!(input)).take_key_focus(cx);
        phone.search_focused = true;
    }

    /// Pointer capture stays with the editor for selection drags. Soft-key
    /// presses never reach its outside-click handler and therefore keep focus.
    pub fn search_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        phone: &mut PhoneState,
        enabled: bool,
    ) -> bool {
        let input = self.search.text_input(cx, ids!(input));
        if !enabled {
            self.dismiss_search(cx, phone, phone.screen != PhoneScreen::Drawer);
            if matches!(
                event,
                Event::KeyFocus(_) | Event::KeyFocusLost(_) | Event::Timer(_) | Event::NextFrame(_)
            ) {
                self.search.handle_event(cx, event, &mut Scope::empty());
            }
            return false;
        }
        let pointer = match event {
            Event::MouseDown(e) => Some((e.abs, true, false)),
            Event::MouseMove(e) => Some((e.abs, false, false)),
            Event::MouseUp(e) => Some((e.abs, false, true)),
            Event::TouchUpdate(e) => e.touches.first().map(|t| {
                (
                    t.abs,
                    t.state == makepad_platform::event::TouchState::Start,
                    t.state == makepad_platform::event::TouchState::Stop,
                )
            }),
            _ => None,
        };
        let mut consumed = false;
        if let Some((point, down, up)) = pointer {
            let inside = self.search_rect.contains(point) && self.hit(point).is_none();
            if down && inside {
                self.search_pointer = true;
            }
            consumed = inside || self.search_pointer;
            if up {
                self.search_pointer = false;
            }
            if !consumed {
                return false;
            }
        }
        let focused = cx.has_key_focus(input.area());
        if focused {
            consumed |= matches!(
                event,
                Event::KeyDown(_)
                    | Event::KeyUp(_)
                    | Event::TextInput(_)
                    | Event::TextCopy(_)
                    | Event::TextCut(_)
            );
        }
        let actions =
            cx.capture_actions(|cx| self.search.handle_event(cx, event, &mut Scope::empty()));
        if input.returned(&actions).is_some() || input.escaped(&actions) {
            self.dismiss_search(cx, phone, input.escaped(&actions));
        } else {
            phone.search_focused = cx.has_key_focus(input.area());
        }
        let query = input.text();
        if phone.search_query != query {
            phone.search_query = query;
            phone.search_scroll = 0.0;
        }
        consumed
    }

    /// `backdrop` is the compositor checkpoint the pill's glass samples (iOS
    /// only; Android's drawer keeps a flat pill), `amplitude` the library's
    /// arrival 0 → 1, which the lens follows instead of popping in.
    pub(super) fn draw_search(
        &mut self,
        cx: &mut Cx2d,
        state: &WmState,
        screen: Rect,
        ink: Vec4f,
        backdrop: Option<GaussBlurSnapshot>,
        amplitude: f32,
    ) -> Rect {
        let ios = state.style.target == DesktopStyle::Ios;
        let editing = state.phone.searching();
        // Under the status bar: the fake one, or the phone's real inset.
        let top = state.phone.chrome.top_reserve(screen);
        let pill = rect(
            screen.pos.x + 20.0,
            screen.pos.y + top + 10.0,
            screen.size.x - 40.0 - if editing { 64.0 } else { 0.0 },
            40.0,
        );
        self.search_rect = pill;
        if self.search_style != Some((ios, state.style.dark)) {
            let mut input = self.search.text_input(cx, ids!(input));
            let muted = alpha(ink, 0.55);
            if ios {
                script_apply_eval!(cx,input,{draw_text.text_style: mod.widgets.PhoneSurface.ios_font{font_size: 14.0}});
            } else {
                script_apply_eval!(cx,input,{draw_text.text_style: mod.widgets.PhoneSurface.android_font{font_size: 14.0}});
            }
            script_apply_eval!(cx, input, {
                draw_text +: {
                    color: #(ink) color_hover: #(ink) color_focus: #(ink) color_down: #(ink)
                    color_empty: #(muted)
                }
            });
            input.set_empty_text(cx, if ios { "App Library" } else { "Search apps" }.into());
            self.search_style = Some((ios, state.style.dark));
        }
        let accent = if ios {
            rgb(0, 122, 255)
        } else {
            rgb(126, 94, 190)
        };
        if ios && backdrop.is_some() {
            // The material's pill profile; a focused pill wears the accent
            // ring around it, the glass itself unchanged.
            let dark = state.style.dark;
            let profile = if state.accessibility.reduce_transparency {
                GlassProfile::pill(dark).opaque(dark)
            } else {
                GlassProfile::pill(dark)
            };
            self.search_glass.apply_profile(cx, &profile);
            self.search_glass.set_lens_amplitude(cx, if state.accessibility.reduce_motion { 1.0 } else { amplitude });
            if state.phone.search_focused {
                self.rounded(cx, rect(pill.pos.x - 1.5, pill.pos.y - 1.5, pill.size.x + 3.0, pill.size.y + 3.0), 13.5, alpha(accent, 0.65));
            }
            self.search_glass.draw_surface_with_backdrop(cx, pill, backdrop, 1.0);
        } else if state.phone.search_focused {
            self.rounded(cx, pill, 14.0, alpha(accent, 0.65));
            self.rounded(
                cx,
                rect(
                    pill.pos.x + 1.5,
                    pill.pos.y + 1.5,
                    pill.size.x - 3.0,
                    pill.size.y - 3.0,
                ),
                12.5,
                if state.style.dark {
                    rgb(40, 40, 48)
                } else {
                    rgb(238, 238, 245)
                },
            );
        } else {
            self.rounded(cx, pill, 14.0, alpha(ink, 0.10));
        }
        self.d.icon_centered(
            cx,
            Ico::Search,
            rect(pill.pos.x + 8.0, pill.pos.y, 28.0, pill.size.y),
            15.0,
            alpha(ink, 0.55),
        );
        self.search
            .draw_walk_all(cx, &mut Scope::empty(), Walk::abs_rect(pill));
        if !state.phone.search_query.is_empty() {
            let clear = rect(pill.pos.x + pill.size.x - 32.0, pill.pos.y, 32.0, 40.0);
            self.label(cx, clear, "×", 20.0, false, alpha(ink, 0.6));
            self.hits.push((clear, PhoneHit::ClearSearch));
        }
        if editing {
            let cancel = rect(pill.pos.x + pill.size.x + 4.0, pill.pos.y, 60.0, 40.0);
            self.label(cx, cancel, "Cancel", 13.0, false, accent);
            self.hits.push((cancel, PhoneHit::CancelSearch));
        }
        pill
    }

    pub(super) fn draw_search_results(
        &mut self,
        cx: &mut Cx2d,
        state: &WmState,
        screen: Rect,
        pill: Rect,
        apps: &[(String, String)],
        ink: Vec4f,
    ) {
        let found = matching_apps(apps, &state.phone.search_query);
        let top = pill.pos.y + pill.size.y + 14.0;
        let bottom = screen.pos.y + screen.size.y
            - state.phone.keyboard.max(state.phone.keyboard_target)
            - 28.0;
        let height = (bottom - top).max(0.0);
        if found.is_empty() {
            self.label(
                cx,
                rect(screen.pos.x + 20.0, top + 24.0, screen.size.x - 40.0, 30.0),
                "No apps found",
                15.0,
                false,
                alpha(ink, 0.6),
            );
            return;
        }
        self.search_scroll_max = (found.len() as f64 * 56.0 - height).max(0.0);
        let scroll = state.phone.search_scroll.clamp(0.0, self.search_scroll_max);
        cx.begin_turtle(
            Walk::abs_rect(rect(screen.pos.x, top, screen.size.x, height)),
            Layout::default(),
        );
        for (index, (id, label)) in found.into_iter().enumerate() {
            let y = top + index as f64 * 56.0 - scroll;
            if y + 56.0 <= top || y >= bottom {
                continue;
            }
            let row = rect(screen.pos.x + 20.0, y, screen.size.x - 40.0, 56.0);
            self.icons.draw(
                cx,
                id,
                state.style.target,
                rect(row.pos.x + 4.0, y + 6.0, 44.0, 44.0),
                1.0,
                ink,
            );
            self.d.label_elided(
                cx,
                rect(row.pos.x + 64.0, y, row.size.x - 64.0, 56.0),
                false,
                16.0,
                ink,
                HAlign::Left,
                label,
            );
            self.d.solid(
                cx,
                rect(row.pos.x + 64.0, y + 55.0, row.size.x - 64.0, 0.5),
                alpha(ink, 0.12),
            );
            let y0 = y.max(top);
            self.hits.push((
                rect(row.pos.x, y0, row.size.x, (y + 56.0).min(bottom) - y0),
                PhoneHit::App(id.clone()),
            ));
        }
        cx.end_turtle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_matches_every_word_in_names_and_ids_and_sorts_labels() {
        let apps = vec![
            ("task".into(), "Task Manager".into()),
            ("files".into(), "Files".into()),
            ("photos".into(), "Photos".into()),
        ];
        assert_eq!(matching_apps(&apps, "TASK man")[0].0, "task");
        assert_eq!(
            matching_apps(&apps, "")
                .iter()
                .map(|a| a.0.as_str())
                .collect::<Vec<_>>(),
            ["files", "photos", "task"]
        );
        assert!(matching_apps(&apps, "task photos").is_empty());
    }
}
