//! `cargo makepad wasm --pack=<film|app|full> [--strict] build ...`: the app
//! packed from what it was seen to need.
//!
//! 1. **collect**: the app built natively (release) and run once in its
//!    collect-web mode (`MAKEPAD_RUN=collect-web`, hidden and muted), which
//!    writes a manifest directory (`makepad_web_pack::manifest`): the draw
//!    shaders it applied as GLSL ES with their reflection, the glyphs it
//!    laid out per font file, what it declared (open text, ranges), the
//!    widget types it reached, the web runtime sections it needs.
//! 2. **link**: the wasm build with the shader pack embedded
//!    (`--cfg makepad_shader_pack`), and without the shader compiler
//!    (`--cfg makepad_precompiled_shaders`) where the preset allows it: the
//!    film preset, or the app preset with `--strict`.
//! 3. **fonts**: each packaged font cut to what the run proved it shows
//!    (the film preset, `--strict`, or an app that declared narrower
//!    coverage for the face), else shipped complete.
//! 4. **js**: the web runtime without the sections the app does not need.
//!
//! Every choice and what it saves is printed, and written to
//! `target/makepad-collect/<crate>/pack-report.txt`.

use crate::makepad_shell::*;
use makepad_web_pack::coverage::{classify, Coverage};
use makepad_web_pack::manifest::Manifest;
use makepad_web_pack::pack::{FontsKnob, PackOptions, PackPreset, ShadersKnob};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The pack's choices, from `--pack=<preset>` and `--strict`.
#[derive(Clone, Debug, PartialEq)]
pub struct PackConfig {
    pub options: PackOptions,
    /// Only what the run collected: no shader compiler, fonts cut, no JS
    /// section kept on spec.
    pub strict: bool,
}

impl PackConfig {
    pub fn new(preset: PackPreset, strict: bool) -> PackConfig {
        let mut options = preset.options();
        if strict && preset != PackPreset::Full {
            options.shaders = ShadersKnob::PrebuiltOnly;
            options.fonts = FontsKnob::Analysed;
        }
        PackConfig { options, strict }
    }

    /// Whether the wasm is linked without the shader compiler.
    pub fn precompiled_shaders(&self) -> bool {
        self.options.shaders == ShadersKnob::PrebuiltOnly
    }
}

/// What a pack did, for the report.
#[derive(Default)]
pub struct PackReport {
    pub lines: Vec<String>,
}

impl PackReport {
    pub fn line(&mut self, s: impl Into<String>) {
        let s = s.into();
        println!("[pack] {s}");
        self.lines.push(s);
    }
}

/// The collect run's directory for a crate.
pub fn collect_dir(cwd: &Path, build_crate: &str) -> PathBuf {
    cwd.join("target/makepad-collect").join(build_crate)
}

/// The arguments of the native build: the wasm build's without its
/// profile and target (the native build is always release).
fn native_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--profile" || a == "--target" {
            i += 2;
            continue;
        }
        if a == "--release" || a.starts_with("--profile=") || a.starts_with("--target=") {
            i += 1;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out
}

/// Builds the app natively and runs its collect-web mode into
/// [`collect_dir`]; the manifest it wrote.
pub fn collect(cwd: &Path, build_crate: &str, build_bin: &str, args: &[String], report: &mut PackReport) -> Result<(PathBuf, Manifest), String> {
    let mut build = vec!["build".to_string(), "--release".to_string()];
    build.extend(native_args(args));
    let build_refs: Vec<&str> = build.iter().map(|s| s.as_str()).collect();
    report.line(format!("collect: cargo {}", build.join(" ")));
    shell_env(&[], cwd, "cargo", &build_refs)?;

    let exe = cwd.join("target/release").join(if cfg!(windows) { format!("{build_bin}.exe") } else { build_bin.to_string() });
    let dir = collect_dir(cwd, build_crate);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let started = std::time::Instant::now();
    let status = std::process::Command::new(&exe)
        .current_dir(cwd)
        .env("MAKEPAD_RUN", "collect-web")
        .env("MAKEPAD_COLLECT", &dir)
        .env("MAKEPAD_HIDE_WINDOWS", "1")
        .env("SANDBOX_MUTE", "1")
        .status()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    if !Manifest::is_done(&dir) {
        return Err(format!("{} in collect-web mode did not write its manifest ({status})", exe.display()));
    }
    let manifest = Manifest::read(&dir)?;
    let get = |k: &str| manifest.summary.get(k).cloned().unwrap_or_else(|| "0".into());
    report.line(format!(
        "collect: {:.1} s, {} shaders, {} fonts, {} glyphs, {} widget types, js sections needed: {}",
        started.elapsed().as_secs_f64(),
        get("shaders"),
        get("fonts"),
        get("glyphs"),
        manifest.widgets.len(),
        if manifest.js.is_empty() { "none".to_string() } else { manifest.js.keys().cloned().collect::<Vec<_>>().join(", ") }
    ));
    for d in &manifest.diagnostics {
        report.line(format!("collect {}: {}", d.severity.as_str(), d.message));
    }
    Ok((dir, manifest))
}

