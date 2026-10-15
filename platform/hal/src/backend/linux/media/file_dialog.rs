//! Desktop dialog helpers, ported from native Linux. Linux did not have a D-Bus portal
//! client: keep its KDE/GTK helper selection and argument contract. One session-owned child
//! per pending request; poll stdout without blocking, and reap children on session teardown.
use crate::types::dialog_kind;
use std::{
    io::Read,
    os::fd::AsRawFd,
    path::PathBuf,
    process::{Child, Command, Stdio},
};
struct Filter {
    description: String,
    extensions: Vec<String>,
}
struct Settings {
    title: Option<String>,
    location: Option<PathBuf>,
    filename: Option<String>,
    multiple: bool,
    filters: Vec<Filter>,
}
pub struct Dialog {
    pub request: u32,
    child: Child,
    bytes: Vec<u8>,
}
impl Dialog {
    pub fn new(
        request: u32,
        kind: u32,
        title: String,
        location: String,
        filename: String,
        filters: Vec<String>,
    ) -> Result<Self, String> {
        let helper = find_helper()
            .ok_or("No desktop file dialog helper (zenity, qarma, matedialog or kdialog)")?;
        let mode = match kind & 0xff {
            dialog_kind::SELECT_FILE => Mode::OpenFile,
            dialog_kind::SAVE_FILE => Mode::SaveFile,
            dialog_kind::SELECT_FOLDER => Mode::OpenFolder,
            dialog_kind::SAVE_FOLDER => Mode::SaveFolder,
            _ => return Err("Unknown dialog kind".into()),
        };
        let settings = Settings {
            title: (!title.is_empty()).then_some(title),
            location: (!location.is_empty()).then(|| PathBuf::from(location)),
            filename: (!filename.is_empty()).then_some(filename),
            multiple: kind & dialog_kind::MULTIPLE != 0,
            filters: filters
                .chunks(2)
                .map(|p| Filter {
                    description: p[0].clone(),
                    extensions: p
                        .get(1)
                        .map(|s| s.split(',').map(str::to_owned).collect())
                        .unwrap_or_default(),
                })
                .collect(),
        };
        let mut command = Command::new(helper.binary());
        if helper.is_zenity_like() {
            build_zenity(&mut command, &settings, mode)
        } else {
            build_kdialog(&mut command, &settings, mode)
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let fd = child
            .stdout
            .as_ref()
            .ok_or("dialog stdout missing")?
            .as_raw_fd();
        let flags = unsafe { super::libc_sys::fcntl(fd, super::libc_sys::F_GETFL) };
        if flags < 0
            || unsafe {
                super::libc_sys::fcntl(
                    fd,
                    super::libc_sys::F_SETFL,
                    flags | super::libc_sys::O_NONBLOCK,
                )
            } < 0
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self {
            request,
            child,
            bytes: Vec::new(),
        })
    }
    fn read_output(&mut self) {
        if let Some(pipe) = self.child.stdout.as_mut() {
            let mut buf = [0; 4096];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => self.bytes.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        eprintln!("[hal-dialog] stdout: {e}");
                        break;
                    }
                }
            }
        }
    }
    pub fn poll(&mut self) -> Option<Vec<String>> {
        self.read_output();
        match self.child.try_wait() {
            Ok(Some(status)) => Some(if status.success() {
                // The child may have written after our first nonblocking drain.
                self.read_output();
                String::from_utf8_lossy(&self.bytes)
                    .lines()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            } else {
                Vec::new()
            }),
            Ok(None) => None,
            Err(e) => {
                eprintln!("[hal-dialog] wait: {e}");
                Some(Vec::new())
            }
        }
    }
}
impl Drop for Dialog {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
/// Which helper we found, in preference order. Zenity first: it is the most
/// widely installed and its `--separator` makes multi-select unambiguous.
#[derive(Clone, Copy, PartialEq)]
enum Helper {
    Zenity,
    Qarma,
    MateDialog,
    KDialog,
}

impl Helper {
    fn binary(self) -> &'static str {
        match self {
            Helper::Zenity => "zenity",
            Helper::Qarma => "qarma",
            Helper::MateDialog => "matedialog",
            Helper::KDialog => "kdialog",
        }
    }

