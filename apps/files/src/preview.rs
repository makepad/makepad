//! Opening and previewing files, through `makepad_wm_api`.
//!
//! Which app answers is never decided here, and there is deliberately no
//! association table in this crate: `makepad_wm_api::viewer_for` is the one the
//! compositor and the browser share (pictures → image, video → video,
//! csv/tsv → sheets, pdf → pdf, html → browser, everything else →
//! terminal's `--preview` pager). A file type that opens in the wrong app is
//! fixed there, never here.
//!
//! Hosted as an wm tile, an app never spawns anything: it asks, and the
//! compositor floats the viewer over the desk (Quick Look) or opens it as a
//! tile. Standalone the same call spawns the sibling binary — except for the
//! preview, which files spawns itself so that Space and Escape can take the
//! popup away again; `makepad_wm_api::preview`'s child is detached and could not be
//! dismissed.
//!
//! Whether a Quick Look panel is open is **never** this app's own belief:
//! hosted, the WM says so with `PreviewShown`/`PreviewHidden`, and standalone
//! the answer is whether the child process we spawned is still alive. A flag
//! this app flips itself goes stale the moment the user closes the viewer, and
//! the next Space then silently "closes" a panel that is already gone.

use makepad_widgets::*;
use makepad_wm_api::{viewer_for, WmEvent, WmRequest};

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use std::process::{Child, Command};

/// Resolve a sibling binary of the running executable, the way wm resolves
/// its clients.
#[cfg(not(target_arch = "wasm32"))]
pub fn sibling_bin(bin: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut path = exe.parent()?.join(bin);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path.exists().then_some(path)
}

#[cfg(target_arch = "wasm32")]
pub fn sibling_bin(_bin: &str) -> Option<PathBuf> {
    None
}

/// What came of a Quick Look request.
pub enum Preview {
    /// A viewer window is showing the file; the status line to say so.
    Shown(String),
    /// No viewer could show it — the caller may fall back to its own panel.
    NoViewer(String),
}

/// The one preview this window has open, if any.
#[derive(Default)]
pub struct PreviewHost {
    /// Only set when *we* spawned it; hosted, the compositor owns the float.
    #[cfg(not(target_arch = "wasm32"))]
    child: Option<Child>,
    path: Option<PathBuf>,
    /// What the window manager says its Quick Look panel is showing. Only the
    /// WM's own events write this.
    hosted: Option<PathBuf>,
}

impl PreviewHost {
    /// The file we can *prove* is still previewed: one we spawned ourselves
    /// and whose process is still alive. Hosted, this stays `None` — see
    /// [`Self::hosted_showing`].
    pub fn showing(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What the WM's Quick Look panel is showing, as the WM last reported it.
    /// This is set by [`Self::on_wm_event`] and by nothing else, which is what
    /// keeps it from going stale.
    pub fn hosted_showing(&self) -> Option<&Path> {
        self.hosted.as_deref()
    }

    /// The panel's state, from the window manager. Everything the app does
    /// about previews follows from this rather than from what it last asked
    /// for.
    pub fn on_wm_event(&mut self, event: &WmEvent) -> bool {
        match event {
            WmEvent::PreviewShown { path } => {
                self.hosted = Some(PathBuf::from(path));
                true
            }
            WmEvent::PreviewHidden => {
                self.hosted = None;
                true
            }
            _ => false,
        }
    }

    /// Point an already-open panel at another file, by the real file behind
    /// its name. Nothing happens unless a
    /// panel is open, which is what makes it safe to call on every selection
    /// change — that is how arrow keys dial through previews.
    pub fn retarget(&mut self, cx: &Cx, path: &Path) -> bool {
        let fs = crate::vfs::vfs();
        if self.hosted.is_none() || fs.is_demo() || fs.is_dir(path) {
            return false;
        }
        makepad_wm_api::preview(cx, &fs.real_path(path))
    }

    /// Quick Look `path` in its associated viewer. External viewers are a
    /// RealVfs-only integration; demo files fall back to the in-app preview.
    pub fn open(&mut self, cx: &Cx, path: &Path) -> Preview {
        let name = crate::model::display_name(path);
        if crate::vfs::vfs().is_demo() {
            return Preview::NoViewer("External previews are not in this demo".to_string());
        }
        let app = viewer_for(path);
        let real = crate::vfs::vfs().real_path(path);
        let path = real.as_path();
        if makepad_wm_api::hosted(cx) {
            // No `close` first: the WM keeps the viewer warm and retargets it,
            // so hiding the panel a frame before showing it again would only
            // make it blink. The panel's state arrives as `PreviewShown`.
            if makepad_wm_api::preview(cx, path) {
                return Preview::Shown(format!(
                    "Previewing {} in {} — arrow keys dial through, Space or Esc closes",
                    name, app
                ));
            }
            return Preview::NoViewer(format!("The window manager could not preview {}", name));
        }
        self.close(cx);
        let Some(bin) = sibling_bin(app) else {
            return Preview::NoViewer(format!("{} is not built — no preview for {}", app, name));
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            match Command::new(&bin).arg("--preview").arg(path).spawn() {
                Ok(child) => {
                    self.child = Some(child);
                    self.path = Some(path.to_path_buf());
                    Preview::Shown(format!("Previewing {} in {} — Space or Esc to close", name, app))
                }
                Err(error) => Preview::NoViewer(format!("Could not preview {}: {}", name, error)),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (bin, path, app);
            Preview::NoViewer("External previews are not in this demo".to_string())
        }
    }

    /// Dismiss the preview. Hosted this only *asks*: the panel is closed when
    /// `PreviewHidden` says it is, never because we assumed so.
    pub fn close(&mut self, cx: &Cx) {
        if self.hosted.is_some() {
            makepad_wm_api::send(cx, &WmRequest::PreviewClose);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.path = None;
    }

    /// Forget a preview whose window the user already closed. A viewer exiting
    /// wakes nothing in this process, so this has to be asked *before* any
    /// decision that depends on the answer — not only when a signal happens to
    /// arrive.
    pub fn poll(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(child) = self.child.as_mut() {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    self.child = None;
                    self.path = None;
                }
            }
        }
    }
}

/// Open a file for real (not as a preview): a new tile when hosted, a sibling
/// process standalone, and the desktop's own opener when neither can.
pub fn open_file(cx: &Cx, path: &Path) -> String {
    let name = crate::model::display_name(path);
    if crate::vfs::vfs().is_demo() {
        return format!("Opening {name} is not in this demo");
    }
    let app = viewer_for(path);
    let real = crate::vfs::vfs().real_path(path);
    let path = real.as_path();
    if makepad_wm_api::open(cx, path) {
        return format!("Opening {} in {}", name, app);
    }
    match os_open(path) {
        Ok(()) => format!("Opening {}", name),
        Err(error) => format!("Could not open {}: {}", name, error),
    }
}

/// Open `path` in one *named* app, the way the Open With submenu means it. An
/// empty `app` is the desktop's own opener — the honest last resort when none
/// of ours claims the file.
pub fn open_file_with(cx: &Cx, path: &Path, app: &str) -> String {
    let name = crate::model::display_name(path);
    if crate::vfs::vfs().is_demo() {
        return format!("Open With for {name} is not in this demo");
    }
    let real = crate::vfs::vfs().real_path(path);
    let path = real.as_path();
    if app.is_empty() {
        return match os_open(path) {
            Ok(()) => format!("Opening {name} with the desktop default"),
            Err(error) => format!("Could not open {name}: {error}"),
        };
    }
    if makepad_wm_api::hosted(cx) {
        let request = makepad_wm_api::WmRequest::Open {
            app: Some(app.to_string()),
            path: path.display().to_string(),
        };
        if makepad_wm_api::send(cx, &request) {
            return format!("Opening {name} in {app}");
        }
    }
    let Some(bin) = sibling_bin(app) else {
        return format!("{app} is not built");
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        match Command::new(&bin).arg(path).spawn() {
            Ok(_) => format!("Opening {name} in {app}"),
            Err(error) => format!("Could not start {app}: {error}"),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (bin, path);
        format!("Open With for {name} is not in this demo")
    }
}

/// True when a sibling app of this name can actually be run — what the Open
/// With submenu offers is only ever what exists.
pub fn app_available(cx: &Cx, app: &str) -> bool {
    !crate::vfs::vfs().is_demo() && (makepad_wm_api::hosted(cx) || sibling_bin(app).is_some())
}

#[cfg(not(target_arch = "wasm32"))]
fn os_open(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "opening files is supported on macOS and Linux",
        ))
    }
}

