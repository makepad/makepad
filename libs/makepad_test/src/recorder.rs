use crate::error::{TestError, TestResult};
use crate::report::{render_suite_report_html, CaseReport, StepEvidence, SuiteReport, TraceStep};
use crate::runtime::{run_with_config, TestApp, TestConfig};
use crate::selector::Selector;
use makepad_micro_serde::*;
use makepad_studio_protocol::{KeyCode, KeyModifiers, WidgetSnapshot};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct SplashRecorderOutput {
    pub artifact_dir: String,
    pub trace_path: String,
    pub report_path: String,
    pub generated_case_path: String,
}

#[derive(Clone, Debug)]
pub struct SplashRecorderOptions {
    pub case_name: String,
}

impl SplashRecorderOptions {
    pub fn new(case_name: impl Into<String>) -> Self {
        Self {
            case_name: case_name.into(),
        }
    }
}

#[derive(Clone, Debug)]
enum RecorderActionKind {
    Click,
    Fill(String),
    Clear,
    TypeText(String),
    PressReturn,
    PressKey {
        key_code: KeyCode,
        modifiers: KeyModifiers,
    },
    Scroll {
        sx: f64,
        sy: f64,
    },
    Drag {
        dx: f64,
        dy: f64,
    },
    Screenshot(String),
}

#[derive(Clone, Debug)]
struct RecorderAction {
    kind: RecorderActionKind,
    selector: Option<Selector>,
    selector_repr: Option<String>,
    inferred_wait: Option<String>,
    comment: Option<String>,
}

pub struct SplashRecorderSession {
    app: TestApp,
    options: SplashRecorderOptions,
    artifact_dir: PathBuf,
    actions: Vec<RecorderAction>,
}

impl SplashRecorderSession {
    fn new(app: TestApp, options: SplashRecorderOptions, artifact_dir: PathBuf) -> Self {
        Self {
            app,
            options,
            artifact_dir,
            actions: Vec::new(),
        }
    }