/// The wasm build's extra rustflags and environment for the pack.
pub fn link_env(config: &PackConfig, dir: &Path, manifest: &Manifest, report: &mut PackReport) -> (String, Vec<(String, String)>) {
    let mut flags = String::new();
    let mut env = Vec::new();
    if manifest.shaders.is_empty() {
        report.line("shaders: none collected; the compiler stays");
        return (flags, env);
    }
    flags.push_str(" --cfg makepad_shader_pack");
    env.push(("MAKEPAD_SHADER_PACK".to_string(), dir.join(makepad_web_pack::manifest::files::SHADERS).display().to_string()));
    if config.precompiled_shaders() {
        flags.push_str(" --cfg makepad_precompiled_shaders");
        report.line(format!("shaders: {} bytes of pack embedded, shader compiler left out (a shader not in the pack is logged and not drawn)", manifest.shaders.len()));
    } else {
        report.line(format!("shaders: {} bytes of pack embedded, the compiler kept for shaders made at run time", manifest.shaders.len()));
    }
    (flags, env)
}

fn font_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            font_files(&path, out);
        } else if crate::font_assets::is_font_path(&path) {
            out.push(path);
        }
    }
}

fn brotli_size(bytes: &[u8]) -> usize {
    makepad_web_pack::brotli(bytes).len()
}

/// Writes `path` and, when the package is brotli'd, its `.br`.
fn write_with_brotli(path: &Path, bytes: &[u8], brotli: bool) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let br = PathBuf::from(format!("{}.br", path.display()));
    if brotli {
        std::fs::write(&br, makepad_web_pack::brotli(bytes)).map_err(|e| format!("{}: {e}", br.display()))?;
    } else {
        let _ = std::fs::remove_file(&br);
    }
    Ok(())
}

/// Cuts every packaged font under `app_dir` to what the run proved it
/// shows, where the knobs allow.
pub fn subset_fonts(app_dir: &Path, config: &PackConfig, manifest: &Manifest, brotli: bool, report: &mut PackReport) -> Result<(), String> {
    let mut fonts = Vec::new();
    font_files(app_dir, &mut fonts);
    let (mut before, mut after, mut before_br, mut after_br) = (0usize, 0usize, 0usize, 0usize);
    // Several resource roots can hold the same file; cut each once.
    let mut done: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in fonts {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let hash = makepad_web_pack::manifest::font_hash(&bytes);
        let name = path.strip_prefix(app_dir).unwrap_or(&path).display().to_string();
        before += bytes.len();
        before_br += brotli_size(&bytes);
        if let Some(cut) = done.get(&hash) {
            write_with_brotli(&path, cut, brotli)?;
            after += cut.len();
            after_br += brotli_size(cut);
            continue;
        }
        let declared: Vec<&str> = manifest.coverage.iter().filter(|c| c.face == hash).map(|c| c.spec.as_str()).collect();
        let full_by: Vec<&str> = manifest.coverage.iter().filter(|c| c.face == hash && c.spec.split(',').any(|p| p.trim() == "all")).map(|c| c.by.as_str()).collect();
        let seen = manifest.glyphs.get(&hash).cloned().unwrap_or_default();
        // An app's run lays text out as it is shown: nothing it laid out is
        // transient film text, so the counter and declared rules apply and
        // open text is what the app declared.
        let texts: Vec<(String, u32)> = manifest.texts.get(&hash).map(|t| t.keys().map(|k| (k.clone(), u32::MAX)).collect()).unwrap_or_default();
        let narrower = !declared.is_empty() && full_by.is_empty();
        let analysed = config.options.fonts == FontsKnob::Analysed || (config.options.preset == PackPreset::App && narrower);
        let coverage = if !full_by.is_empty() {
            Coverage::Full(format!("open text, requested by {}", full_by.iter().copied().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join("; ")))
        } else if !analysed {
            Coverage::Full(format!("fonts: full ({} preset)", config.options.preset.name()))
        } else if seen.is_empty() && declared.is_empty() {
            Coverage::Full("never laid out by the run: shipped complete (declare its coverage to cut it)".into())
        } else {
            classify(&seen, &texts, &declared, "")
        };
        let out = match coverage {
            Coverage::Full(why) => {
                report.line(format!("font {name}: full, {} bytes: {why}", bytes.len()));
                bytes.clone()
            }
            Coverage::Subset { mut glyphs, chars, why } => match cut_font(&bytes, &mut glyphs, &chars) {
                Ok((cut, kept, total)) => {
                    report.line(format!("font {name}: subset, {kept} of {total} glyphs, {} -> {} bytes: {}", bytes.len(), cut.len(), why.join("; ")));
                    cut
                }
                Err(e) => {
                    report.line(format!("font {name}: full, {} bytes: cannot cut it ({e})", bytes.len()));
                    bytes.clone()
                }
            },
        };
        if out != bytes {
            write_with_brotli(&path, &out, brotli)?;
        }
        after += out.len();
        after_br += brotli_size(&out);
        done.insert(hash, out);
    }
    report.line(format!("fonts: {before} -> {after} bytes ({before_br} -> {after_br} brotli)"));
    Ok(())
}

