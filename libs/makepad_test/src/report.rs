use crate::error::TestResult;
use makepad_micro_serde::*;
use makepad_studio_protocol::WidgetSnapshot;
use std::fmt::Write;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct StepEvidence {
    pub selector_query: Option<String>,
    pub before_widgets: Option<Vec<WidgetSnapshot>>,
    pub after_widgets: Option<Vec<WidgetSnapshot>>,
    pub screenshot_path: Option<String>,
    pub log_excerpt: Option<String>,
    pub widget_dump_excerpt: Option<String>,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct TraceStep {
    pub index: usize,
    pub kind: String,
    pub detail: String,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub status: String,
    pub error_message: Option<String>,
    pub evidence: StepEvidence,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct CaseReport {
    pub case_name: String,
    pub status: String,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub artifact_dir: String,
    pub failure_message: Option<String>,
    pub generated_case_path: Option<String>,
    pub steps: Vec<TraceStep>,
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct SuiteReport {
    pub suite_id: String,
    pub session_mode: String,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub status: String,
    pub failure_message: Option<String>,
    pub generated_case_path: Option<String>,
    pub cases: Vec<CaseReport>,
}

pub fn write_suite_outputs(suite_dir: &Path, report: &SuiteReport) -> TestResult<()> {
    fs::create_dir_all(suite_dir)?;
    fs::write(suite_dir.join("suite-report.json"), report.serialize_json())?;
    fs::write(
        suite_dir.join("index.html"),
        render_suite_report_html(report),
    )?;
    Ok(())
}

pub fn write_case_report(case_dir: &Path, report: &CaseReport) -> TestResult<()> {
    fs::create_dir_all(case_dir)?;
    fs::write(case_dir.join("case-report.json"), report.serialize_json())?;
    Ok(())
}

pub fn render_suite_report_html(report: &SuiteReport) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    out.push_str("<title>makepad_test suite report</title>");
    out.push_str(
        "<style>\
        body{font-family:ui-sans-serif,system-ui,sans-serif;margin:24px;background:#101318;color:#e8eef6}\
        h1,h2{margin:0 0 12px}\
        .meta,.failure{margin:12px 0;padding:12px;border-radius:8px;background:#171c24}\
        .case{margin:16px 0;padding:16px;border-radius:10px;background:#171c24}\
        .ok{color:#7ee787}.failed{color:#ff7b72}\
        table{width:100%;border-collapse:collapse;margin-top:12px}\
        th,td{text-align:left;padding:8px;border-bottom:1px solid #283142;vertical-align:top}\
        code,pre{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
        pre{white-space:pre-wrap;background:#0d1117;padding:10px;border-radius:8px}\
        a{color:#79c0ff}\
        </style></head><body>",
    );
    let _ = write!(
        &mut out,
        "<h1>Suite {}</h1><div class=\"meta\"><strong>Status:</strong> <span class=\"{}\">{}</span><br>\
         <strong>Session mode:</strong> {}<br><strong>Duration:</strong> {} ms<br>\
         <strong>Cases:</strong> {}</div>",
        html_escape(&report.suite_id),
        status_class(&report.status),
        html_escape(&report.status),
        html_escape(&report.session_mode),
        report.duration_ms,
        report.cases.len()
    );
    if let Some(message) = &report.failure_message {
        let _ = write!(
            &mut out,
            "<div class=\"failure\"><strong>Failure:</strong><pre>{}</pre></div>",
            html_escape(message)
        );
    }
    if let Some(path) = &report.generated_case_path {
        let _ = write!(
            &mut out,
            "<div class=\"meta\"><strong>Generated Splash:</strong> <a href=\"{}\">{}</a></div>",
            html_escape(path),
            html_escape(path)
        );
    }
    for case in &report.cases {
        let _ = write!(
            &mut out,
            "<section class=\"case\"><h2>{}</h2>\
             <div><strong>Status:</strong> <span class=\"{}\">{}</span> \
             <strong>Duration:</strong> {} ms \
             <strong>Artifacts:</strong> <code>{}</code></div>",
            html_escape(&case.case_name),
            status_class(&case.status),
            html_escape(&case.status),
            case.duration_ms,
            html_escape(&case.artifact_dir)
        );
        if let Some(message) = &case.failure_message {
            let _ = write!(
                &mut out,
                "<div class=\"failure\"><strong>Failure:</strong><pre>{}</pre></div>",
                html_escape(message)
            );
        }
        if let Some(path) = &case.generated_case_path {
            let _ = write!(
                &mut out,
                "<div><strong>Generated Splash:</strong> <a href=\"{}\">{}</a></div>",
                html_escape(path),
                html_escape(path)
            );
        }
        out.push_str("<table><thead><tr><th>#</th><th>Kind</th><th>Detail</th><th>Status</th><th>Duration</th><th>Evidence</th></tr></thead><tbody>");
        for step in &case.steps {
            let _ = write!(
                &mut out,
                "<tr><td>{}</td><td>{}</td><td><code>{}</code></td><td class=\"{}\">{}</td><td>{} ms</td><td>{}</td></tr>",
                step.index,
                html_escape(&step.kind),
                html_escape(&step.detail),
                status_class(&step.status),
                html_escape(&step.status),
                step.duration_ms,
                render_step_evidence(step)
            );
        }
        out.push_str("</tbody></table></section>");
    }
    out.push_str("</body></html>");
    out
}

fn render_step_evidence(step: &TraceStep) -> String {
    let mut out = String::new();
    if let Some(message) = &step.error_message {
        let _ = write!(
            &mut out,
            "<div><strong>Error:</strong> {}</div>",
            html_escape(message)
        );
    }
    if let Some(selector) = &step.evidence.selector_query {
        let _ = write!(
            &mut out,
            "<div><strong>Selector:</strong> <code>{}</code></div>",
            html_escape(selector)
        );
    }
    if let Some(path) = &step.evidence.screenshot_path {
        let _ = write!(
            &mut out,
            "<div><strong>Screenshot:</strong> <a href=\"{}\">{}</a></div>",
            html_escape(path),
            html_escape(path)
        );
    }
    if let Some(log_excerpt) = &step.evidence.log_excerpt {
        let _ = write!(
            &mut out,
            "<details><summary>Logs</summary><pre>{}</pre></details>",
            html_escape(log_excerpt)
        );
    }
    if let Some(dump_excerpt) = &step.evidence.widget_dump_excerpt {
        let _ = write!(
            &mut out,
            "<details><summary>Widget dump</summary><pre>{}</pre></details>",
            html_escape(dump_excerpt)
        );
    }
    if step.evidence.before_widgets.is_some() || step.evidence.after_widgets.is_some() {
        let _ = write!(
            &mut out,
            "<div><strong>Snapshots:</strong> before={} after={}</div>",
            step.evidence
                .before_widgets
                .as_ref()
                .map_or(0, |items| items.len()),
            step.evidence
                .after_widgets
                .as_ref()
                .map_or(0, |items| items.len())
        );
    }
    out
}

fn status_class(status: &str) -> &'static str {
    if status.eq_ignore_ascii_case("passed") {
        "ok"
    } else if status.eq_ignore_ascii_case("failed") {
        "failed"
    } else {
        ""
    }
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{write_case_report, write_suite_outputs, CaseReport, SuiteReport, TraceStep};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("makepad_test_report_{prefix}_{unique}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn writes_suite_and_case_reports() {
        let root = temp_dir("write_reports");
        let case_dir = root.join("cases/smoke");
        let case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: case_dir.to_string_lossy().to_string(),
            steps: vec![TraceStep {
                index: 1,
                kind: "click".to_string(),
                detail: "{id: \"submit\"}".to_string(),
                status: "passed".to_string(),
                ..TraceStep::default()
            }],
            ..CaseReport::default()
        };
        let suite = SuiteReport {
            suite_id: "suite".to_string(),
            session_mode: "isolated".to_string(),
            status: "passed".to_string(),
            cases: vec![case.clone()],
            ..SuiteReport::default()
        };

        write_case_report(&case_dir, &case).unwrap();
        write_suite_outputs(&root, &suite).unwrap();

        assert!(case_dir.join("case-report.json").exists());
        assert!(root.join("suite-report.json").exists());
        assert!(root.join("index.html").exists());
    }
}
