use crate::error::TestResult;
use makepad_micro_serde::*;
use makepad_studio_protocol::WidgetSnapshot;
use std::fmt::Write;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

const APNG_FRAME_DELAY_MS: u16 = 400;

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
    pub session_apng_path: Option<String>,
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

pub(crate) fn write_case_outputs(
    case_dir: &Path,
    report: &mut CaseReport,
    frame_paths: &[PathBuf],
) -> TestResult<()> {
    fs::create_dir_all(case_dir)?;
    report.session_apng_path = build_case_apng(case_dir, frame_paths).ok().flatten();
    let result = write_case_report(case_dir, report);
    let _ = fs::remove_dir_all(case_dir.join(".frames"));
    result
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
        .preview{margin:12px 0;padding:12px;border-radius:8px;background:#0d1117}\
        .preview img{display:block;max-width:min(960px,100%);height:auto;margin-top:12px;border-radius:8px;border:1px solid #283142}\
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
        if let Some(path) = &case.session_apng_path {
            let _ = write!(
                &mut out,
                "<div class=\"preview\"><strong>Session APNG:</strong> <a href=\"{}\">{}</a><img src=\"{}\" alt=\"{}\"></div>",
                html_escape(path),
                html_escape(path),
                html_escape(path),
                html_escape(&format!("{} session animation", case.case_name))
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

fn build_case_apng(case_dir: &Path, frame_paths: &[PathBuf]) -> TestResult<Option<String>> {
    let mut frames: Vec<RgbaFrame> = Vec::new();
    for path in frame_paths {
        let Ok(frame) = decode_rgba_frame(&path) else {
            continue;
        };
        if let Some(first) = frames.first() {
            if first.width != frame.width || first.height != frame.height {
                continue;
            }
        }
        frames.push(frame);
    }
    if frames.len() < 2 {
        return Ok(None);
    }

    let output_path = case_dir.join("session.png");
    encode_apng(&output_path, &frames)?;
    Ok(Some(output_path.to_string_lossy().to_string()))
}

fn decode_rgba_frame(path: &Path) -> TestResult<RgbaFrame> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    let mut reader = decoder.read_info().map_err(|err| {
        crate::TestError::new(format!("failed to decode PNG {}: {err}", path.display()))
    })?;
    let output_buffer_size = reader.output_buffer_size().ok_or_else(|| {
        crate::TestError::new(format!(
            "failed to determine PNG output buffer size for {}",
            path.display()
        ))
    })?;
    let mut buffer = vec![0; output_buffer_size];
    let frame = reader.next_frame(&mut buffer).map_err(|err| {
        crate::TestError::new(format!(
            "failed to read PNG frame {}: {err}",
            path.display()
        ))
    })?;
    let bytes = &buffer[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => rgb_to_rgba(bytes),
        png::ColorType::GrayscaleAlpha => grayscale_alpha_to_rgba(bytes),
        png::ColorType::Grayscale => grayscale_to_rgba(bytes),
        png::ColorType::Indexed => {
            return Err(crate::TestError::new(format!(
                "unsupported indexed PNG frame {}",
                path.display()
            )))
        }
    };
    Ok(RgbaFrame {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

fn encode_apng(output_path: &Path, frames: &[RgbaFrame]) -> TestResult<()> {
    let file = File::create(output_path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, frames[0].width, frames[0].height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .set_animated(frames.len() as u32, 0)
        .map_err(|err| {
            crate::TestError::new(format!("failed to configure APNG frame count: {err}"))
        })?;
    encoder.set_sep_def_img(false).map_err(|err| {
        crate::TestError::new(format!("failed to configure APNG default image: {err}"))
    })?;
    let mut writer = encoder
        .write_header()
        .map_err(|err| crate::TestError::new(format!("failed to write APNG header: {err}")))?;
    writer
        .set_frame_delay(APNG_FRAME_DELAY_MS, 1_000)
        .map_err(|err| crate::TestError::new(format!("failed to set APNG frame delay: {err}")))?;
    for frame in frames {
        writer
            .write_image_data(&frame.rgba)
            .map_err(|err| crate::TestError::new(format!("failed to write APNG frame: {err}")))?;
    }
    Ok(())
}

fn rgb_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 3 * 4);
    for chunk in bytes.chunks_exact(3) {
        out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
    }
    out
}

fn grayscale_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 4);
    for &gray in bytes {
        out.extend_from_slice(&[gray, gray, gray, 255]);
    }
    out
}

fn grayscale_alpha_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 2 * 4);
    for chunk in bytes.chunks_exact(2) {
        out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
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

#[derive(Clone, Debug)]
struct RgbaFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::{
        render_suite_report_html, write_case_outputs, write_suite_outputs, CaseReport,
        StepEvidence, SuiteReport, TraceStep,
    };
    use std::fs;
    use std::fs::File;
    use std::io::BufWriter;
    use std::path::{Path, PathBuf};
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
        let mut case = CaseReport {
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

        write_case_outputs(&case_dir, &mut case, &[]).unwrap();

        let suite = SuiteReport {
            suite_id: "suite".to_string(),
            session_mode: "isolated".to_string(),
            status: "passed".to_string(),
            cases: vec![case],
            ..SuiteReport::default()
        };
        write_suite_outputs(&root, &suite).unwrap();

        assert!(case_dir.join("case-report.json").exists());
        assert!(root.join("suite-report.json").exists());
        assert!(root.join("index.html").exists());
    }

    #[test]
    fn writes_session_apng_when_multiple_frames_exist() {
        let root = temp_dir("apng_multi");
        let case_dir = root.join("cases/smoke");
        let steps_dir = case_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();
        let frame_a = steps_dir.join("001-click.png");
        let frame_b = steps_dir.join("002-fill.png");
        write_png(&frame_a, 2, 2, &[255, 0, 0, 255].repeat(4));
        write_png(&frame_b, 2, 2, &[0, 255, 0, 255].repeat(4));

        let mut case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: case_dir.to_string_lossy().to_string(),
            steps: vec![
                step_with_screenshot(1, &frame_a),
                step_with_screenshot(2, &frame_b),
            ],
            ..CaseReport::default()
        };

        write_case_outputs(&case_dir, &mut case, &[frame_a.clone(), frame_b.clone()]).unwrap();

        let session_path = case_dir.join("session.png");
        let expected = session_path.to_string_lossy().to_string();
        assert_eq!(case.session_apng_path.as_deref(), Some(expected.as_str()));
        let bytes = fs::read(&session_path).unwrap();
        assert!(session_path.exists());
        assert!(bytes.windows(4).any(|chunk| chunk == b"acTL"));
    }

    #[test]
    fn skips_bad_or_mismatched_frames_when_building_apng() {
        let root = temp_dir("apng_mixed");
        let case_dir = root.join("cases/smoke");
        let steps_dir = case_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();
        let frame_a = steps_dir.join("001-click.png");
        let frame_b = steps_dir.join("002-fill.png");
        let frame_bad = steps_dir.join("003-bad.png");
        let frame_c = steps_dir.join("004-type.png");
        write_png(&frame_a, 2, 2, &[255, 0, 0, 255].repeat(4));
        write_png(&frame_b, 2, 2, &[0, 255, 0, 255].repeat(4));
        fs::write(&frame_bad, b"not a png").unwrap();
        write_png(&frame_c, 3, 3, &[0, 0, 255, 255].repeat(9));

        let mut case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: case_dir.to_string_lossy().to_string(),
            steps: vec![
                step_with_screenshot(1, &frame_a),
                step_with_screenshot(2, &frame_b),
                step_with_screenshot(3, &frame_bad),
                step_with_screenshot(4, &frame_c),
            ],
            ..CaseReport::default()
        };

        write_case_outputs(
            &case_dir,
            &mut case,
            &[
                frame_a.clone(),
                frame_b.clone(),
                frame_bad.clone(),
                frame_c.clone(),
            ],
        )
        .unwrap();

        assert!(case_dir.join("session.png").exists());
        assert!(case.session_apng_path.is_some());
    }

    #[test]
    fn omits_session_apng_when_fewer_than_two_valid_frames_exist() {
        let root = temp_dir("apng_single");
        let case_dir = root.join("cases/smoke");
        let steps_dir = case_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();
        let frame_a = steps_dir.join("001-click.png");
        let frame_bad = steps_dir.join("002-bad.png");
        write_png(&frame_a, 2, 2, &[255, 0, 0, 255].repeat(4));
        fs::write(&frame_bad, b"not a png").unwrap();

        let mut case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: case_dir.to_string_lossy().to_string(),
            steps: vec![
                step_with_screenshot(1, &frame_a),
                step_with_screenshot(2, &frame_bad),
            ],
            ..CaseReport::default()
        };

        write_case_outputs(&case_dir, &mut case, &[frame_a.clone(), frame_bad.clone()]).unwrap();

        assert!(!case_dir.join("session.png").exists());
        assert!(case.session_apng_path.is_none());
    }

    #[test]
    fn builds_apng_from_transient_frames_and_cleans_them_up() {
        let root = temp_dir("apng_transient");
        let case_dir = root.join("cases/smoke");
        let frames_dir = case_dir.join(".frames");
        fs::create_dir_all(&frames_dir).unwrap();
        let frame_a = frames_dir.join("001-click.png");
        let frame_b = frames_dir.join("002-fill.png");
        write_png(&frame_a, 2, 2, &[255, 0, 0, 255].repeat(4));
        write_png(&frame_b, 2, 2, &[0, 255, 0, 255].repeat(4));

        let mut case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: case_dir.to_string_lossy().to_string(),
            steps: vec![
                TraceStep {
                    index: 1,
                    kind: "click".to_string(),
                    detail: "click".to_string(),
                    ..TraceStep::default()
                },
                TraceStep {
                    index: 2,
                    kind: "fill".to_string(),
                    detail: "fill".to_string(),
                    ..TraceStep::default()
                },
            ],
            ..CaseReport::default()
        };

        write_case_outputs(&case_dir, &mut case, &[frame_a.clone(), frame_b.clone()]).unwrap();

        assert!(case_dir.join("session.png").exists());
        assert!(!frames_dir.exists());
        assert!(!case_dir.join("steps").exists());
        assert!(case
            .steps
            .iter()
            .all(|step| step.evidence.screenshot_path.is_none()));
    }

    #[test]
    fn renders_session_apng_in_html() {
        let case = CaseReport {
            case_name: "smoke".to_string(),
            status: "passed".to_string(),
            artifact_dir: "/tmp/cases/smoke".to_string(),
            session_apng_path: Some("/tmp/cases/smoke/session.png".to_string()),
            steps: vec![TraceStep {
                index: 1,
                kind: "snapshot".to_string(),
                detail: "snapshot".to_string(),
                ..TraceStep::default()
            }],
            ..CaseReport::default()
        };
        let suite = SuiteReport {
            suite_id: "suite".to_string(),
            session_mode: "isolated".to_string(),
            status: "passed".to_string(),
            cases: vec![case],
            ..SuiteReport::default()
        };

        let html = render_suite_report_html(&suite);
        assert!(html.contains("Session APNG"));
        assert!(html.contains("/tmp/cases/smoke/session.png"));
        assert!(html.contains("<img"));
    }

    fn step_with_screenshot(index: usize, path: &Path) -> TraceStep {
        TraceStep {
            index,
            kind: "step".to_string(),
            detail: "step".to_string(),
            evidence: StepEvidence {
                screenshot_path: Some(path.to_string_lossy().to_string()),
                ..StepEvidence::default()
            },
            ..TraceStep::default()
        }
    }

    fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
    }
}
