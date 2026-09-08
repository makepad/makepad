// The status bar, utility panel and F10 tools read one worker snapshot.
impl App {
    fn hide_usage_email(&mut self, cx: &mut Cx) {
        self.usage_email_hover = None;
        if self.usage_email_visible {
            self.usage_email_visible = false;
            self.ui.tooltip(cx, ids!(usage_email_tooltip)).hide(cx);
        }
    }

    fn handle_usage_email_hover(&mut self, cx: &mut Cx, event: &Event) {
        if self.usage_history_provider.is_some()
            || self.ui.modal(cx, ids!(utility_overlay)).is_open()
            || matches!(event, Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_)
                | Event::KeyDown(_) | Event::TouchUpdate(_) | Event::WindowLostFocus(_) | Event::MouseLeave(_)
                | Event::WindowGeomChange(_)) {
            self.hide_usage_email(cx);
            return;
        }
        let now = cx.seconds_since_app_start();
        if let Event::MouseMove(pointer) = event {
            let strip = self.ui.view(cx, ids!(usage_strip)).area().rect(cx);
            let provider = [(UsageProvider::Claude, id!(fable_usage)), (UsageProvider::Codex, id!(astra_usage))]
                .into_iter().find_map(|(provider, id)| {
                    let area = self.ui.view(cx, &[id]).area();
                    (!area.is_empty() && strip.contains(pointer.abs) && area.rect(cx).contains(pointer.abs)).then_some(provider)
                });
            if provider != self.usage_email_hover.map(|(provider, _)| provider) {
                self.hide_usage_email(cx);
                self.usage_email_hover = provider.map(|provider| (provider, now));
            }
        }
        let Some((provider, entered)) = self.usage_email_hover else { return; };
        let age = now - entered;
        if age >= 4.35 {
            if self.usage_email_visible {
                self.usage_email_visible = false;
                self.ui.tooltip(cx, ids!(usage_email_tooltip)).hide(cx);
            }
            // Keep the expired hover until the pointer leaves this button.
        } else if age >= 0.35 {
            let email = self.usage_snapshot.provider(provider)
                .and_then(|usage| usage.account_email.as_deref()).unwrap_or("Account unavailable");
            if self.usage_email_visible {
                self.ui.tooltip(cx, ids!(usage_email_tooltip)).set_text(cx, email);
            } else {
                let id = if provider == UsageProvider::Claude { id!(fable_usage) } else { id!(astra_usage) };
                let rect = self.ui.view(cx, &[id]).area().rect(cx);
                let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
                let pos = dvec2(rect.pos.x.clamp(8.0, (size.x - 344.0).max(8.0)), rect.pos.y + rect.size.y + 5.0);
                self.ui.tooltip(cx, ids!(usage_email_tooltip)).show_with_options(cx, pos, email);
                self.usage_email_visible = true;
            }
        }
    }

    fn close_usage_history(&mut self, cx: &mut Cx) {
        self.usage_history_provider = None;
        self.ui.modal(cx, ids!(usage_history_overlay)).close(cx);
    }

    fn toggle_usage_history(&mut self, cx: &mut Cx, provider: UsageProvider) {
        self.hide_usage_email(cx);
        if self.usage_history_provider == Some(provider)
            && self.ui.modal(cx, ids!(usage_history_overlay)).is_open() {
            self.close_usage_history(cx);
            return;
        }
        self.close_utility(cx);
        self.usage_history_provider = Some(provider);
        self.ui.label(cx, ids!(usage_history_title)).set_text(cx, &format!("{} · Account history", provider.label()));
        self.refresh_usage_history(cx);
        self.ui.modal(cx, ids!(usage_history_overlay)).open(cx);
    }

    fn refresh_usage_history(&self, cx: &mut Cx) {
        let Some(provider) = self.usage_history_provider else { return; };
        if let Some(mut view) = self.ui.widget(cx, ids!(usage_history_list)).borrow_mut::<StudioUsageHistoryView>() {
            view.set_snapshot(cx, self.usage_snapshot.clone(), provider);
        }
        self.refresh_usage_history_layout(cx);
    }

    fn refresh_usage_history_layout(&self, cx: &mut Cx) {
        let Some(provider) = self.usage_history_provider else { return; };
        let island = if provider == UsageProvider::Claude { id!(fable_island) } else { id!(astra_island) };
        let anchor = self.ui.view(cx, &[island]).area().rect(cx);
        let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
        if size.x <= 0.0 || size.y <= 0.0 { return; }
        let width = (size.x - 16.0).clamp(1.0, 560.0);
        let y = (anchor.pos.y + anchor.size.y + 5.0).clamp(8.0, (size.y - 80.0).max(8.0));
        let x = anchor.pos.x.clamp(8.0, (size.x - width - 8.0).max(8.0));
        let preferred = self.ui.widget(cx, ids!(usage_history_list)).borrow::<StudioUsageHistoryView>()
            .map(|view| view.preferred_height()).unwrap_or(100.0);
        let height = (preferred + 54.0).clamp(110.0, 420.0).min((size.y - y - 8.0).max(1.0));
        if let Some(mut content) = self.ui.view(cx, ids!(usage_history_overlay.content)).borrow_mut() {
            let pos = Some(dvec2(x, y));
            if content.walk.width != Size::Fixed(width) || content.walk.height != Size::Fixed(height) || content.walk.abs_pos != pos {
                content.walk.width = Size::Fixed(width);
                content.walk.height = Size::Fixed(height);
                content.walk.abs_pos = pos;
                content.redraw(cx);
            }
        }
    }

    fn handle_usage_history_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.modal(cx, ids!(usage_history_overlay)).dismissed(actions) {
            self.close_usage_history(cx);
        }
        for (id, provider) in [(id!(fable_history), UsageProvider::Claude), (id!(astra_history), UsageProvider::Codex)] {
            if self.ui.button(cx, &[id]).clicked(actions) {
                self.toggle_usage_history(cx, provider);
            }
        }
        self.refresh_usage_history_layout(cx);
    }

    fn handle_usage_history_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.usage_history_provider.is_none() { return false; }
        match event {
            Event::KeyDown(key) if key.key_code == KeyCode::Escape => return true,
            Event::KeyUp(key) if key.key_code == KeyCode::Escape => {
                self.close_usage_history(cx);
                return true;
            }
            Event::WindowLostFocus(_) => self.close_usage_history(cx),
            _ => {}
        }
        false
    }

    fn flow_usage_provider(&self, flow: &str) -> Option<UsageProvider> {
        let provider = self
            .agent_sessions
            .bindings
            .get(&self.flow_terminal_id(flow))
            .map(|binding| binding.provider.as_str())
            .or_else(|| {
                self.iterations
                    .snapshot
                    .engine
                    .flows
                    .get(flow)?
                    .config
                    .agent_provider
                    .as_deref()
            });
        match provider {
            Some("claude") => Some(UsageProvider::Claude),
            Some("codex") => Some(UsageProvider::Codex),
            _ => None,
        }
    }
    fn recovery_lane(&self, provider: UsageProvider) -> Option<String> {
        let active = |flow: &str| {
            self.iterations
                .snapshot
                .engine
                .flows
                .get(flow)
                .is_some_and(|f| {
                    f.lifecycle == iteration::FlowLifecycle::Active
                        && self.flow_usage_provider(flow) == Some(provider)
                })
        };
        self.iterations
            .selected
            .as_ref()
            .filter(|flow| active(flow) && self.usage_stalls.report(flow).is_some())
            .cloned()
            .or_else(|| {
                self.usage_stalls
                    .reports(provider)
                    .into_iter()
                    .find(|r| active(&r.lane))
                    .map(|r| r.lane.clone())
            })
            .or_else(|| {
                self.iterations
                    .selected
                    .as_ref()
                    .filter(|flow| active(flow))
                    .cloned()
            })
            .or_else(|| {
                self.iterations
                    .snapshot
                    .engine
                    .flows
                    .keys()
                    .find(|flow| active(flow))
                    .cloned()
            })
    }
    fn recover_usage_provider(&mut self, cx: &mut Cx, provider: UsageProvider) {
        let Some(flow) = self.recovery_lane(provider) else {
            self.flow_note(cx, "No active Studio lane is connected to this provider");
            return;
        };
        let Some(lane) = self.iterations.snapshot.engine.flows.get(&flow) else {
            return;
        };
        let (title, accept, explanation) = match provider {
            UsageProvider::Claude => (
                "Sign in to Fable?",
                "Open Fable login",
                "Send /login to this lane’s existing Fable terminal. Complete sign-in there; its conversation stays open.",
            ),
            UsageProvider::Codex => (
                "Sign out and recover Astra?",
                "Sign out and sign in",
                "Save this lane’s verified conversation ID, then sign out of Codex and open browser sign-in. Resume that exact conversation after login.\n\nThis changes the shared Codex account used by other lanes.",
            ),
        };
        let message = format!("{}\n\n{explanation}", lane.title);
        self.show_utility(
            cx,
            id!(provider_recovery_confirm_tab),
            id!(ProviderRecoveryConfirmTab),
            title,
        );
        self.ui.label(cx, ids!(provider_recovery_message)).set_text(cx, &message);
        self.ui.button(cx, ids!(provider_recovery_accept)).set_text(cx, accept);
        // Freeze the lane now; changing selection while the popup is open
        // must not redirect an account operation to a different conversation.
        self.usage_recovery_confirmation = Some((provider, flow));
        cx.set_key_focus(self.ui.button(cx, ids!(provider_recovery_cancel)).area());
    }

    fn handle_usage_recovery_confirmation(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.usage_recovery_confirmation.is_none() {
            return;
        }
        if !self.ui.modal(cx, ids!(utility_overlay)).is_open() {
            self.usage_recovery_confirmation = None;
            return;
        }
        if self.ui.button(cx, ids!(provider_recovery_cancel)).clicked(actions) {
            self.close_utility(cx);
            return;
        }
        if !self.ui.button(cx, ids!(provider_recovery_accept)).clicked(actions) {
            return;
        }
        let Some((provider, flow)) = self.usage_recovery_confirmation.take() else {
            return;
        };
        self.close_utility(cx);
        if !self.iterations.snapshot.engine.flows.get(&flow).is_some_and(|lane| {
            lane.lifecycle == iteration::FlowLifecycle::Active
                && self.flow_usage_provider(&flow) == Some(provider)
        }) {
            self.flow_note(cx, "This lane is no longer connected to that provider; recovery was canceled");
            return;
        }
        self.iterations.selected = Some(flow.clone());
        self.set_workspace_mode(cx, Mode::Tasks);
        if let Err(error) = self.recover_flow_terminal(cx, &flow) {
            self.flow_note(cx, &error);
        }
    }
    fn ingest_usage_stalls(&mut self, cx: &mut Cx) {
        self.usage_stall_sequence = self.usage_stall_sequence.saturating_add(1);
        let flows: Vec<_> = self
            .iterations
            .snapshot
            .engine
            .flows
            .values()
            .filter(|flow| flow.lifecycle == iteration::FlowLifecycle::Active)
            .filter_map(|flow| {
                self.flow_usage_provider(&flow.id)
                    .map(|provider| (flow.id.clone(), provider))
            })
            .collect();
        self.usage_stalls.retain_lanes(
            &flows
                .iter()
                .map(|(flow, _)| flow.as_str())
                .collect::<Vec<_>>(),
        );
        for (flow, provider) in flows {
            let Some(tab) = self.terminal_view_for_flow(&flow) else { continue; };
            let Some(binding) = self.agent_sessions.bindings.get(&tab) else {
                continue;
            };
            let generation = self.agent_sessions.mirrors.get(&tab).or(binding.info.as_ref())
                .map(|info| info.supervisor_pid as u64)
                .unwrap_or(tab);
            let Ok(widget) = self.terminal(cx, tab) else {
                continue;
            };
            if let Some(term) = widget.borrow::<MpTerm>() {
                if let Some((rows, _, _)) = term.ai_screen_rows(Some(40)) {
                    self.usage_stalls.ingest_terminal(
                        &flow,
                        provider,
                        generation,
                        self.usage_stall_sequence,
                        &rows.join("\n"),
                        disk::now(),
                        makepad_studio::usage_stall::ActivitySignal::None,
                    );
                }
            };
        }
    }
    fn usage_json(&self) -> Value {
        let number = |value: Option<f64>| value.map(Value::F64).unwrap_or(Value::Null);
        json::obj(vec![
            ("account_history", Value::Arr(self.usage_snapshot.account_history.iter().map(|account| account.json()).collect())),
            ("history_error", self.usage_snapshot.history_error.as_ref().map(json::s).unwrap_or(Value::Null)),
            (
                "limit_reports",
                Value::Arr(
                    [UsageProvider::Claude, UsageProvider::Codex]
                        .into_iter()
                        .flat_map(|p| self.usage_stalls.reports(p))
                        .map(|r| {
                            json::obj(vec![
                                ("flow", json::s(&r.lane)),
                                ("provider", json::s(r.provider.as_str())),
                                ("status", json::s(r.label())),
                                ("evidence", json::s(&r.excerpt)),
                                ("observed_at", Value::Int(r.observed_at as i64)),
                                (
                                    "reset",
                                    r.reset_text.as_ref().map(json::s).unwrap_or(Value::Null),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "polling",
                self.usage_snapshot
                    .polling
                    .map(|p| json::s(p.as_str()))
                    .unwrap_or(Value::Null),
            ),
            (
                "next_poll_at",
                Value::Int(self.usage_snapshot.next_poll_at as i64),
            ),
            (
                "error",
                self.usage_error
                    .as_ref()
                    .map(json::s)
                    .unwrap_or(Value::Null),
            ),
            (
                "providers",
                Value::Arr(
                    self.usage_snapshot
                        .providers
                        .iter()
                        .map(|p| {
                            json::obj(vec![
                                ("provider", json::s(p.provider.as_str())),
                                ("source", json::s(&p.source)),
                                ("observed_at", Value::Int(p.observed_at as i64)),
                                ("stale", Value::Bool(p.is_stale(disk::now()))),
                                ("plan", p.plan.as_ref().map(json::s).unwrap_or(Value::Null)),
                                (
                                    "account_email",
                                    p.account_email.as_ref().map(json::s).unwrap_or(Value::Null),
                                ),
                                (
                                    "error",
                                    p.error.as_ref().map(json::s).unwrap_or(Value::Null),
                                ),
                                (
                                    "windows",
                                    Value::Arr(
                                        p.limits()
                                            .into_iter()
                                            .flatten()
                                            .map(|w| {
                                                let (date, time, zone) = w.reset_parts();
                                                json::obj(vec![
                                                    ("name", json::s(w.scope.unwrap_or("unknown"))),
                                                    ("used_percent", number(w.used_percent)),
                                                    (
                                                        "remaining_percent",
                                                        number(w.remaining_percent),
                                                    ),
                                                    (
                                                        "reset_at",
                                                        w.reset_at
                                                            .map(|t| Value::Int(t as i64))
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                    (
                                                        "reset_text",
                                                        w.reset_text
                                                            .as_ref()
                                                            .map(json::s)
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                    ("reset_label", json::s(w.reset_label())),
                                                    (
                                                        "reset_date",
                                                        date.as_ref()
                                                            .map(json::s)
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                    (
                                                        "reset_time",
                                                        time.as_ref()
                                                            .map(json::s)
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                    (
                                                        "reset_timezone",
                                                        zone.as_ref()
                                                            .map(json::s)
                                                            .unwrap_or(Value::Null),
                                                    ),
                                                ])
                                            })
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    fn refresh_usage_panel(&self, cx: &mut Cx) {
        for (provider, id, island, recovery) in [
            (
                UsageProvider::Claude,
                id!(fable_usage),
                id!(fable_island),
                id!(fable_recover),
            ),
            (
                UsageProvider::Codex,
                id!(astra_usage),
                id!(astra_island),
                id!(astra_recover),
            ),
        ] {
            let limit = if self.usage_stalls.reports(provider).is_empty() {
                0.0f32
            } else {
                1.0f32
            };
            let mut group = self.ui.widget(cx, &[island]);
            script_apply_eval!(cx, group, {draw_bg +: {limit_reached: #(limit)}});
            group.redraw(cx);
            let p = self.usage_snapshot.provider(provider);
            let limits = p.map(|p| p.limits()).unwrap_or([None, None]);
            // LIMIT shares ProviderPercent's theme style but never receives a
            // quota color, so it remains the normal-color reference on recovery
            // below a threshold or when a value becomes unavailable.
            let normal_percent_color = self.ui.widget(cx, &[id, id!(usage_limit)])
                .borrow::<Label>().map(|label| label.draw_text.color);
            for (index, percent, reset) in [
                (0, id!(percent_session), id!(reset_session)),
                (1, id!(percent_week), id!(reset_week)),
            ] {
                if index == 0 && provider == UsageProvider::Codex {
                    continue;
                }
                let used = limits[index]
                    .and_then(|window| window.used_percent)
                    .filter(|value| value.is_finite());
                let value = used
                    .map(|value| format!("{value:.0}%"))
                    .unwrap_or_else(|| "—".into());
                let label = self.ui.label(cx, &[id, percent]);
                label.set_text(cx, &value);
                if let Some(normal) = normal_percent_color {
                    let dark_surface = normal.x * 0.2126 + normal.y * 0.7152 + normal.z * 0.0722 > 0.5;
                    let color = match used {
                        Some(value) if value >= 90.0 => if dark_surface { vec4(1.0, 0.39, 0.43, 1.0) } else { vec4(0.77, 0.12, 0.18, 1.0) },
                        Some(value) if value >= 80.0 => if dark_surface { vec4(0.98, 0.65, 0.25, 1.0) } else { vec4(0.70, 0.36, 0.04, 1.0) },
                        _ => normal,
                    };
                    label.set_text_color(cx, color);
                }
                let value = limits[index]
                    .map(|window| window.reset_brief())
                    .unwrap_or_else(|| "—".into());
                self.ui.label(cx, &[id, reset]).set_text(cx, &value);
            }
            // the quiet slot reads "refreshing…" while this provider's fetch
            // runs, else "stale" once the last observation is old
            let refreshing = self.usage_snapshot.polling == Some(provider);
            let stale = p.is_some_and(|p| p.observed_at > 0 && p.is_stale(disk::now()));
            let quiet = self.ui.label(cx, &[id, id!(usage_stale)]);
            quiet.set_text(cx, if refreshing { "refreshing…" } else { "stale" });
            quiet.set_visible(cx, refreshing || stale);
            self.ui
                .widget(cx, &[id, id!(usage_limit)])
                .set_visible(cx, limit > 0.0);
            for segment in [
                id!(provider_name),
                id!(usage_limit),
                id!(scope_session),
                id!(percent_session),
                id!(reset_session),
                id!(scope_week),
                id!(percent_week),
                id!(reset_week),
                id!(usage_stale),
            ] {
                if provider == UsageProvider::Codex
                    && [id!(scope_session), id!(percent_session), id!(reset_session)]
                        .contains(&segment)
                {
                    continue;
                }
                if let Some(mut label) = self.ui.widget(cx, &[id, segment]).borrow_mut::<Label>() {
                    label
                        .draw_text
                        .draw_vars
                        .set_dyn_instance(cx, id!(limit_reached), &[limit]);
                }
            }
            self.ui.widget(cx, &[id]).redraw(cx);
            if let Some(mut button) = self.ui.widget(cx, &[recovery]).borrow_mut::<Button>() {
                button
                    .draw_icon
                    .draw_vars
                    .set_dyn_instance(cx, id!(limit_reached), &[limit]);
            }
            self.ui
                .button(cx, &[recovery])
                .set_disabled(cx, self.recovery_lane(provider).is_none());
        }
        let status = if let Some(error) = &self.usage_error {
            format!("Usage unavailable: {error}")
        } else if let Some(provider) = self.usage_snapshot.polling {
            format!("Checking {}…", provider.label())
        } else {
            "Refreshes every 2 minutes · failed queries retry after 10 minutes".into()
        };
        self.ui
            .label(cx, ids!(usage_poll_status))
            .set_text(cx, &status);
        for (kind, label) in [
            (UsageProvider::Codex, id!(codex_usage_report)),
            (UsageProvider::Claude, id!(claude_usage_report)),
        ] {
            let text = self
                .usage_snapshot
                .provider(kind)
                .map(|p| {
                    let mut text = ["Session", "Week"]
                        .into_iter()
                        .zip(p.limits())
                        .filter(|(label, _)| kind == UsageProvider::Claude || *label == "Week")
                        .map(|(label, window)| match window {
                            Some(w) => format!(
                                "{label}: {} used · {}",
                                w.used_percent
                                    .map(|v| format!("{v:.0}%"))
                                    .unwrap_or_else(|| "Unknown".into()),
                                w.reset_label()
                            ),
                            None => format!("{label}: not reported"),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    text.push_str(&format!(
                        "\nAccount: {}",
                        p.account_email.as_deref().unwrap_or("unavailable")
                    ));
                    if p.observed_at > 0 {
                        let age = disk::now().saturating_sub(p.observed_at) / 60;
                        let freshness = if p.is_stale(disk::now()) {
                            "Stale"
                        } else {
                            "Updated"
                        };
                        text.push_str(&format!(
                            "\n\n{freshness} · {}",
                            if age == 0 {
                                "less than a minute ago".into()
                            } else {
                                format!("{age} min ago")
                            }
                        ));
                        if let Some(plan) = &p.plan {
                            text.push_str(&format!(" · {plan}"));
                        }
                    }
                    if let Some(error) = &p.error {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(error);
                    }
                    if text.is_empty() {
                        text = "Waiting for first query".into();
                    }
                    text
                })
                .unwrap_or_else(|| "Waiting for first query".into());
            self.ui.label(cx, &[label]).set_text(cx, &text);
        }
        self.refresh_usage_history(cx);
    }

    fn drain_usage(&mut self, cx: &mut Cx) {
        if let Some(snapshot) = self.usage_worker.as_mut().and_then(UsageWorker::poll) {
            self.usage_snapshot = snapshot;
            self.refresh_usage_panel(cx);
            self.refresh_workspace(cx);
        }
    }

    fn open_usage(&mut self, cx: &mut Cx) {
        self.show_utility(cx, id!(usage_tab), id!(UsageTab), "AI usage");
        self.refresh_usage_panel(cx);
    }
}
