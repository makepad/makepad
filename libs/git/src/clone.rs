use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::commit::parse_commit;
use crate::error::GitError;
use crate::index::Index;
use crate::object::{read_loose_object, Object};
use crate::oid::ObjectId;
use crate::pack::{
    find_packs, read_pack_object, read_pack_object_from_data, PackIndex, PackLookup,
};
use crate::refs;
use crate::tree::parse_tree;

/// Timings for the various phases of a local clone.
#[derive(Debug, Clone)]
pub struct CloneTimings {
    pub resolve_ms: f64,
    pub setup_ms: f64,
    pub checkout_ms: f64,
    pub total_ms: f64,
    pub num_files: usize,
    pub bytes_written: u64,
    /// Time spent walking the tree to collect entries
    pub tree_walk_ms: f64,
    /// Time spent decompressing objects + writing files (parallel)
    pub parallel_ms: f64,
}

/// A file/symlink entry discovered during tree walk.
struct FileEntry {
    path: String,
    oid: ObjectId,
    mode: u32,
}

/// Thread-safe pack reader: pre-loaded pack data + immutable index metadata.
struct SharedPack {
    pack_data: Vec<u8>,
    fanout: [u32; 256],
    oids: Vec<ObjectId>,
    offsets: Vec<u64>,
}

impl PackLookup for SharedPack {
    fn find_offset(&self, oid: &ObjectId) -> Option<u64> {
        let first_byte = oid.as_bytes()[0] as usize;
        let start = if first_byte == 0 {
            0
        } else {
            self.fanout[first_byte - 1] as usize
        };
        let end = self.fanout[first_byte] as usize;
        let slice = &self.oids[start..end];
        match slice.binary_search_by(|probe| probe.as_bytes().cmp(oid.as_bytes())) {
            Ok(idx) => Some(self.offsets[start + idx]),
            Err(_) => None,
        }
    }
}

impl SharedPack {
    fn from_pack_index(idx: &PackIndex) -> Result<Self, GitError> {
        idx.ensure_loaded()?;
        let pack_data = idx.get_cached_data().ok_or_else(|| {
            GitError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "pack data not loaded",
            ))
        })?;
        Ok(SharedPack {
            pack_data,
            fanout: idx.fanout_copy(),
            oids: idx.oids_copy(),
            offsets: idx.offsets_copy(),
        })
    }

    fn read_object(&self, oid: &ObjectId) -> Result<Object, GitError> {
        if let Some(offset) = self.find_offset(oid) {
            read_pack_object_from_data(&self.pack_data, offset as usize, self)
        } else {
            Err(GitError::ObjectNotFound(oid.to_hex()))
        }
    }
}

// SharedPack has no interior mutability — all fields are plain data.
unsafe impl Send for SharedPack {}
unsafe impl Sync for SharedPack {}

