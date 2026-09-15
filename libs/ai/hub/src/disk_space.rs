//! Native worker download admission. Installed models need no download budget;
//! missing artifacts share one budget per destination volume. Re-evaluate on
//! every advertisement/admission so cleaning a disk restores eligibility.
use crate::download::{converted_file_is_verified, part_path, source_file_is_verified};
use crate::error::AssetAiError;
use crate::registry::ModelSpec;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const RESERVE: u64 = 1024 * 1024 * 1024;
pub(crate) const STREAM_WINDOW: u64 = 64 * 1024 * 1024;

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map_or(0, |m| m.len())
}

fn remaining(total: Option<u64>, part: &Path) -> u64 {
    match total {
        // Oversized partials are not valid resume prefixes.
        Some(total) => {
            let prefix = file_len(part);
            total.saturating_sub(if prefix <= total { prefix } else { 0 })
        }
        // Legacy manifests learn their actual size from HTTP headers. Admit
        // only with headroom, then check the full length before writing.
        None => STREAM_WINDOW,
    }
}

fn requirements(spec: &ModelSpec, cache: &Path) -> BTreeMap<PathBuf, u64> {
    let mut paths = BTreeMap::<PathBuf, u64>::new();
    for file in spec.files.iter().filter(|f| !f.optional) {
        let converted = file.converted_path(cache);
        if converted.as_ref().is_some_and(|path| {
            if file.conversion.is_some() { converted_file_is_verified(file, cache) }
            else { path.is_file() }
        }) {
            continue;
        }
        // Conversion writes a separate artifact while the source remains.
        if let (Some(path), Some(conversion)) = (converted, &file.conversion) {
            paths.entry(path).and_modify(|n| *n = (*n).max(conversion.size))
                .or_insert(conversion.size);
        }
        let dest = file.dest_path(cache);
        // A file with no size or digest has no content identity to check. The
        // downloader accepts an existing file (and records its revision in a
        // receipt) even when the registry pins only a revision, so do not
        // reserve an arbitrary stream window for an artifact that will not be
        // downloaded.
        if source_file_is_verified(file, cache)
            || (file.sha256.is_none() && file.size.is_none() && dest.is_file())
        {
            continue;
        }
        let part = part_path(&dest);
        let bytes = remaining(file.size, &part);
        paths.entry(part).and_modify(|n| *n = (*n).max(bytes)).or_insert(bytes);
    }
    paths
}

fn refuse(path: &Path, needed: u64, free: u64, subject: &str) -> AssetAiError {
    AssetAiError::Unavailable(format!(
        "disk-space: insufficient for {subject} on {}: {:.2} GiB free, {:.2} GiB additional required plus 1 GiB reserve",
        path.display(), free as f64 / RESERVE as f64, needed as f64 / RESERVE as f64,
    ))
}

fn check_requirements(
    paths: BTreeMap<PathBuf, u64>, subject: &str,
    mut probe: impl FnMut(&Path) -> std::io::Result<crate::disk_volume::Volume>,
) -> Result<(), AssetAiError> {
    let mut volumes = BTreeMap::<String, (PathBuf, u64, u64)>::new();
    for (path, bytes) in paths {
        let volume = probe(&path).map_err(|error| AssetAiError::Unavailable(format!(
            "disk-space: cannot check {} for {subject}: {error}", path.display()
        )))?;
        let entry = volumes.entry(volume.key).or_insert((volume.path, volume.available, 0));
        // Free space can decrease between probes; use the lowest observation.
        entry.1 = entry.1.min(volume.available);
        entry.2 = entry.2.saturating_add(bytes);
    }
    for (_, (path, free, needed)) in volumes {
        if free < needed.saturating_add(RESERVE) {
            return Err(refuse(&path, needed, free, subject));
        }
    }
    Ok(())
}

pub(crate) fn check_model(spec: &ModelSpec, cache: &Path) -> Result<(), AssetAiError> {
    check_requirements(requirements(spec, cache), &spec.id, crate::disk_volume::for_path)
}