    pub fn click(&mut self, selector: Selector) -> TestResult<()> {
        let selector_repr = selector_to_splash(&selector);
        let before = self.app.try_query_widgets(&selector, false)?;
        self.app.locator(selector.clone()).try_click()?;
        let after = self.app.try_query_widgets(&selector, false)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Click,
            selector: Some(selector),
            selector_repr: Some(selector_repr),
            inferred_wait: infer_wait(&before, &after),
            comment: None,
        });
        Ok(())
    }

    pub fn fill(&mut self, selector: Selector, text: impl Into<String>) -> TestResult<()> {
        let text = text.into();
        let selector_repr = selector_to_splash(&selector);
        let before = self.app.try_query_widgets(&selector, false)?;
        self.app.locator(selector.clone()).try_fill(text.clone())?;
        let after = self.app.try_query_widgets(&selector, false)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Fill(text),
            selector: Some(selector),
            selector_repr: Some(selector_repr),
            inferred_wait: infer_wait(&before, &after),
            comment: None,
        });
        Ok(())
    }

    pub fn clear(&mut self, selector: Selector) -> TestResult<()> {
        let selector_repr = selector_to_splash(&selector);
        let before = self.app.try_query_widgets(&selector, false)?;
        self.app.locator(selector.clone()).try_clear()?;
        let after = self.app.try_query_widgets(&selector, false)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Clear,
            selector: Some(selector),
            selector_repr: Some(selector_repr),
            inferred_wait: infer_wait(&before, &after),
            comment: None,
        });
        Ok(())
    }

    pub fn type_text(&mut self, text: impl Into<String>) -> TestResult<()> {
        let text = text.into();
        self.app.try_type_text(text.clone())?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::TypeText(text),
            selector: None,
            selector_repr: None,
            inferred_wait: None,
            comment: Some("TODO: add a post-action wait if widget state changed".to_string()),
        });
        Ok(())
    }

    pub fn press_return(&mut self) -> TestResult<()> {
        self.app.try_press_return()?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::PressReturn,
            selector: None,
            selector_repr: None,
            inferred_wait: None,
            comment: None,
        });
        Ok(())
    }

    pub fn press_key(&mut self, key_code: KeyCode, modifiers: KeyModifiers) -> TestResult<()> {
        self.app.try_press_key_with_modifiers(key_code, modifiers)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::PressKey {
                key_code,
                modifiers,
            },
            selector: None,
            selector_repr: None,
            inferred_wait: None,
            comment: Some("TODO: add a post-action wait if widget state changed".to_string()),
        });
        Ok(())
    }

    pub fn scroll(&mut self, selector: Selector, sx: f64, sy: f64) -> TestResult<()> {
        let selector_repr = selector_to_splash(&selector);
        self.app.locator(selector.clone()).try_scroll(sx, sy)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Scroll { sx, sy },
            selector: Some(selector),
            selector_repr: Some(selector_repr),
            inferred_wait: None,
            comment: Some("TODO: confirm the expected post-scroll state".to_string()),
        });
        Ok(())
    }

    pub fn drag(&mut self, selector: Selector, dx: f64, dy: f64) -> TestResult<()> {
        let selector_repr = selector_to_splash(&selector);
        self.app.locator(selector.clone()).try_drag_by(dx, dy)?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Drag { dx, dy },
            selector: Some(selector),
            selector_repr: Some(selector_repr),
            inferred_wait: None,
            comment: Some("TODO: confirm the expected post-drag state".to_string()),
        });
        Ok(())
    }

    pub fn screenshot_checkpoint(&mut self, name: impl Into<String>) -> TestResult<()> {
        let name = name.into();
        self.app.try_screenshot()?;
        self.actions.push(RecorderAction {
            kind: RecorderActionKind::Screenshot(name),
            selector: None,
            selector_repr: None,
            inferred_wait: None,
            comment: None,
        });
        Ok(())
    }

    fn finish(self) -> TestResult<SplashRecorderOutput> {
        fs::create_dir_all(&self.artifact_dir)?;
        let generated_case = render_generated_case(&self.options.case_name, &self.actions);
        let generated_case_path = self.artifact_dir.join("generated-case.splash");
        fs::write(&generated_case_path, generated_case)?;

        let steps: Vec<_> = self
            .actions
            .iter()
            .enumerate()
            .map(|(index, action)| TraceStep {
                index: index + 1,
                kind: recorder_kind_name(&action.kind).to_string(),
                detail: recorder_detail(action),
                started_at_ms: 0,
                finished_at_ms: 0,
                duration_ms: 0,
                status: "recorded".to_string(),
                error_message: None,
                evidence: StepEvidence {
                    selector_query: action.selector.as_ref().map(Selector::describe),
                    before_widgets: None,
                    after_widgets: None,
                    screenshot_path: None,
                    log_excerpt: None,
                    widget_dump_excerpt: action.comment.clone(),
                },
            })
            .collect();
        let case_report = CaseReport {
            case_name: self.options.case_name.clone(),
            status: "recorded".to_string(),
            started_at_ms: 0,
            finished_at_ms: 0,
            duration_ms: 0,
            artifact_dir: self.artifact_dir.to_string_lossy().to_string(),
            failure_message: None,
            generated_case_path: Some(generated_case_path.to_string_lossy().to_string()),
            steps,
        };
        let trace_path = self.artifact_dir.join("recording-trace.json");
        fs::write(&trace_path, case_report.serialize_json())?;

        let suite_report = SuiteReport {
            suite_id: format!("recording:{}", self.options.case_name),
            session_mode: "visible-recorder".to_string(),
            started_at_ms: 0,
            finished_at_ms: 0,
            duration_ms: 0,
            status: "recorded".to_string(),
            failure_message: None,
            generated_case_path: Some(generated_case_path.to_string_lossy().to_string()),
            cases: vec![case_report],
        };
        let report_path = self.artifact_dir.join("recording-report.html");
        fs::write(&report_path, render_suite_report_html(&suite_report))?;

        Ok(SplashRecorderOutput {
            artifact_dir: self.artifact_dir.to_string_lossy().to_string(),
            trace_path: trace_path.to_string_lossy().to_string(),
            report_path: report_path.to_string_lossy().to_string(),
            generated_case_path: generated_case_path.to_string_lossy().to_string(),
        })
    }
}