/// Perform a local depth=1 clone using git alternates.
///
/// Strategy:
/// 1. Walk the tree to collect all file entries (sequential — trees are small)
/// 2. Create all directories
/// 3. Pre-load pack data into thread-safe shared readers
/// 4. Decompress blobs + write files in parallel across threads
pub fn local_clone_depth1(
    src: &Path,
    dst: &Path,
    branch: Option<&str>,
) -> Result<CloneTimings, GitError> {
    let total_start = Instant::now();

    // --- Phase 1: Resolve ref ---
    let resolve_start = Instant::now();

    let src_git_dir = src.join(".git");
    if !src_git_dir.is_dir() {
        return Err(GitError::InvalidRef(format!(
            "not a git repository: {}",
            src.display()
        )));
    }

    let (target_oid, ref_name) = if let Some(branch) = branch {
        let refname = format!("refs/heads/{}", branch);
        let oid = refs::resolve_ref(&src_git_dir, &refname)?;
        (oid, refname)
    } else {
        let oid = refs::resolve_head(&src_git_dir)?;
        let ref_name = match refs::read_head(&src_git_dir)? {
            refs::RefTarget::Symbolic(s) => s,
            refs::RefTarget::Direct(_) => "refs/heads/main".to_string(),
        };
        (oid, ref_name)
    };

    let src_packs = find_packs(&src_git_dir)?;
    let commit_obj = read_object_from(&src_git_dir, &src_packs, &target_oid)?;
    let commit = parse_commit(&commit_obj.data)?;

    let resolve_ms = resolve_start.elapsed().as_secs_f64() * 1000.0;

    // --- Phase 2: Setup .git with alternates ---
    let setup_start = Instant::now();

    let dst_git_dir = dst.join(".git");
    fs::create_dir_all(dst_git_dir.join("objects/info"))?;
    fs::create_dir_all(dst_git_dir.join("refs/heads"))?;

    let src_objects = src_git_dir
        .join("objects")
        .canonicalize()
        .map_err(GitError::Io)?;
    fs::write(
        dst_git_dir.join("objects/info/alternates"),
        format!("{}\n", src_objects.display()),
    )?;

    refs::update_head(&dst_git_dir, &refs::RefTarget::Symbolic(ref_name.clone()))?;
    refs::write_ref(&dst_git_dir, &ref_name, &target_oid)?;

    fs::write(
        dst_git_dir.join("shallow"),
        format!("{}\n", target_oid.to_hex()),
    )?;

    fs::write(
        dst_git_dir.join("config"),
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n",
    )?;

    let setup_ms = setup_start.elapsed().as_secs_f64() * 1000.0;

    // --- Phase 3: Walk tree to collect all entries + directories ---
    let walk_start = Instant::now();

    let tree = parse_tree(&read_object_from(&src_git_dir, &src_packs, &commit.tree)?.data)?;

    let mut file_entries = Vec::new();
    let mut dirs = Vec::new();
    collect_entries(
        &src_git_dir,
        &src_packs,
        &tree,
        "",
        &mut file_entries,
        &mut dirs,
    )?;

    let tree_walk_ms = walk_start.elapsed().as_secs_f64() * 1000.0;

    // --- Phase 4: Create all directories ---
    let workdir = dst.to_path_buf();
    for dir in &dirs {
        fs::create_dir_all(workdir.join(dir))?;
    }

    // --- Phase 5: Build thread-safe pack readers ---
    for pack in &src_packs {
        pack.ensure_loaded()?;
    }
    let shared_packs: Vec<SharedPack> = src_packs
        .iter()
        .filter_map(|p| SharedPack::from_pack_index(p).ok())
        .collect();
    let shared_packs = Arc::new(shared_packs);
    let src_git_dir_arc = Arc::new(src_git_dir.clone());

    // --- Phase 6: Parallel decompress + write ---
    let parallel_start = Instant::now();

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);

    // Partition file entries into chunks for each thread
    let total = file_entries.len();
    let chunk_size = (total + num_threads - 1) / num_threads;

    let mut chunks: Vec<Vec<FileEntry>> = Vec::with_capacity(num_threads);
    let mut iter = file_entries.into_iter();
    for _ in 0..num_threads {
        let chunk: Vec<FileEntry> = iter.by_ref().take(chunk_size).collect();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }

    let workdir_arc = Arc::new(workdir);
    let mut handles = Vec::new();

    for chunk in chunks {
        let packs = Arc::clone(&shared_packs);
        let wd = Arc::clone(&workdir_arc);
        let git_dir = Arc::clone(&src_git_dir_arc);

        handles.push(std::thread::spawn(
            move || -> Result<(Vec<crate::index::IndexEntry>, u64), GitError> {
                let mut entries = Vec::with_capacity(chunk.len());
                let mut bytes = 0u64;

                for fe in &chunk {
                    let file_path = wd.join(&fe.path);

                    if fe.mode == 0o120000 {
                        // Symlink
                        let blob = read_object_shared(&git_dir, &packs, &fe.oid)?;
                        #[cfg(unix)]
                        {
                            let target = std::str::from_utf8(&blob.data).unwrap_or("");
                            std::os::unix::fs::symlink(target, &file_path)?;
                        }
                        #[cfg(not(unix))]
                        fs::write(&file_path, &blob.data)?;
                        bytes += blob.data.len() as u64;
                    } else {
                        // Regular file
                        let blob = read_object_shared(&git_dir, &packs, &fe.oid)?;
                        fs::write(&file_path, &blob.data)?;
                        bytes += blob.data.len() as u64;

                        #[cfg(unix)]
                        if fe.mode == 0o100755 {
                            use std::os::unix::fs::PermissionsExt;
                            fs::set_permissions(&file_path, fs::Permissions::from_mode(0o755))?;
                        }
                    }

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        let metadata = if fe.mode == 0o120000 {
                            fs::symlink_metadata(&file_path)?
                        } else {
                            fs::metadata(&file_path)?
                        };
                        entries.push(crate::index::IndexEntry {
                            ctime_sec: metadata.mtime() as u32,
                            ctime_nsec: 0,
                            mtime_sec: metadata.mtime() as u32,
                            mtime_nsec: 0,
                            dev: metadata.dev() as u32,
                            ino: metadata.ino() as u32,
                            mode: fe.mode,
                            uid: metadata.uid(),
                            gid: metadata.gid(),
                            file_size: metadata.len() as u32,
                            oid: fe.oid,
                            flags: (fe.path.len().min(0xFFF)) as u16,
                            path: fe.path.clone(),
                        });
                    }
                    #[cfg(not(unix))]
                    {
                        entries.push(crate::index::IndexEntry {
                            ctime_sec: 0,
                            ctime_nsec: 0,
                            mtime_sec: 0,
                            mtime_nsec: 0,
                            dev: 0,
                            ino: 0,
                            mode: fe.mode,
                            uid: 0,
                            gid: 0,
                            file_size: 0,
                            oid: fe.oid,
                            flags: (fe.path.len().min(0xFFF)) as u16,
                            path: fe.path.clone(),
                        });
                    }
                }

                Ok((entries, bytes))
            },
        ));
    }

    // Collect results from all threads
    let mut index_entries = Vec::with_capacity(total);
    let mut bytes_written = 0u64;
    for handle in handles {
        let (entries, bytes) = handle.join().map_err(|_| {
            GitError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "thread panicked",
            ))
        })??;
        index_entries.extend(entries);
        bytes_written += bytes;
    }

    let parallel_ms = parallel_start.elapsed().as_secs_f64() * 1000.0;

    // Write index
    index_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let num_files = index_entries.len();
    let index = Index {
        version: 2,
        entries: index_entries,
    };
    crate::index::write_index(&dst_git_dir, &index)?;

    let checkout_ms = walk_start.elapsed().as_secs_f64() * 1000.0;
    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

    Ok(CloneTimings {
        resolve_ms,
        setup_ms,
        checkout_ms,
        total_ms,
        num_files,
        bytes_written,
        tree_walk_ms,
        parallel_ms,
    })
}

