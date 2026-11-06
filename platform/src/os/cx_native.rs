#[cfg(not(target_os = "android"))]
use std::{fs::File, io::Read};
use {
    crate::cx::Cx,
    std::{
        path::{Path, PathBuf},
        rc::Rc,
        sync::OnceLock,
        time::{Instant, SystemTime},
    },
};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EventFlow {
    Poll,
    Wait,
    Exit,
}

/// The directory holding the running executable, queried once.
fn exe_dir() -> Option<&'static Path> {
    static EXE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE_DIR
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
        })
        .as_deref()
}

/// Resolves a relative resource path against the directory holding the executable.
///
/// Packaged desktop layouts ship resources beside the executable and address them through a
/// relative package root, which a plain relative open resolves against the process working
/// directory instead. Any launcher that does not set a working directory — a URL-protocol
/// handler, a file association, a service, a shortcut without one — then starts the app in an
/// unrelated directory and every resource open fails, leaving a window that draws its shapes
/// but has no fonts, icons or images. Callers retry through here so the executable's own
/// directory is searched as well. Returns `None` for an absolute path (already anchored) and
/// when the executable path is unavailable.
pub fn exe_relative_path(rel: impl AsRef<Path>) -> Option<PathBuf> {
    let rel = rel.as_ref();
    if rel.is_absolute() {
        return None;
    }
    Some(exe_dir()?.join(rel))
}

/// Desktop bundles and compile-on-device installations map crates to resource directories.
/// The adjacent map contains `crate_name<TAB>relative/source/directory` rows;
/// neither resource lookup nor the map depends on the original install path.
/// Read once when the `Cx` is created and owned by it (`Cx::package_paths`).
pub(crate) fn load_package_paths() -> std::collections::HashMap<String, PathBuf> {
    let Some(root) = exe_dir() else { return Default::default() };
    // The app's own map beside it. Rows are relative to the executable's folder.
    let exe_name = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let text = exe_name.and_then(|n| std::fs::read_to_string(root.join(format!("{}.makepad-package-paths", n))).ok());
    text.map(|text| parse_package_paths(root, &text)).unwrap_or_default()
}

/// Parse package directories relative to the directory containing the executable.
fn parse_package_paths(root: &Path, text: &str) -> std::collections::HashMap<String, PathBuf> {
    use std::path::Component;
    let mut packages = std::collections::HashMap::new();
    for line in text.lines() {
        let Some((name, relative)) = line.split_once('\t') else { continue };
        let relative = Path::new(relative);
        if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            && !relative.as_os_str().is_empty()
            && relative.components().all(|c| matches!(c, Component::Normal(_)))
        {
            packages.insert(name.to_owned(), root.join(relative));
        }
    }
    packages
}

/// `crate_name/rest` resolved through the package map, when the crate is in it.
#[cfg(not(target_os = "android"))]
fn source_resource_path(
    packages: &std::collections::HashMap<String, PathBuf>,
    path: &Path,
) -> Option<PathBuf> {
    use std::path::Component;
    let mut components = path.components().filter(|c| !matches!(c, Component::CurDir));
    let Component::Normal(name) = components.next()? else { return None };
    let mut resolved = packages.get(name.to_str()?)?.clone();
    for component in components {
        let Component::Normal(part) = component else { return None };
        resolved.push(part);
    }
    Some(resolved)
}

/// A mapped crate uses only its package directory; other crates use the package root.
#[cfg(all(
    not(any(target_os = "android", target_os = "ios", target_os = "tvos")),
    not(all(target_os = "macos", apple_bundle))
))]
pub(crate) fn packaged_resource_path(
    packages: &std::collections::HashMap<String, PathBuf>,
    package_root: &str,
    dep_path: &str,
) -> PathBuf {
    source_resource_path(packages, Path::new(dep_path))
        .unwrap_or_else(|| PathBuf::from(format!("{package_root}/{dep_path}")))
}

#[cfg(all(test, unix, not(any(target_os = "android", target_os = "ios", target_os = "tvos")), not(all(target_os = "macos", apple_bundle))))]
mod tests {
    use super::*;