#[cfg(target_arch = "wasm32")]
fn os_open(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening files is not in this demo",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_wm_api::WmRequest;

    #[test]
    fn the_shared_table_picks_the_viewer() {
        // files keeps no association table of its own: every kind it shows
        // is routed by makepad_wm_api, and the kinds it thumbnails are exactly the
        // ones the picture and video viewers claim.
        for ext in crate::model::IMAGE_EXTS {
            assert_eq!(viewer_for(&PathBuf::from(format!("/a/x.{ext}"))), "image");
        }
        for ext in crate::model::PLAYABLE_VIDEO_EXTS {
            assert_eq!(viewer_for(&PathBuf::from(format!("/a/x.{ext}"))), "video");
        }
        // Text and code fall to the terminal's pager, not to nothing.
        assert_eq!(viewer_for(Path::new("/a/m.rs")), "terminal");
        assert_eq!(viewer_for(Path::new("/a/n.txt")), "terminal");
        // Every type this browser names in its Kind column has an owner, and
        // none of them is decided in this crate.
        assert_eq!(viewer_for(Path::new("/a/d.pdf")), "pdf");
        assert_eq!(viewer_for(Path::new("/a/t.csv")), "sheets");
        assert_eq!(viewer_for(Path::new("/a/p.html")), "browser");
    }

    #[test]
    fn the_panels_state_comes_from_the_window_manager() {
        let mut host = PreviewHost::default();
        assert!(host.hosted_showing().is_none());
        // Asking for a preview proves nothing; only the WM saying so does.
        assert!(host.on_wm_event(&WmEvent::PreviewShown {
            path: "/a/x.png".to_string()
        }));
        assert_eq!(host.hosted_showing(), Some(Path::new("/a/x.png")));
        // Dialing to another file is the WM telling us again.
        host.on_wm_event(&WmEvent::PreviewShown {
            path: "/a/y.png".to_string(),
        });
        assert_eq!(host.hosted_showing(), Some(Path::new("/a/y.png")));
        assert!(host.on_wm_event(&WmEvent::PreviewHidden));
        assert!(host.hosted_showing().is_none());
        // Events that are not about the panel leave it alone.
        assert!(!host.on_wm_event(&WmEvent::Focus { focused: true }));
        // And the standalone half is a different question entirely.
        assert!(host.showing().is_none());
    }

    #[test]
    fn requests_carry_the_path_the_wm_reads_back() {
        // A quote or a backslash in a filename must survive the envelope.
        let req = WmRequest::Preview {
            app: None,
            path: "/q\"uote\\x.png".to_string(),
        };
        assert_eq!(WmRequest::parse(&req.to_json()), Some(req));
    }
}