/// Called before peer/HTTP writes, including Range-ignored restarts. Existing
/// partial bytes occupy disk now and will either be appended to or reclaimed
/// by truncation, so both cases need the same additional disk budget.
pub(crate) fn check_download(part: &Path, total: Option<u64>) -> Result<(), AssetAiError> {
    let mut paths = BTreeMap::new();
    paths.insert(part.to_path_buf(), remaining(total, part));
    check_requirements(paths, &part.display().to_string(), crate::disk_volume::for_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Domain, FileSpec, ConversionSpec};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!("hub-disk-{}-{}", std::process::id(), ID.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&p).unwrap(); Self(p)
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn file(path: &str, size: Option<u64>) -> FileSpec {
        FileSpec { role:None, repo:"fixture".into(),path:path.into(),revision:None,cache_as:path.into(),size,
            sha256:None,local:false,optional:false,converts_to:None,conversion:None }
    }
    fn spec(files: Vec<FileSpec>) -> ModelSpec {
        ModelSpec { id:"fixture".into(),domain:Domain::Video,backend:"h3".into(),available:true,gated:false,
            vram_gb:None,min_vram_gb:None,min_compute_cap:None,note:None,license:None,files }
    }
    fn check(paths: BTreeMap<PathBuf,u64>, free:u64) -> Result<(),AssetAiError> {
        check_requirements(paths,"fixture", |_| Ok(crate::disk_volume::Volume {
            key:"one-volume".into(),path:PathBuf::from("cache"),available:free,
        }))
    }
    #[test]
    fn aggregate_missing_downloads_counts_resume_and_deduplicates_shared_files() {
        let temp=Temp::new();
        std::fs::write(temp.0.join("a.part"), [0u8;40]).unwrap();
        let model=spec(vec![file("a",Some(100)),file("b",Some(70)),file("a",Some(100))]);
        let paths=requirements(&model,&temp.0);
        assert_eq!(paths.values().sum::<u64>(),130);
        assert!(check(paths.clone(),RESERVE+129).is_err());
        assert!(check(paths,RESERVE+130).is_ok());
    }
    #[test]
    fn cached_and_optional_files_do_not_require_free_disk() {
        let temp=Temp::new();
        std::fs::write(temp.0.join("cached"),b"legacy").unwrap();
        let mut optional=file("optional",Some(u64::MAX)); optional.optional=true;
        let paths=requirements(&spec(vec![file("cached",None),optional]),&temp.0);
        assert!(paths.is_empty()); assert!(check(paths,0).is_ok());
    }
    #[test]
    fn conversion_budgets_source_and_output_together() {
        let temp=Temp::new(); let mut f=file("source",Some(100));
        f.conversion=Some(ConversionSpec {cache_as:"converted".into(),size:200,sha256:"a".repeat(64),converter_id:"fixture".into(),converter_version:"1".into()});
        let paths=requirements(&spec(vec![f]),&temp.0);
        assert_eq!(paths.values().sum::<u64>(),300);
        assert!(check(paths,RESERVE+299).is_err());
    }
    #[test]
    fn separate_volumes_have_separate_budgets_and_unknown_sizes_need_headroom() {
        let temp=Temp::new();
        let paths=requirements(&spec(vec![file("a",Some(100)),file("b",None)]),&temp.0);
        assert_eq!(paths[&temp.0.join("b.part")],STREAM_WINDOW);
        assert!(check_requirements(paths,"fixture",|p| Ok(crate::disk_volume::Volume {
            key:p.display().to_string(),path:p.into(),available:RESERVE+STREAM_WINDOW,
        })).is_ok());
    }
    #[test]
    fn failed_volume_query_refuses_and_clearing_space_restores_admission() {
        let temp=Temp::new(); let paths=requirements(&spec(vec![file("a",Some(100))]),&temp.0);
        assert!(check_requirements(paths.clone(),"fixture",|_| Err(std::io::ErrorKind::PermissionDenied.into())).is_err());
        assert!(check(paths.clone(),RESERVE).is_err());
        assert!(check(paths,RESERVE+100).is_ok());
    }

    #[test]
    fn enormous_download_is_refused_before_network_and_keeps_partial() {
        let temp = Temp::new();
        let file = file("huge", Some(u64::MAX));
        let destination = file.dest_path(&temp.0);
        let part = part_path(&destination);
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, [7u8; 32]).unwrap();
        let downloader = crate::download::Downloader::new("http://127.0.0.1:1", None).unwrap();
        let mut progress = |_| {};
        let error = downloader
            .ensure_file(&file, &temp.0, &mut progress, &crate::backend::CancelToken::new())
            .unwrap_err();
        assert!(matches!(error, AssetAiError::Unavailable(message) if message.starts_with("disk-space:")));
        assert_eq!(std::fs::read(&part).unwrap(), [7u8; 32]);
    }

    #[test]
    fn existing_unknown_length_file_does_not_reserve_stream_window() {
        let temp = Temp::new();
        let mut model_file = file("legacy", None);
        model_file.revision = Some("immutable-revision".into());
        std::fs::write(model_file.dest_path(&temp.0), b"already-installed").unwrap();
        assert!(requirements(&spec(vec![model_file]), &temp.0).is_empty());
    }
}