    #[test]
    fn packaged_resource_aliases_resolve_in_desktop_bundles() {
        struct Bundle(PathBuf);
        impl Drop for Bundle {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let bundle = Bundle(std::env::temp_dir().join(format!(
            "makepad-package-paths-{}-{}",
            std::process::id(),
            crate::makepad_live_id::LiveId::unique().0,
        )));
        let macos = bundle.0.join("Contents/MacOS");
        let resources = bundle.0.join("Contents/Resources");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(resources.join("makepad_foo/resources/icons")).unwrap();
        std::os::unix::fs::symlink("../Resources", macos.join("resources")).unwrap();
        let file = resources.join("makepad_foo/resources/icons/x.svg");
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        std::fs::write(&file, bytes).unwrap();
        let map = "makepad_foo\tresources/makepad_foo\nfoo\tresources/makepad_foo\n";
        let packages = parse_package_paths(&macos, map);
        for name in ["foo", "makepad_foo"] {
            let path = packaged_resource_path(&packages, "resources", &format!("{name}/resources/icons/x.svg"));
            assert_eq!(path, macos.join("resources/makepad_foo/resources/icons/x.svg"), "{name} must use its mapped package directory");
            assert_eq!(std::fs::canonicalize(&path).unwrap(), std::fs::canonicalize(&file).unwrap(), "{name} must resolve through the bundle symlink");
            assert_eq!(read_file_cwd_or_exe_relative(&packages, &path).unwrap(), bytes, "{name} must read the packaged bytes");
        }
        assert_eq!(packaged_resource_path(&packages, "resources", "other/resources/x.svg"), PathBuf::from("resources/other/resources/x.svg"));
        assert_eq!(packaged_resource_path(&Default::default(), "resources", "foo/resources/icons/x.svg"), PathBuf::from("resources/foo/resources/icons/x.svg"));

        // The folder layout uses the same map without the macOS symlink.
        let folder = bundle.0.join("folder");
        std::fs::create_dir_all(folder.join("resources/makepad_foo/resources/icons")).unwrap();
        std::fs::write(folder.join("resources/makepad_foo/resources/icons/x.svg"), bytes).unwrap();
        let packages = parse_package_paths(&folder, map);
        let path = packaged_resource_path(&packages, "resources", "foo/resources/icons/x.svg");
        assert_eq!(read_file_cwd_or_exe_relative(&packages, &path).unwrap(), bytes);

        // A missing mapped file must not load a copy under the package root.
        std::fs::create_dir_all(folder.join("resources/foo/resources/icons")).unwrap();
        std::fs::write(folder.join("resources/foo/resources/icons/missing.svg"), bytes).unwrap();
        let path = packaged_resource_path(&packages, folder.join("resources").to_str().unwrap(), "foo/resources/icons/missing.svg");
        assert!(read_file_cwd_or_exe_relative(&packages, path).is_none());
    }
}

/// Reads a file at `path`, falling back to the same path resolved against the executable's
/// directory. Returns `None` when neither location holds a readable file.
pub fn read_file_cwd_or_exe_relative(
    packages: &std::collections::HashMap<String, PathBuf>,
    path: impl AsRef<Path>,
) -> Option<Vec<u8>> {
    #[cfg(not(target_os = "android"))]
    fn read(path: &Path) -> Option<Vec<u8>> {
        let mut buffer = Vec::<u8>::new();
        File::open(path).ok()?.read_to_end(&mut buffer).ok()?;
        Some(buffer)
    }
    let path = path.as_ref();
    #[cfg(target_os = "android")]
    {
        let _ = packages;
        return crate::os::hal::read_asset(path.to_str()?);
    }
    #[cfg(not(target_os = "android"))]
    source_resource_path(packages, path).and_then(|p| read(&p))
        .or_else(|| read(path)).or_else(|| read(&exe_relative_path(path)?))
}

// lets start a websocket thread

impl Cx {
    pub(crate) fn start_native_storage_request(
        &mut self,
        request: crate::storage::StorageRequest,
    ) {
        let sender = self.storage_state.sender();
        let held = self.storage_state.hold(request);
        let root = self.storage_state.root.clone();
        match self.task_pool().submit_internal(crate::thread::Class::Background, move || {
            let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(request) = held.take() {
                let response = crate::storage::native::execute(&root, request);
                let _ = sender.send(response);
            }
        }) {
            Ok(task) => task.detach(),
            Err(error) => crate::error!("storage request refused by the task pool: {error}"),
        }
    }

    /// The app is ending and its pool is about to cancel what no worker took: every write it asked
    /// for reaches the disk first. One a worker runs is waited for and the rest run here; their
    /// answers have nobody left to go to.
    pub(crate) fn finish_native_storage_writes(&mut self) {
        for write in self.storage_state.take_writes() {
            let mut held = write.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(request) = held.take() {
                let _ = crate::storage::native::execute(&self.storage_state.root, request);
            }
        }
    }

    pub fn native_load_dependencies(&mut self) {
        for (path, dep) in &mut self.dependencies {
            if let Some(buffer) = read_file_cwd_or_exe_relative(&self.package_paths, path) {
                dep.data = Some(Ok(Rc::new(buffer)));
            } else {
                eprintln!("Could not load resource {}", path);
                dep.data = Some(Err(format!("Could not read resource {}", path)));
            }
        }
    }

    pub fn time_now() -> f64 {
        if let Some(time) = crate::remote::app_clock::epoch_now() {
            return time;
        }
        if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            return elapsed.as_secs_f64();
        }
        return 0.0;
    }

    pub fn monotonic_now() -> f64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64()
    }
}