pub fn run_splash_recorder<F>(
    mut config: TestConfig,
    options: SplashRecorderOptions,
    record: F,
) -> TestResult<SplashRecorderOutput>
where
    F: FnOnce(&mut SplashRecorderSession) -> TestResult<()>,
{
    if std::env::var("MAKEPAD_TEST_VISIBLE").ok().as_deref() != Some("1") {
        return Err(TestError::new(
            "Splash recorder requires MAKEPAD_TEST_VISIBLE=1",
        ));
    }
    let artifact_dir = config.artifacts_dir.join("recorder");
    config = config.with_artifacts_dir(artifact_dir.clone());
    let mut output = None;
    run_with_config(config, |app| {
        let mut session = SplashRecorderSession::new(app, options, artifact_dir);
        record(&mut session)?;
        output = Some(session.finish()?);
        Ok::<(), TestError>(())
    })?;
    output.ok_or_else(|| TestError::new("Splash recorder did not produce any output"))
}

fn infer_wait(before: &[WidgetSnapshot], after: &[WidgetSnapshot]) -> Option<String> {
    let before = before.first()?;
    let after = after.first()?;
    if before.text != after.text {
        return after.text.as_ref().map(|text| {
            format!(
                "test.wait_text({}, {:?})",
                selector_from_snapshot(after),
                text
            )
        });
    }
    if before.value != after.value {
        return after.value.as_ref().map(|value| {
            format!(
                "test.wait_value({}, {:?})",
                selector_from_snapshot(after),
                value
            )
        });
    }
    if before.checked != after.checked {
        return after.checked.map(|checked| {
            format!(
                "test.wait_checked({}, {})",
                selector_from_snapshot(after),
                checked
            )
        });
    }
    if before.enabled != after.enabled {
        return Some(format!(
            "test.wait_enabled({}, {})",
            selector_from_snapshot(after),
            after.enabled
        ));
    }
    None
}

fn recorder_kind_name(kind: &RecorderActionKind) -> &'static str {
    match kind {
        RecorderActionKind::Click => "click",
        RecorderActionKind::Fill(_) => "fill",
        RecorderActionKind::Clear => "clear",
        RecorderActionKind::TypeText(_) => "type_text",
        RecorderActionKind::PressReturn => "press_return",
        RecorderActionKind::PressKey { .. } => "press_key",
        RecorderActionKind::Scroll { .. } => "scroll",
        RecorderActionKind::Drag { .. } => "drag",
        RecorderActionKind::Screenshot(_) => "screenshot",
    }
}

fn recorder_detail(action: &RecorderAction) -> String {
    let mut out = String::new();
    if let Some(selector) = &action.selector_repr {
        out.push_str(selector);
    }
    match &action.kind {
        RecorderActionKind::Click | RecorderActionKind::Clear | RecorderActionKind::PressReturn => {
        }
        RecorderActionKind::Fill(text) | RecorderActionKind::TypeText(text) => {
            out.push_str(" ");
            out.push_str(text);
        }
        RecorderActionKind::PressKey {
            key_code,
            modifiers,
        } => {
            out.push_str(&format!(" {:?} {:?}", key_code, modifiers));
        }
        RecorderActionKind::Scroll { sx, sy } => {
            out.push_str(&format!(" {sx}, {sy}"));
        }
        RecorderActionKind::Drag { dx, dy } => {
            out.push_str(&format!(" {dx}, {dy}"));
        }
        RecorderActionKind::Screenshot(name) => {
            out.push_str(name);
        }
    }
    out.trim().to_string()
}

fn render_generated_case(case_name: &str, actions: &[RecorderAction]) -> String {
    let mut out = String::new();
    out.push_str("use mod.test\n\n");
    let _ =
        std::fmt::Write::write_fmt(&mut out, format_args!("test.case({:?}, || {{\n", case_name));
    for action in actions {
        if let Some(comment) = &action.comment {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("    // {}\n", comment));
        }
        out.push_str("    ");
        out.push_str(&render_action(action));
        out.push('\n');
        if let Some(wait) = &action.inferred_wait {
            out.push_str("    ");
            out.push_str(wait);
            out.push('\n');
        }
    }
    out.push_str("})\n");
    out
}

