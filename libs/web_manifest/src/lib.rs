//! The collect manifest: what a collect run learned an app needs on the
//! web, as files in one directory.
//!
//! Two writers make it: the platform's collect-web run mode
//! (`MAKEPAD_RUN=collect-web MAKEPAD_COLLECT=<dir>`, see the platform's
//! `collect` module) and Stage's film analysis. Two readers use it:
//! `cargo makepad wasm --pack` and Stage's web export. The file names are
//! [`files`]; [`Manifest`] writes and reads the whole directory.
//!
//! A font face is named by the 16 hex digits of the FNV-1a 64 hash of its
//! file ([`font_hash`]), or, in `coverage.txt` only, by a family name as a
//! document wrote it. Every requirement that an emitter can be named for
//! carries a `by` field: the widget type and path, the source location, or
//! the app or runtime that asked.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

/// The files of a manifest directory.
pub mod files {
    /// Per font file: `<font hash> <glyph id> <glyph id> ...`, one font per
    /// line: every glyph a layout returned.
    pub const GLYPHS: &str = "glyphs.txt";
    /// Per font, each row of text it laid out and in how many frames:
    /// `<font hash>\t<frames>\t<text>` (`\\`, `\n`, `\t` escaped).
    pub const TEXTS: &str = "texts.txt";
    /// What a face must show beyond the glyphs seen:
    /// `<family or font hash>\t<spec>[\t<by>]`. Specs: `all`, `latin`,
    /// `digits`, `U+0041-U+005A`, `chars:...`, comma-separated.
    pub const COVERAGE: &str = "coverage.txt";
    /// One wasm module holding every kernel compiled (a runtime that links
    /// its kernels writes it).
    pub const KERNELS: &str = "kernels.wasm";
    /// Per kernel of [`KERNELS`]: the runtime's own key table.
    pub const KERNEL_KEYS: &str = "kernels.keys";
    /// Opaque named kernel blobs: `<name>\t<bytes>\t<by>`, each blob in
    /// [`KERNEL_DIR`]`/<name>`.
    pub const KERNEL_LIST: &str = "kernels.txt";
    pub const KERNEL_DIR: &str = "kernels";
    /// The shader pack: every draw shader's GLSL ES and the compiler's
    /// reflection of it (the platform's `shader_pack` format).
    pub const SHADERS: &str = "shaders.pack";
    /// The draw shaders of [`SHADERS`], one `<key hex>\t<name>` per line
    /// (for reports).
    pub const SHADER_LIST: &str = "shaders.txt";
    /// The subsystems used, one name per line.
    pub const FEATURES: &str = "features.txt";
    /// `name=value` lines: frames, shaders, fonts, glyphs, widgets.
    pub const SUMMARY: &str = "summary.txt";
    /// `severity\tfile\tline\tcol\tmessage`, one per line.
    pub const DIAGNOSTICS: &str = "diagnostics.txt";
    /// Widget types reached: `<type>\t<instances>\t<by>` (`by`: the first
    /// one seen).
    pub const WIDGETS: &str = "widgets.txt";
    /// The web runtime's `// @section` names the app needs:
    /// `<section>\t<by>`, one line per (section, emitter).
    pub const JS: &str = "js.txt";
    /// Assets the app loads: `<path>\t<by>`.
    pub const ASSETS: &str = "assets.txt";
    /// Written last: the run finished.
    pub const DONE: &str = "done";
}

/// The marked sections of the platform's web runtime (`// @section <name>`
/// ... `// @end <name>` in `web.js`, `web_gl.js`), by what they serve.
pub const JS_SECTIONS: [&str; 14] = [
    "text-input",
    "clipboard",
    "file-dialog",
    "midi",
    "xr",
    "websocket",
    "live-reload",
    "storage",
    "crash-upload",
    "history",
    "geolocation",
    "legacy-http",
    "permissions",
    "video-playback",
];

/// The FNV-1a 64 hash of a font file as 16 hex digits: how the manifest
/// names a face.
pub fn font_hash(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    format!("{h:016x}")
}

