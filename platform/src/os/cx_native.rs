use {
    crate::cx::Cx,
    std::{
        fs::File,
        io::prelude::*,
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

/// A compile-on-device installation keeps resources in its downloaded sources.
/// The adjacent map contains `crate_name<TAB>relative/source/directory` rows;
/// neither resource lookup nor the map depends on the original install path.
/// Read once when the `Cx` is created and owned by it (`Cx::package_paths`).
pub(crate) fn load_package_paths() -> std::collections::HashMap<String, PathBuf> {
    use std::collections::HashMap;
    use std::path::Component;
    {
        let mut packages = HashMap::new();
        let Some(root) = exe_dir() else { return packages };
        let own_map = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| root.join(format!("{}.makepad-package-paths", n.to_string_lossy()))));
        let text = own_map.and_then(|p| std::fs::read_to_string(p).ok()).or_else(|| std::fs::read_to_string(root.join("makepad-package-paths")).ok());
        let Some(text) = text else { return packages };
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
}

/// `crate_name/rest` resolved through the package map, when the crate is in it.
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

/// Reads a file at `path`, falling back to the same path resolved against the executable's
/// directory. Returns `None` when neither location holds a readable file.
pub fn read_file_cwd_or_exe_relative(
    packages: &std::collections::HashMap<String, PathBuf>,
    path: impl AsRef<Path>,
) -> Option<Vec<u8>> {
    fn read(path: &Path) -> Option<Vec<u8>> {
        let mut buffer = Vec::<u8>::new();
        File::open(path).ok()?.read_to_end(&mut buffer).ok()?;
        Some(buffer)
    }
    let path = path.as_ref();
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
        match self.task_pool().submit_internal(crate::thread::Lane::Heavy, move || {
            let response = crate::storage::native::execute(&crate::home::storage_dir(), request);
            let _ = sender.send(response);
        }) {
            Ok(task) => task.detach(),
            Err(error) => crate::error!("storage request refused by the task pool: {error}"),
        }
    }

    pub fn native_load_dependencies(&mut self) {
        for (path, dep) in &mut self.dependencies {
            if let Some(buffer) = read_file_cwd_or_exe_relative(&self.package_paths, path) {
                dep.data = Some(Ok(Rc::new(buffer)));
            } else {
                println!("Could not load resource {}", path);
                dep.data = Some(Err(format!("Could not read resource {}", path)));
            }
        }
    }

    pub fn time_now() -> f64 {
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
