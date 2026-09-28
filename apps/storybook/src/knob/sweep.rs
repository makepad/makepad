//! The Knob cost page's unattended sweep ("Run everything"): the data it
//! walks, the records it keeps, and the JSON it writes.
//!
//! The sweep compiles every detail level once, cold, then for each material
//! and style below shows every version side by side, holds still for a
//! screenshot, and measures each version's frame cost on that look. The
//! page (`stories/knob_cost.rs`) runs it; this module is its data.
use super::lod::{launch_salt, VERSIONS};
use super::presets::{material_index, style_index, MATERIALS, STYLES};

/// The materials the sweep shows, by preset name.
pub const SWEEP_MATERIALS: &[&str] = &["Neumorphic dark", "Neumorphic", "Glossy", "Milled", "Chrome", "Porcelain"];
/// And the styles.
pub const SWEEP_STYLES: &[&str] = &["classic", "knurled", "winged", "scalloped", "cutwing", "skirted"];

/// How long a look holds still for its screenshot, once it has drawn.
pub const HOLD_SECONDS: f64 = 1.2;
/// A measuring run's length in the sweep (the page's own runs take 2 s).
pub const RUN_SECONDS: f64 = 1.5;

/// Whether `MAKEPAD_KNOB_COST_SWEEP=1` asks the page to run the sweep when
/// it opens.
pub fn auto_sweep() -> bool {
    matches!(std::env::var("MAKEPAD_KNOB_COST_SWEEP").as_deref(), Ok("1") | Ok("true"))
}

/// Every (material, style) the sweep shows, as indices into the presets, in
/// order: each material through every style.
pub fn combinations() -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for m in SWEEP_MATERIALS {
        let Some(mi) = material_index(m) else {
            continue;
        };
        for s in SWEEP_STYLES {
            if let Some(si) = style_index(s) {
                out.push((mi, si));
            }
        }
    }
    out
}

pub fn material_name(i: usize) -> &'static str {
    MATERIALS[i].name
}

pub fn style_name(i: usize) -> String {
    STYLES[i].label()
}

/// A version's compile, as the sweep reports it.
#[derive(Clone, Default)]
pub struct CompileRecord {
    /// "cold", "warm" (compiled earlier this launch), "failed",
    /// "timed_out", "not_measured" (a backend that does not say) or "none".
    pub state: &'static str,
    pub total_ms: Option<f64>,
    pub codegen_ms: Option<f64>,
    pub text_bytes: Option<usize>,
    pub text_fnv: Option<u64>,
}

/// One run's medians.
#[derive(Clone, Default)]
pub struct RunRecord {
    pub gpu_ms: Option<f64>,
    pub gpu_samples: usize,
    pub cpu_ms: Option<f64>,
    pub frames: usize,
}

/// One look: where each version stood in the screenshot, the empty grid,
/// and each version's run.
#[derive(Clone, Default)]
pub struct ComboRecord {
    pub step: usize,
    pub material: usize,
    pub style: usize,
    /// Window-local layout points: x, y, width, height, per version.
    pub rects: Vec<Option<[f64; 4]>>,
    pub baseline: RunRecord,
    pub baseline_runs: Vec<(Option<f64>, Option<f64>)>,
    pub versions: Vec<Option<RunRecord>>,
}

/// Everything the sweep writes.
pub struct SweepReport<'a> {
    pub backend: &'a str,
    pub adapter: Option<&'a str>,
    pub started: f64,
    pub finished: f64,
    pub stopped: bool,
    pub grid_knobs: usize,
    pub grid_size: f64,
    pub shot_size: f64,
    /// The window's DPI factor: pixels per layout point in the screenshots.
    pub dpi_factor: f64,
    /// The knob's radius as a fraction of its square, in the screenshot and
    /// in the measuring grid.
    pub shot_fill: f64,
    pub grid_fill: f64,
    pub warm_seconds: f64,
    pub compiles: &'a [CompileRecord],
    pub combos: &'a [ComboRecord],
    pub steps: usize,
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => format!("{v:.4}"),
        _ => "null".to_string(),
    }
}

fn per_knob(v: Option<f64>, base: Option<f64>, knobs: usize) -> Option<f64> {
    Some((v? - base?) / knobs.max(1) as f64)
}