/// Whether `key` names a font file ([`font_hash`]) rather than a family.
pub fn is_font_hash(key: &str) -> bool {
    key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A field for a tab-separated line: `\\`, `\n` and `\t` escaped.
pub fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

/// A field back to its text.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// A coverage declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Coverage {
    /// A family name, or a [`font_hash`].
    pub face: String,
    pub spec: String,
    pub by: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Severity {
        match s {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => Severity::Note,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Empty when the problem has no place.
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

/// One diagnostics line.
pub fn diagnostic_line(severity: &str, file: &str, line: u32, col: u32, message: &str) -> String {
    format!("{severity}\t{file}\t{line}\t{col}\t{}\n", message.replace('\n', "\\n").replace('\t', " "))
}

/// A widget type reached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WidgetUse {
    pub instances: usize,
    /// Where it was first seen.
    pub by: String,
}

/// A named kernel blob.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KernelBlob {
    pub bytes: Vec<u8>,
    pub by: String,
}

/// A whole manifest directory in memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub glyphs: BTreeMap<String, BTreeSet<u16>>,
    /// Per font hash: text -> frames it was laid out in.
    pub texts: BTreeMap<String, BTreeMap<String, u32>>,
    pub coverage: BTreeSet<Coverage>,
    pub kernels_wasm: Vec<u8>,
    pub kernel_keys: Vec<u8>,
    pub kernel_blobs: BTreeMap<String, KernelBlob>,
    pub shaders: Vec<u8>,
    /// (key, name) of each shader of the pack.
    pub shader_list: Vec<(u64, String)>,
    pub features: BTreeSet<String>,
    pub summary: BTreeMap<String, String>,
    pub diagnostics: Vec<Diagnostic>,
    pub widgets: BTreeMap<String, WidgetUse>,
    /// Section -> the emitters that asked for it.
    pub js: BTreeMap<String, BTreeSet<String>>,
    /// Path -> the first emitter.
    pub assets: BTreeMap<String, String>,
}

/// A kernel name as a file name: anything but `[A-Za-z0-9._-]` becomes `_`.
fn kernel_file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if s.starts_with('.') || s.is_empty() {
        format!("_{s}")
    } else {
        s
    }
}

