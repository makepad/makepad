// The status bar, utility panel and F10 tools read one worker snapshot.
impl App {
    fn usage_brief(&self, kind: UsageProvider) -> String {
        let p = self.usage_snapshot.provider(kind);
        let limits = p.map(|p| p.limits()).unwrap_or([None, None]);
        let percent = |i: usize| {
            limits[i]
                .and_then(|w| w.used_percent)
                .map(|v| format!("{v:.0}%"))
                .unwrap_or_else(|| "—".into())
        };
        let stale = p.is_some_and(|p| p.observed_at > 0 && p.is_stale(disk::now()));
        let suffix = if stale { " stale" } else { "" };
        let reset = |i: usize| {
            limits[i]
                .map(|w| w.reset_brief())
                .unwrap_or_else(|| "—".into())
        };
        match kind {
            UsageProvider::Claude => format!(
                "Fable  S {} · W {}{suffix}\nReset {} · {}",
                percent(0),
                percent(1),
                reset(0),
                reset(1)
            ),
            UsageProvider::Codex => format!("Astra  W {}{suffix}\nReset {}", percent(1), reset(1)),
        }
    }
    fn usage_json(&self) -> Value {
        let number = |value: Option<f64>| value.map(Value::F64).unwrap_or(Value::Null);
        json::obj(vec![
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
        self.ui
            .button(cx, ids!(fable_usage))
            .set_text(cx, &self.usage_brief(UsageProvider::Claude));
        self.ui
            .button(cx, ids!(astra_usage))
            .set_text(cx, &self.usage_brief(UsageProvider::Codex));
        let polling = self.usage_snapshot.polling.is_some();
        self.ui
            .button(cx, ids!(refresh_usage_status))
            .set_disabled(cx, polling);
        self.ui
            .button(cx, ids!(refresh_usage))
            .set_disabled(cx, polling);
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