impl SweepReport<'_> {
    pub fn to_json(&self) -> String {
        let mut o = String::new();
        o.push_str("{\n");
        o.push_str("  \"tool\": \"makepad storybook: Knob cost, Run everything\",\n");
        o.push_str(&format!("  \"timestamp\": {:.3},\n", self.finished));
        o.push_str(&format!("  \"started\": {:.3},\n", self.started));
        o.push_str(&format!("  \"stopped\": {},\n", self.stopped));
        o.push_str(&format!("  \"os\": {},\n", json_str(std::env::consts::OS)));
        o.push_str(&format!("  \"arch\": {},\n", json_str(std::env::consts::ARCH)));
        o.push_str(&format!("  \"backend\": {},\n", json_str(self.backend)));
        o.push_str(&format!("  \"adapter\": {},\n", self.adapter.map(json_str).unwrap_or_else(|| "null".into())));
        o.push_str(&format!("  \"salt\": {},\n", launch_salt()));
        o.push_str(&format!(
            "  \"grid\": {{\"knobs\": {}, \"knob_size_pt\": {}, \"fill\": {}, \"warm_seconds\": {}, \
             \"run_seconds\": {}}},\n",
            self.grid_knobs, self.grid_size, self.grid_fill, self.warm_seconds, RUN_SECONDS
        ));
        o.push_str(&format!("  \"shot_size_pt\": {},\n", self.shot_size));
        o.push_str(&format!("  \"dpi_factor\": {},\n", self.dpi_factor));
        o.push_str(&format!("  \"shot_fill\": {},\n", self.shot_fill));
        o.push_str(&format!("  \"hold_seconds\": {},\n", HOLD_SECONDS));
        o.push_str(&format!(
            "  \"notes\": {},\n",
            json_str(
                "compile_ms: from Compile until the backend reports the shader ready, polled once a frame; the shader \
                 text is salted per launch so no cache answers. gpu_ms_frame: median GPU time of the page's pass per \
                 frame (null where the backend records none). cpu_ms_frame: median frame interval. per_knob: (version \
                 - empty grid) / knobs, the empty grid measured before and after the look's runs. rects: window-local \
                 layout points of each version's knob in the screenshot."
            )
        ));
        o.push_str("  \"versions\": [\n");
        for (i, v) in VERSIONS.iter().enumerate() {
            let c = self.compiles.get(i).cloned().unwrap_or_default();
            o.push_str(&format!(
                "    {{\"key\": {}, \"name\": {}, \"leaves_out\": {}, \"replaces\": {}, \"compile\": {}, \
                 \"compile_ms\": {}, \"codegen_ms\": {}, \"text_bytes\": {}, \"text_fnv\": {}}}{}\n",
                json_str(v.key),
                json_str(v.name),
                json_str(v.leaves_out),
                json_str(v.replaces),
                json_str(if c.state.is_empty() { "none" } else { c.state }),
                num(c.total_ms),
                num(c.codegen_ms),
                c.text_bytes.map(|b| b.to_string()).unwrap_or_else(|| "null".into()),
                c.text_fnv.map(|h| json_str(&format!("{h:016x}"))).unwrap_or_else(|| "null".into()),
                if i + 1 < VERSIONS.len() { "," } else { "" }
            ));
        }
        o.push_str("  ],\n");
        let names: Vec<String> = SWEEP_MATERIALS.iter().map(|m| json_str(m)).collect();
        o.push_str(&format!("  \"materials\": [{}],\n", names.join(", ")));
        let names: Vec<String> = SWEEP_STYLES
            .iter()
            .filter_map(|s| style_index(s))
            .map(|i| json_str(&style_name(i)))
            .collect();
        o.push_str(&format!("  \"styles\": [{}],\n", names.join(", ")));
        o.push_str(&format!("  \"steps\": {},\n", self.steps));
        o.push_str("  \"combinations\": [\n");
        for (k, c) in self.combos.iter().enumerate() {
            o.push_str("    {\n");
            o.push_str(&format!(
                "      \"step\": {}, \"material\": {}, \"style\": {},\n",
                c.step,
                json_str(material_name(c.material)),
                json_str(&style_name(c.style))
            ));
            let rects: Vec<String> = VERSIONS
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let r = c.rects.get(i).copied().flatten();
                    let r = r
                        .map(|r| format!("[{:.1}, {:.1}, {:.1}, {:.1}]", r[0], r[1], r[2], r[3]))
                        .unwrap_or_else(|| "null".into());
                    format!("{}: {}", json_str(v.key), r)
                })
                .collect();
            o.push_str(&format!("      \"rects\": {{{}}},\n", rects.join(", ")));
            let runs: Vec<String> =
                c.baseline_runs.iter().map(|(g, u)| format!("[{}, {}]", num(*g), num(*u))).collect();
            o.push_str(&format!(
                "      \"empty_grid\": {{\"gpu_ms_frame\": {}, \"cpu_ms_frame\": {}, \"frames\": {}, \
                 \"gpu_samples\": {}, \"runs_gpu_cpu\": [{}]}},\n",
                num(c.baseline.gpu_ms),
                num(c.baseline.cpu_ms),
                c.baseline.frames,
                c.baseline.gpu_samples,
                runs.join(", ")
            ));
            o.push_str("      \"versions\": {\n");
            for (i, v) in VERSIONS.iter().enumerate() {
                let r = c.versions.get(i).cloned().flatten();
                let line = match r {
                    Some(r) => format!(
                        "{{\"gpu_ms_frame\": {}, \"gpu_ms_per_knob\": {}, \"cpu_ms_frame\": {}, \
                         \"cpu_ms_per_knob\": {}, \"frames\": {}, \"gpu_samples\": {}}}",
                        num(r.gpu_ms),
                        num(per_knob(r.gpu_ms, c.baseline.gpu_ms, self.grid_knobs)),
                        num(r.cpu_ms),
                        num(per_knob(r.cpu_ms, c.baseline.cpu_ms, self.grid_knobs)),
                        r.frames,
                        r.gpu_samples
                    ),
                    None => "null".to_string(),
                };
                o.push_str(&format!(
                    "        {}: {}{}\n",
                    json_str(v.key),
                    line,
                    if i + 1 < VERSIONS.len() { "," } else { "" }
                ));
            }
            o.push_str("      }\n");
            o.push_str(&format!("    }}{}\n", if k + 1 < self.combos.len() { "," } else { "" }));
        }
        o.push_str("  ]\n}\n");
        o
    }
}

/// Where the results go: `local/knob-cost/` under the current directory
/// when it has a `local/`, else the system's temp directory.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_results(json: &str, stamp: f64) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let dir = if cwd.join("local").is_dir() { cwd.join("local").join("knob-cost") } else { std::env::temp_dir() };
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("results-{}.json", stamp as u64));
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    let path = path.canonicalize().unwrap_or(path);
    Ok(path.display().to_string())
}

#[cfg(target_arch = "wasm32")]
pub fn write_results(_json: &str, _stamp: f64) -> Result<String, String> {
    Err("no file system here; the JSON is on the clipboard and in the log".to_string())
}