impl Manifest {
    /// Writes every file into `dir` (created), [`files::DONE`] last.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::remove_file(dir.join(files::DONE));
        let mut glyphs = String::new();
        for (font, ids) in &self.glyphs {
            glyphs.push_str(font);
            for id in ids {
                glyphs.push_str(&format!(" {id}"));
            }
            glyphs.push('\n');
        }
        std::fs::write(dir.join(files::GLYPHS), glyphs)?;
        let mut texts = String::new();
        for (font, rows) in &self.texts {
            for (text, frames) in rows {
                texts.push_str(&format!("{font}\t{frames}\t{}\n", escape(text)));
            }
        }
        std::fs::write(dir.join(files::TEXTS), texts)?;
        let coverage: String = self
            .coverage
            .iter()
            .map(|c| if c.by.is_empty() { format!("{}\t{}\n", c.face, c.spec) } else { format!("{}\t{}\t{}\n", c.face, c.spec, escape(&c.by)) })
            .collect();
        std::fs::write(dir.join(files::COVERAGE), coverage)?;
        std::fs::write(dir.join(files::KERNELS), &self.kernels_wasm)?;
        std::fs::write(dir.join(files::KERNEL_KEYS), &self.kernel_keys)?;
        let mut kernel_list = String::new();
        if !self.kernel_blobs.is_empty() {
            std::fs::create_dir_all(dir.join(files::KERNEL_DIR))?;
        }
        for (name, blob) in &self.kernel_blobs {
            let file = kernel_file_name(name);
            std::fs::write(dir.join(files::KERNEL_DIR).join(&file), &blob.bytes)?;
            kernel_list.push_str(&format!("{file}\t{}\t{}\n", blob.bytes.len(), escape(&blob.by)));
        }
        std::fs::write(dir.join(files::KERNEL_LIST), kernel_list)?;
        if !self.shaders.is_empty() {
            std::fs::write(dir.join(files::SHADERS), &self.shaders)?;
        }
        let shader_list: String = self.shader_list.iter().map(|(k, n)| format!("{k:016x}\t{}\n", escape(n))).collect();
        std::fs::write(dir.join(files::SHADER_LIST), shader_list)?;
        let features: String = self.features.iter().map(|f| format!("{f}\n")).collect();
        std::fs::write(dir.join(files::FEATURES), features)?;
        let widgets: String = self.widgets.iter().map(|(ty, w)| format!("{ty}\t{}\t{}\n", w.instances, escape(&w.by))).collect();
        std::fs::write(dir.join(files::WIDGETS), widgets)?;
        let mut js = String::new();
        for (section, by) in &self.js {
            for b in by {
                js.push_str(&format!("{section}\t{}\n", escape(b)));
            }
        }
        std::fs::write(dir.join(files::JS), js)?;
        let assets: String = self.assets.iter().map(|(p, by)| format!("{}\t{}\n", escape(p), escape(by))).collect();
        std::fs::write(dir.join(files::ASSETS), assets)?;
        let diagnostics: String = self.diagnostics.iter().map(|d| diagnostic_line(d.severity.as_str(), &d.file, d.line, d.col, &d.message)).collect();
        std::fs::write(dir.join(files::DIAGNOSTICS), diagnostics)?;
        let mut summary = std::fs::File::create(dir.join(files::SUMMARY))?;
        for (k, v) in &self.summary {
            writeln!(summary, "{k}={v}")?;
        }
        std::fs::write(dir.join(files::DONE), b"")?;
        Ok(())
    }

    /// Reads a manifest directory. Only [`files::GLYPHS`] is required;
    /// the other files are empty when absent (older writers).
    pub fn read(dir: &Path) -> Result<Manifest, String> {
        let text = |name: &str| std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()));
        let opt = |name: &str| text(name).unwrap_or_default();
        let mut m = Manifest::default();
        for line in text(files::GLYPHS)?.lines() {
            let mut words = line.split_whitespace();
            let Some(font) = words.next() else { continue };
            let set = m.glyphs.entry(font.to_string()).or_default();
            for w in words {
                set.insert(w.parse().map_err(|_| format!("{}: `{w}` is not a glyph id", files::GLYPHS))?);
            }
        }
        for line in opt(files::TEXTS).lines() {
            let mut f = line.splitn(3, '\t');
            let (Some(font), Some(frames), Some(t)) = (f.next(), f.next(), f.next()) else { continue };
            *m.texts.entry(font.to_string()).or_default().entry(unescape(t)).or_default() += frames.parse().unwrap_or(1);
        }
        for line in opt(files::COVERAGE).lines() {
            let mut f = line.splitn(3, '\t');
            if let (Some(face), Some(spec)) = (f.next(), f.next()) {
                m.coverage.insert(Coverage { face: face.to_string(), spec: spec.to_string(), by: f.next().map(unescape).unwrap_or_default() });
            }
        }
        m.kernels_wasm = std::fs::read(dir.join(files::KERNELS)).unwrap_or_default();
        m.kernel_keys = std::fs::read(dir.join(files::KERNEL_KEYS)).unwrap_or_default();
        for line in opt(files::KERNEL_LIST).lines() {
            let mut f = line.splitn(3, '\t');
            let (Some(name), Some(_), by) = (f.next(), f.next(), f.next()) else { continue };
            let bytes = std::fs::read(dir.join(files::KERNEL_DIR).join(name)).map_err(|e| format!("{}/{name}: {e}", files::KERNEL_DIR))?;
            m.kernel_blobs.insert(name.to_string(), KernelBlob { bytes, by: by.map(unescape).unwrap_or_default() });
        }
        m.shaders = std::fs::read(dir.join(files::SHADERS)).unwrap_or_default();
        for line in opt(files::SHADER_LIST).lines() {
            if let Some((k, n)) = line.split_once('\t') {
                m.shader_list.push((u64::from_str_radix(k, 16).unwrap_or(0), unescape(n)));
            }
        }
        m.features = opt(files::FEATURES).lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        for line in opt(files::WIDGETS).lines() {
            let mut f = line.splitn(3, '\t');
            if let (Some(ty), Some(n)) = (f.next(), f.next()) {
                m.widgets.insert(ty.to_string(), WidgetUse { instances: n.parse().unwrap_or(1), by: f.next().map(unescape).unwrap_or_default() });
            }
        }
        for line in opt(files::JS).lines() {
            let mut f = line.splitn(2, '\t');
            if let Some(section) = f.next().filter(|s| !s.is_empty()) {
                m.js.entry(section.to_string()).or_default().extend(f.next().map(unescape));
            }
        }
        for line in opt(files::ASSETS).lines() {
            let mut f = line.splitn(2, '\t');
            if let Some(path) = f.next().filter(|s| !s.is_empty()) {
                m.assets.insert(unescape(path), f.next().map(unescape).unwrap_or_default());
            }
        }
        m.diagnostics = read_diagnostics(dir);
        for line in opt(files::SUMMARY).lines() {
            if let Some((k, v)) = line.split_once('=') {
                m.summary.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Ok(m)
    }

    /// Whether the writer finished ([`files::DONE`] exists).
    pub fn is_done(dir: &Path) -> bool {
        dir.join(files::DONE).exists()
    }
}