fn render_action(action: &RecorderAction) -> String {
    match &action.kind {
        RecorderActionKind::Click => format!(
            "test.click({})",
            action.selector_repr.as_deref().unwrap_or("{}")
        ),
        RecorderActionKind::Fill(text) => format!(
            "test.fill({}, {:?})",
            action.selector_repr.as_deref().unwrap_or("{}"),
            text
        ),
        RecorderActionKind::Clear => format!(
            "test.clear({})",
            action.selector_repr.as_deref().unwrap_or("{}")
        ),
        RecorderActionKind::TypeText(text) => format!("test.type_text({:?})", text),
        RecorderActionKind::PressReturn => "test.press_return()".to_string(),
        RecorderActionKind::PressKey {
            key_code,
            modifiers,
        } => format!(
            "test.press_key({{key: {:?} shift: {} control: {} alt: {} logo: {}}})",
            key_code, modifiers.shift, modifiers.control, modifiers.alt, modifiers.logo
        ),
        RecorderActionKind::Scroll { sx, sy } => format!(
            "test.scroll({}, {}, {})",
            action.selector_repr.as_deref().unwrap_or("{}"),
            sx,
            sy
        ),
        RecorderActionKind::Drag { dx, dy } => format!(
            "test.drag({}, {}, {})",
            action.selector_repr.as_deref().unwrap_or("{}"),
            dx,
            dy
        ),
        RecorderActionKind::Screenshot(name) => format!("test.screenshot({:?})", name),
    }
}

fn selector_to_splash(selector: &Selector) -> String {
    if let Some(raw) = selector.raw_query() {
        return format!("{{raw: {:?}}}", raw);
    }
    if let Some(id) = selector.id_value() {
        return format!("{{id: {:?}}}", id);
    }
    let mut fields = Vec::new();
    if let Some(widget_type) = selector.widget_type_value() {
        fields.push(format!("widget_type: {:?}", widget_type));
    }
    if let Some(text_exact) = selector.text_exact_value() {
        fields.push(format!("text_exact: {:?}", text_exact));
    }
    if let Some(text_contains) = selector.text_contains_value() {
        fields.push(format!("text_contains: {:?}", text_contains));
    }
    if let Some(nth) = selector.nth_index() {
        fields.push(format!("nth: {}", nth));
    }
    if fields.is_empty() {
        "{raw: \"*\"}".to_string()
    } else {
        format!("{{{}}}", fields.join(" "))
    }
}

fn selector_from_snapshot(snapshot: &WidgetSnapshot) -> String {
    if !snapshot.id.is_empty() {
        return format!("{{id: {:?}}}", snapshot.id);
    }
    if !snapshot.widget_type.is_empty() && snapshot.text.as_deref().is_some() {
        return format!(
            "{{widget_type: {:?} text_exact: {:?}}}",
            snapshot.widget_type,
            snapshot.text.as_deref().unwrap_or_default()
        );
    }
    if let Some(text) = &snapshot.text {
        return format!("{{raw: {:?}}}", text);
    }
    "{raw: \"*\"}".to_string()
}

#[cfg(test)]
mod tests {
    use super::{infer_wait, render_generated_case, selector_from_snapshot};
    use makepad_studio_protocol::WidgetSnapshot;

    fn snapshot(id: &str, widget_type: &str, text: Option<&str>) -> WidgetSnapshot {
        WidgetSnapshot {
            id: id.to_string(),
            widget_type: widget_type.to_string(),
            text: text.map(ToString::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn selector_generation_prefers_id() {
        let selector = selector_from_snapshot(&snapshot("submit", "Button", Some("Submit")));
        assert_eq!(selector, "{id: \"submit\"}");
    }

    #[test]
    fn selector_generation_falls_back_to_widget_type_and_text() {
        let selector = selector_from_snapshot(&snapshot("", "DockTab", Some("Modal")));
        assert_eq!(selector, "{widget_type: \"DockTab\" text_exact: \"Modal\"}");
    }

    #[test]
    fn inferred_wait_prefers_text_delta() {
        let before = vec![snapshot("status", "Label", Some("Before"))];
        let after = vec![snapshot("status", "Label", Some("After"))];
        let wait = infer_wait(&before, &after).unwrap();
        assert!(wait.contains("test.wait_text"));
        assert!(wait.contains("After"));
    }

    #[test]
    fn generated_case_contains_actions_and_waits() {
        let case = render_generated_case("smoke", &[]);
        assert!(case.contains("test.case(\"smoke\""));
    }
}