/// One font cut to `glyphs` plus `chars` (with their substitutions): the
/// bytes, glyphs kept, glyphs in the font.
fn cut_font(bytes: &[u8], glyphs: &mut BTreeSet<u16>, chars: &BTreeSet<char>) -> Result<(Vec<u8>, usize, usize), String> {
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|_| "not a font".to_string())?;
    let total = face.number_of_glyphs() as usize;
    let mut extra: BTreeSet<u16> = chars.iter().filter_map(|&c| face.glyph_index(c).map(|g| g.0)).collect();
    makepad_font_subset::gsub_closure(&face, &mut extra);
    glyphs.extend(extra);
    let copyright = face.names().into_iter().filter(|n| n.name_id == 0 && n.is_unicode()).find_map(|n| n.to_string()).unwrap_or_default();
    let family = face.names().into_iter().filter(|n| n.name_id == 1 && n.is_unicode()).find_map(|n| n.to_string()).unwrap_or_default();
    // The OFL treats a subset as a modified version: a reserved name goes.
    let reserved = makepad_font_subset::reserved_font_names(&copyright);
    let rename = (!reserved.is_empty()).then(|| makepad_font_subset::Rename { reserved: reserved.clone(), family: format!("{} Web", family.replace(reserved[0].as_str(), "Makepad")) });
    let opts = makepad_font_subset::SubsetOptions { rename, ..Default::default() };
    let (cut, report) = makepad_font_subset::subset_with_report(bytes, glyphs, chars, &opts).map_err(|e| format!("{e:?}"))?;
    Ok((cut, report.kept.len(), total))
}

/// Packages the web runtime's JavaScript (`(source, packaged)` pairs)
/// without the sections the pack leaves out, minified as the package's
/// other JavaScript is.
pub fn strip_js(runtime: &[(PathBuf, PathBuf)], stripped: &[String], brotli: bool, report: &mut PackReport) -> Result<(), String> {
    let names: Vec<&str> = stripped.iter().map(|s| s.as_str()).collect();
    let (mut before, mut after) = (0usize, 0usize);
    for (source_path, dest) in runtime {
        let source = std::fs::read_to_string(source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
        let out = super::compile::minify_js(&makepad_web_pack::js::strip_sections(&source, &names, &source_path.display().to_string())?);
        before += brotli_size(super::compile::minify_js(&source).as_bytes());
        after += brotli_size(out.as_bytes());
        write_with_brotli(dest, out.as_bytes(), brotli)?;
    }
    report.line(format!(
        "js: stripped {} ({before} -> {after} bytes brotli)",
        if names.is_empty() { "nothing".to_string() } else { names.join(", ") }
    ));
    Ok(())
}

/// The per-crate code sizes of a linked (named) wasm, largest first.
pub fn crate_profile(wasm: &[u8]) -> String {
    let opts = makepad_wasm_strip::ProfileOptions::default();
    match makepad_wasm_strip::wasm_size_profile(wasm, &opts) {
        Ok(p) => {
            let mut out = String::new();
            let _ = writeln!(out, "total {} code {} data {}", p.total_bytes, p.code_bytes, p.data_bytes);
            for c in &p.crates {
                let _ = writeln!(out, "{}\t{}\t{}", c.name, c.bytes, c.functions);
            }
            out
        }
        Err(_) => "unparsable wasm\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_args_drop_profile_and_target() {
        let args: Vec<String> = ["--release", "-p", "app", "--profile=small", "--target", "x", "--no-default-features"].iter().map(|s| s.to_string()).collect();
        assert_eq!(native_args(&args), vec!["-p", "app", "--no-default-features"]);
    }

    #[test]
    fn strict_app_drops_the_compiler() {
        assert!(!PackConfig::new(PackPreset::App, false).precompiled_shaders());
        assert!(PackConfig::new(PackPreset::App, true).precompiled_shaders());
        assert!(PackConfig::new(PackPreset::Film, false).precompiled_shaders());
        assert!(!PackConfig::new(PackPreset::Full, true).precompiled_shaders());
    }
}