/// The diagnostics of a manifest directory (empty when there are none).
pub fn read_diagnostics(dir: &Path) -> Vec<Diagnostic> {
    let Ok(text) = std::fs::read_to_string(dir.join(files::DIAGNOSTICS)) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(5, '\t');
            let severity = Severity::parse(f.next()?);
            let file = f.next()?.to_string();
            let line = f.next()?.parse().unwrap_or(0);
            let col = f.next()?.parse().unwrap_or(0);
            let message = f.next()?.replace("\\n", "\n");
            Some(Diagnostic { severity, file, line, col, message })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_reads_back_as_written() {
        let dir = std::env::temp_dir().join(format!("makepad-web-manifest-test-{}", std::process::id()));
        let mut m = Manifest::default();
        m.glyphs.insert(font_hash(b"font"), [1, 2, 300].into());
        m.texts.entry(font_hash(b"font")).or_default().insert("a\tb\nc\\".into(), 3);
        m.coverage.insert(Coverage { face: font_hash(b"font"), spec: "all".into(), by: "TextInput at main.login".into() });
        m.coverage.insert(Coverage { face: "Inter".into(), spec: "latin".into(), by: String::new() });
        m.kernel_blobs.insert("blur/x".into(), KernelBlob { bytes: vec![0, 97, 115, 109], by: "runtime".into() });
        m.shaders = b"MPSP".to_vec();
        m.shader_list.push((0x1234, "DrawQuad".into()));
        m.features.insert("kernels".into());
        m.summary.insert("frames".into(), "4".into());
        m.diagnostics.push(Diagnostic { severity: Severity::Warning, file: "a.rs".into(), line: 3, col: 1, message: "two\nlines".into() });
        m.widgets.insert("Button".into(), WidgetUse { instances: 2, by: "main.ok".into() });
        m.js.entry("text-input".into()).or_default().insert("TextInput at main.name".into());
        m.assets.insert("app/resources/logo.png".into(), "Image at main.logo".into());
        m.write(&dir).unwrap();
        assert!(Manifest::is_done(&dir));
        let mut back = Manifest::read(&dir).unwrap();
        // Blob names are file names.
        assert!(back.kernel_blobs.remove("blur_x").is_some());
        m.kernel_blobs.clear();
        assert_eq!(back, m);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn font_hashes_name_files() {
        assert!(is_font_hash(&font_hash(b"x")));
        assert!(!is_font_hash("Inter"));
        assert_eq!(unescape(&escape("a\\b\tc\nd")), "a\\b\tc\nd");
    }
}