    /// Zenity's command line, which qarma and matedialog clone exactly.
    fn is_zenity_like(self) -> bool {
        !matches!(self, Helper::KDialog)
    }
}

/// Look for a helper on PATH. KDE first when the session says KDE, so a
/// Plasma user gets Plasma's dialog even with zenity installed.
fn find_helper() -> Option<Helper> {
    let kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_ascii_lowercase().contains("kde"))
        .unwrap_or(false);
    let order = if kde {
        [
            Helper::KDialog,
            Helper::Zenity,
            Helper::Qarma,
            Helper::MateDialog,
        ]
    } else {
        [
            Helper::Zenity,
            Helper::Qarma,
            Helper::MateDialog,
            Helper::KDialog,
        ]
    };
    order.into_iter().find(|helper| on_path(helper.binary()))
}

fn on_path(binary: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())
}

/// What the caller wants a path for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    OpenFile,
    SaveFile,
    OpenFolder,
    SaveFolder,
}

fn build_zenity(command: &mut Command, settings: &Settings, mode: Mode) {
    command.arg("--file-selection");
    if let Some(title) = &settings.title {
        command.arg(format!("--title={title}"));
    }
    match mode {
        Mode::OpenFile => {
            if settings.multiple {
                command.arg("--multiple");
                // Newline, so a path containing the default `|` cannot
                // split one answer into two.
                command.arg("--separator=\n");
            }
        }
        Mode::SaveFile => {
            command.arg("--save");
            command.arg("--confirm-overwrite");
        }
        Mode::OpenFolder => {
            command.arg("--directory");
        }
        Mode::SaveFolder => {
            command.arg("--directory");
            command.arg("--save");
        }
    }
    // Zenity takes the start location and the suggested name in one
    // argument; a trailing slash is what marks it as a directory.
    let mut start = settings
        .location
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !start.is_empty() && !start.ends_with('/') {
        start.push('/');
    }
    if let Some(filename) = &settings.filename {
        start.push_str(filename);
    }
    if !start.is_empty() {
        command.arg(format!("--filename={start}"));
    }
    if matches!(mode, Mode::OpenFile | Mode::SaveFile) {
        for filter in &settings.filters {
            let patterns = filter
                .extensions
                .iter()
                .map(|e| glob_for(e))
                .collect::<Vec<_>>()
                .join(" ");
            command.arg(format!(
                "--file-filter={} | {}",
                filter.description, patterns
            ));
        }
    }
}

fn build_kdialog(command: &mut Command, settings: &Settings, mode: Mode) {
    let start = settings
        .location
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    match mode {
        Mode::OpenFile => {
            command
                .arg("--getopenfilename")
                .arg(&start)
                .arg(kdialog_filter(settings));
            if settings.multiple {
                command.arg("--multiple").arg("--separate-output");
            }
        }
        Mode::SaveFile => {
            let mut start = start;
            if let Some(filename) = &settings.filename {
                if !start.ends_with('/') {
                    start.push('/');
                }
                start.push_str(filename);
            }
            command
                .arg("--getsavefilename")
                .arg(&start)
                .arg(kdialog_filter(settings));
        }
        Mode::OpenFolder | Mode::SaveFolder => {
            command.arg("--getexistingdirectory").arg(&start);
        }
    }
    if let Some(title) = &settings.title {
        command.arg("--title").arg(title);
    }
}

/// kdialog wants one filter string: `*.mp4 *.mkv|Video`, groups joined by
/// newlines. An empty filter means "everything", which is its default.
fn kdialog_filter(settings: &Settings) -> String {
    settings
        .filters
        .iter()
        .map(|filter| {
            let patterns = filter
                .extensions
                .iter()
                .map(|e| glob_for(e))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{patterns}|{}", filter.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `mp4` / `.mp4` / `*.mp4` all mean the same glob; `*` means everything.
fn glob_for(extension: &str) -> String {
    let cleaned = extension
        .trim()
        .trim_start_matches('*')
        .trim_start_matches('.');
    if cleaned.is_empty() || extension.trim() == "*" {
        "*".to_string()
    } else {
        format!("*.{cleaned}")
    }
}