/// Recursively walk a tree and collect all file entries and directory paths.
fn collect_entries(
    git_dir: &Path,
    packs: &[PackIndex],
    tree: &crate::tree::Tree,
    prefix: &str,
    file_entries: &mut Vec<FileEntry>,
    dirs: &mut Vec<String>,
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}{}", prefix, entry.name)
        };

        if entry.is_tree() {
            dirs.push(path.clone());
            let sub_obj = read_object_from(git_dir, packs, &entry.oid)?;
            let sub_tree = parse_tree(&sub_obj.data)?;
            collect_entries(
                git_dir,
                packs,
                &sub_tree,
                &format!("{}/", path),
                file_entries,
                dirs,
            )?;
        } else if entry.is_gitlink() {
            // Submodule — skip
        } else {
            file_entries.push(FileEntry {
                path,
                oid: entry.oid,
                mode: entry.mode,
            });
        }
    }
    Ok(())
}

/// Read an object using thread-safe shared packs (for parallel checkout).
fn read_object_shared(
    git_dir: &Path,
    packs: &[SharedPack],
    oid: &ObjectId,
) -> Result<Object, GitError> {
    match read_loose_object(git_dir, oid) {
        Ok(obj) => return Ok(obj),
        Err(GitError::ObjectNotFound(_)) => {}
        Err(e) => return Err(e),
    }
    for pack in packs {
        if let Ok(obj) = pack.read_object(oid) {
            return Ok(obj);
        }
    }
    Err(GitError::ObjectNotFound(oid.to_hex()))
}

/// Read an object from a git dir, trying loose first then packs.
fn read_object_from(
    git_dir: &Path,
    packs: &[PackIndex],
    oid: &ObjectId,
) -> Result<Object, GitError> {
    match read_loose_object(git_dir, oid) {
        Ok(obj) => return Ok(obj),
        Err(GitError::ObjectNotFound(_)) => {}
        Err(e) => return Err(e),
    }
    for pack in packs {
        if let Some(offset) = pack.find_offset(oid) {
            return read_pack_object(&pack.pack_path, offset, pack);
        }
    }
    Err(GitError::ObjectNotFound(oid.to_hex()))
}
