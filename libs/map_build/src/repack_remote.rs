//! [`InPlaceHost`] for an archive on another machine, reached over ssh.
//!
//! The conversion runs locally against a sparse mirror directory: the root
//! index, every shard's leaf directory and the cross-shard blobs are fetched
//! first; then each original shard is downloaded whole (the next one while the
//! current one converts), converted and verified locally, and uploaded back
//! as `tiles-NNN.partial` (in the background while the next shard converts).
//! The remote side gets the same journal as a local in-place run: the
//! pending manifest lands before `tiles-NNN.partial` is renamed over the
//! original, so an interruption anywhere is completed or redone on the next
//! run. Every transfer is split into parallel ssh streams (one TCP
//! connection each: single streams cap far below the link), retried, and
//! checked end to end with SHA-256 on both sides.
//!
//! Remote requirements: a POSIX shell with dd (skip_bytes/seek_bytes),
//! truncate, stat -c, sha256sum, base64 and sync.

use crate::repack::{
    decode_manifest, manifest_path, remove_if_present, shard_path, stash_data_path,
    stash_index_path, write_atomic, InPlaceHost,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use crate::repack::ReadExactAt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const ATTEMPTS: u32 = 6;
const RANGE_BATCH: usize = 400;

#[derive(Clone)]
struct Remote {
    ssh: String,
    dir: String,
}

impl Remote {
    fn command(&self, script: &str) -> Command {
        let mut command = Command::new("ssh");
        command
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=6")
            .arg(&self.ssh)
            .arg(format!("set -e; cd '{}'; {script}", self.dir));
        command
    }

    /// Run a script, return stdout; retried with backoff.
    fn run(&self, script: &str) -> Result<Vec<u8>, String> {
        retry(&format!("ssh: {}", abbreviate(script)), || {
            let output = self
                .command(script)
                .stdin(Stdio::null())
                .output()
                .map_err(|err| format!("spawn ssh: {err}"))?;
            if output.status.success() {
                Ok(output.stdout)
            } else {
                Err(format!(
                    "exit {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        })
    }

    fn sha256(&self, name: &str) -> Result<String, String> {
        let out = self.run(&format!("sha256sum '{name}'"))?;
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .next()
            .map(str::to_string)
            .ok_or_else(|| format!("no sha256 for remote {name}"))
    }

    /// Download `ranges` of remote file `name` into `local` at the same
    /// offsets, one ssh stream per range, `streams` at a time.
    fn download(&self, name: &str, local: &Path, ranges: &[(u64, u64)], streams: usize) -> Result<(), String> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(local)
            .map_err(|err| format!("open {}: {err}", local.display()))?;
        for group in ranges.chunks(streams.max(1)) {
            let handles: Vec<JoinHandle<Result<(), String>>> = group
                .iter()
                .map(|&(offset, len)| {
                    let remote = self.clone();
                    let file = file.try_clone().expect("clone mirror handle");
                    let name = name.to_string();
                    std::thread::spawn(move || {
                        retry(&format!("download {name} @{offset}+{len}"), || {
                            let mut child = remote
                                .command(&format!(
                                    "dd if='{name}' bs=1M iflag=skip_bytes,count_bytes skip={offset} count={len} status=none"
                                ))
                                .stdin(Stdio::null())
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .spawn()
                                .map_err(|err| format!("spawn ssh: {err}"))?;
                            let mut stdout = child.stdout.take().unwrap();
                            let mut buffer = vec![0_u8; 1 << 20];
                            let mut at = offset;
                            loop {
                                let read = stdout
                                    .read(&mut buffer)
                                    .map_err(|err| format!("read stream: {err}"))?;
                                if read == 0 {
                                    break;
                                }
                                file.write_all_at(&buffer[..read], at)
                                    .map_err(|err| format!("write mirror: {err}"))?;
                                at += read as u64;
                            }
                            let status = child.wait().map_err(|err| err.to_string())?;
                            if !status.success() || at != offset + len {
                                return Err(format!(
                                    "stream ended at {} of {}..{} ({status})",
                                    at,
                                    offset,
                                    offset + len
                                ));
                            }
                            Ok(())
                        })
                    })
                })
                .collect();
            for handle in handles {
                handle.join().map_err(|_| "download thread panicked".to_string())??;
            }
        }
        file.sync_all().map_err(|err| format!("sync {}: {err}", local.display()))
    }

    /// Upload local `path` as remote `name` in `streams` parallel ranges.
    fn upload(&self, path: &Path, name: &str, streams: usize) -> Result<(), String> {
        let len = fs::metadata(path)
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .len();
        self.run(&format!("rm -f '{name}'; truncate -s {len} '{name}'"))?;
        let chunk = len.div_ceil(streams.max(1) as u64).max(1);
        let handles: Vec<JoinHandle<Result<(), String>>> = (0..streams.max(1) as u64)
            .map(|index| (index * chunk, (len.saturating_sub(index * chunk)).min(chunk)))
            .filter(|&(_, part)| part > 0)
            .map(|(offset, part)| {
                let remote = self.clone();
                let path = path.to_path_buf();
                let name = name.to_string();
                std::thread::spawn(move || {
                    retry(&format!("upload {name} @{offset}+{part}"), || {
                        let file = File::open(&path).map_err(|err| err.to_string())?;
                        let mut child = remote
                            .command(&format!(
                                "dd of='{name}' bs=1M oflag=seek_bytes seek={offset} conv=notrunc status=none"
                            ))
                            .stdin(Stdio::piped())
                            .stdout(Stdio::null())
                            .stderr(Stdio::piped())
                            .spawn()
                            .map_err(|err| format!("spawn ssh: {err}"))?;
                        let mut stdin = child.stdin.take().unwrap();
                        let mut buffer = vec![0_u8; 1 << 20];
                        let mut at = offset;
                        while at < offset + part {
                            let want = ((offset + part - at) as usize).min(buffer.len());
                            file.read_exact_at(&mut buffer[..want], at)
                                .map_err(|err| err.to_string())?;
                            stdin
                                .write_all(&buffer[..want])
                                .map_err(|err| format!("write stream: {err}"))?;
                            at += want as u64;
                        }
                        drop(stdin);
                        let status = child.wait().map_err(|err| err.to_string())?;
                        if !status.success() {
                            return Err(format!("remote dd {status}"));
                        }
                        Ok(())
                    })
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "upload thread panicked".to_string())??;
        }
        let (local_sha, remote_sha) = (local_sha256(path)?, self.sha256(name)?);
        if local_sha != remote_sha {
            return Err(format!("uploaded {name} sha256 {remote_sha} != local {local_sha}"));
        }
        Ok(())
    }
}

fn abbreviate(script: &str) -> String {
    let line: String = script.chars().take(120).collect();
    line.replace('\n', " ")
}

fn retry<T>(what: &str, mut attempt: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut last = String::new();
    for index in 0..ATTEMPTS {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(err) => {
                last = err;
                let wait = Duration::from_secs(5 << index.min(5));
                println!("remote: {what} failed ({last}); retry {}/{} in {:?}", index + 1, ATTEMPTS, wait);
                std::thread::sleep(wait);
            }
        }
    }
    Err(format!("{what}: gave up after {ATTEMPTS} attempts: {last}"))
}

fn local_sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(path)
        .output()
        .map_err(|err| format!("spawn shasum: {err}"))?;
    if !output.status.success() {
        return Err(format!("shasum {}: {}", path.display(), output.status));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("no sha256 for {}", path.display()))
}

fn shard_name(shard: u32) -> String {
    format!("tiles-{shard:03}.mkshard")
}

/// The remote journal as listed at startup.
#[derive(Default)]
struct RemoteJournal {
    manifests: HashMap<u32, Vec<u8>>,
    pending: HashMap<u32, Vec<u8>>,
    partial_len: HashMap<u32, u64>,
}

pub struct RemoteHost {
    remote: Remote,
    local: PathBuf,
    streams: usize,
    shard_lens: Vec<u64>,
    journal: RemoteJournal,
    prefetch: Option<(u32, JoinHandle<Result<(), String>>)>,
    upload: Option<(u32, JoinHandle<Result<(), String>>)>,
}

impl RemoteHost {
    /// `ssh` is `user@host`, `remote_dir` the archive directory there, and
    /// `local` the mirror directory (created; reused on resume).
    pub fn connect(ssh: &str, remote_dir: &str, local: &Path, streams: usize) -> Result<Self, String> {
        let remote = Remote {
            ssh: ssh.to_string(),
            dir: remote_dir.to_string(),
        };
        fs::create_dir_all(local).map_err(|err| format!("create {}: {err}", local.display()))?;
        // Sizes of the root and every shard as they are now.
        let listing = remote.run("stat -c '%n %s' root.mkidx tiles-*.mkshard")?;
        let mut shard_lens = Vec::new();
        let mut root_len = 0;
        for line in String::from_utf8_lossy(&listing).lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(len)) = (parts.next(), parts.next()) else {
                continue;
            };
            let len: u64 = len.parse().map_err(|_| format!("bad stat line '{line}'"))?;
            if name == "root.mkidx" {
                root_len = len;
            } else if let Some(index) = name
                .strip_prefix("tiles-")
                .and_then(|rest| rest.strip_suffix(".mkshard"))
                .and_then(|index| index.parse::<usize>().ok())
            {
                if shard_lens.len() <= index {
                    shard_lens.resize(index + 1, 0);
                }
                shard_lens[index] = len;
            }
        }
        if shard_lens.is_empty() || shard_lens.contains(&0) {
            return Err(format!("{ssh}:{remote_dir} does not list contiguous shards"));
        }
        // The mirror's root is the original: fetched once, kept until the
        // conversion writes the new one over it.
        let root = local.join("root.mkidx");
        if !root.exists() {
            let bytes = remote.run("cat root.mkidx")?;
            if bytes.len() as u64 != root_len {
                return Err("root.mkidx download was truncated".to_string());
            }
            write_atomic(&root, &bytes)?;
        }
        for (shard, &len) in shard_lens.iter().enumerate() {
            let path = shard_path(local, shard as u32);
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|err| format!("open {}: {err}", path.display()))?;
            if file.metadata().map_err(|err| err.to_string())?.len() != len {
                file.set_len(len).map_err(|err| format!("size {}: {err}", path.display()))?;
            }
        }
        let journal = Self::read_journal(&remote)?;
        println!(
            "remote: {ssh}:{remote_dir}, {} shards, {} journaled, {} pending",
            shard_lens.len(),
            journal.manifests.len(),
            journal.pending.len()
        );
        Ok(Self {
            remote,
            local: local.to_path_buf(),
            streams,
            shard_lens,
            journal,
            prefetch: None,
            upload: None,
        })
    }

    fn read_journal(remote: &Remote) -> Result<RemoteJournal, String> {
        let out = remote.run(
            "for f in tiles-*.mkrepack tiles-*.mkrepack-pending; do [ -e \"$f\" ] && echo \"M $f $(base64 -w0 \"$f\")\"; done; \
             for f in tiles-*.partial; do [ -e \"$f\" ] && echo \"P $f $(stat -c %s \"$f\")\"; done; true",
        )?;
        let mut journal = RemoteJournal::default();
        for line in String::from_utf8_lossy(&out).lines() {
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() != 3 {
                continue;
            }
            let shard = parts[1]
                .strip_prefix("tiles-")
                .and_then(|rest| rest.get(..3))
                .and_then(|index| index.parse::<u32>().ok())
                .ok_or_else(|| format!("bad journal entry '{line}'"))?;
            match parts[0] {
                "M" => {
                    let bytes = base64_decode(parts[2])?;
                    if parts[1].ends_with("-pending") {
                        journal.pending.insert(shard, bytes);
                    } else {
                        journal.manifests.insert(shard, bytes);
                    }
                }
                "P" => {
                    journal
                        .partial_len
                        .insert(shard, parts[2].trim().parse().map_err(|_| format!("bad '{line}'"))?);
                }
                _ => {}
            }
        }
        Ok(journal)
    }

    fn join_upload(&mut self) -> Result<(), String> {
        if let Some((shard, handle)) = self.upload.take() {
            handle
                .join()
                .map_err(|_| format!("upload of shard {shard:03} panicked"))??;
        }
        Ok(())
    }

    /// 64 MiB pieces keep a retry cheap; `streams` of them run at once.
    fn whole_shard_ranges(len: u64) -> Vec<(u64, u64)> {
        let piece = 64 << 20;
        (0..len.div_ceil(piece))
            .map(|index| (index * piece, (len - index * piece).min(piece)))
            .collect()
    }

    fn spawn_download(&self, shard: u32) -> JoinHandle<Result<(), String>> {
        let remote = self.remote.clone();
        let local = shard_path(&self.local, shard);
        let len = self.shard_lens[shard as usize];
        let streams = self.streams;
        std::thread::spawn(move || {
            let start = Instant::now();
            let ranges = Self::whole_shard_ranges(len);
            remote.download(&shard_name(shard), &local, &ranges, streams)?;
            let (local_sha, remote_sha) = (local_sha256(&local)?, remote.sha256(&shard_name(shard))?);
            if local_sha != remote_sha {
                return Err(format!("shard {shard:03} download sha256 mismatch"));
            }
            println!(
                "remote: shard {shard:03} downloaded, {len} bytes in {:.0}s ({:.1} MB/s)",
                start.elapsed().as_secs_f64(),
                len as f64 / 1e6 / start.elapsed().as_secs_f64().max(0.001)
            );
            Ok(())
        })
    }
}

impl InPlaceHost for RemoteHost {
    fn fetch_ranges(&mut self, ranges: &[(u32, u64, u64)]) -> Result<(), String> {
        let start = Instant::now();
        let mut by_shard: HashMap<u32, Vec<(u64, u64)>> = HashMap::new();
        for &(shard, offset, len) in ranges {
            by_shard.entry(shard).or_default().push((offset, len));
        }
        // Many small ranges: batch them into one remote dd loop per call so
        // the thousands of leaf/stash reads do not each pay an ssh handshake.
        let mut jobs: Vec<(u32, Vec<(u64, u64)>)> = Vec::new();
        for (shard, mut list) in by_shard {
            list.sort_unstable();
            for batch in list.chunks(RANGE_BATCH) {
                jobs.push((shard, batch.to_vec()));
            }
        }
        let total: u64 = ranges.iter().map(|range| range.2).sum();
        for group in jobs.chunks(self.streams.max(1)) {
            let handles: Vec<JoinHandle<Result<(), String>>> = group
                .iter()
                .cloned()
                .map(|(shard, batch)| {
                    let remote = self.remote.clone();
                    let local = shard_path(&self.local, shard);
                    std::thread::spawn(move || {
                        let script: String = batch
                            .iter()
                            .map(|(offset, len)| {
                                format!(
                                    "dd if='{}' bs=1M iflag=skip_bytes,count_bytes skip={offset} count={len} status=none; ",
                                    shard_name(shard)
                                )
                            })
                            .collect();
                        let bytes = remote.run(&script)?;
                        let expected: u64 = batch.iter().map(|range| range.1).sum();
                        if bytes.len() as u64 != expected {
                            return Err(format!(
                                "shard {shard:03} range batch returned {} of {expected} bytes",
                                bytes.len()
                            ));
                        }
                        let file = OpenOptions::new()
                            .write(true)
                            .open(&local)
                            .map_err(|err| format!("open {}: {err}", local.display()))?;
                        let mut at = 0_usize;
                        for (offset, len) in batch {
                            file.write_all_at(&bytes[at..at + len as usize], offset)
                                .map_err(|err| format!("write {}: {err}", local.display()))?;
                            at += len as usize;
                        }
                        file.sync_all().map_err(|err| err.to_string())
                    })
                })
                .collect();
            for handle in handles {
                handle.join().map_err(|_| "range thread panicked".to_string())??;
            }
        }
        println!(
            "remote: fetched {} ranges ({total} bytes) in {:.0}s",
            ranges.len(),
            start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    fn fetch_shard(&mut self, shard: u32) -> Result<(), String> {
        let handle = match self.prefetch.take() {
            Some((prefetched, handle)) if prefetched == shard => handle,
            Some((other, handle)) => {
                // A prefetch of another shard (resume skipped it): finish it
                // quietly and fetch the one asked for.
                let _ = handle.join();
                let _ = other;
                self.spawn_download(shard)
            }
            None => self.spawn_download(shard),
        };
        handle
            .join()
            .map_err(|_| format!("download of shard {shard:03} panicked"))??;
        // Next original shard downloads while this one converts. Skip ones
        // the remote journal already lists as converted.
        let next = (shard + 1..self.shard_lens.len() as u32).find(|next| {
            !self.journal.manifests.contains_key(next) && !self.journal.pending.contains_key(next)
        });
        if let Some(next) = next {
            self.prefetch = Some((next, self.spawn_download(next)));
        }
        Ok(())
    }

    fn recover(&mut self, shard: u32) -> Result<(), String> {
        let Some(bytes) = self.journal.pending.remove(&shard) else {
            if let Some(bytes) = self.journal.manifests.get(&shard) {
                if !manifest_path(&self.local, shard).exists() {
                    write_atomic(&manifest_path(&self.local, shard), bytes)?;
                }
            }
            return Ok(());
        };
        let manifest = decode_manifest(&bytes)?;
        let partial = format!("tiles-{shard:03}.partial");
        let rename = match self.journal.partial_len.get(&shard) {
            Some(&len) if len == manifest.file_len => {
                format!("sync '{partial}'; mv '{partial}' '{}'; ", shard_name(shard))
            }
            Some(&len) => {
                return Err(format!(
                    "remote {partial} is {len} bytes, its journal says {}",
                    manifest.file_len
                ))
            }
            None => String::new(),
        };
        self.remote.run(&format!(
            "{rename}cp tiles-{shard:03}.mkrepack-pending tiles-{shard:03}.mkrepack.tmp; sync tiles-{shard:03}.mkrepack.tmp; \
             mv tiles-{shard:03}.mkrepack.tmp tiles-{shard:03}.mkrepack; rm -f tiles-{shard:03}.mkrepack-pending; sync"
        ))?;
        write_atomic(&manifest_path(&self.local, shard), &bytes)?;
        self.journal.manifests.insert(shard, bytes);
        println!("remote: shard {shard:03} completed from its journal");
        Ok(())
    }

    fn completed(&mut self, shard: u32) -> Result<Option<Vec<u8>>, String> {
        let path = manifest_path(&self.local, shard);
        if path.exists() {
            return fs::read(&path)
                .map(Some)
                .map_err(|err| format!("read {}: {err}", path.display()));
        }
        Ok(self.journal.manifests.get(&shard).cloned())
    }

    fn finish_shard(&mut self, shard: u32, manifest: &[u8]) -> Result<(), String> {
        // One upload in flight: the previous shard must be published before
        // this one starts, so the journal advances in order.
        self.join_upload()?;
        let remote = self.remote.clone();
        let local = self.local.clone();
        let streams = self.streams;
        let manifest = manifest.to_vec();
        let expected_len = decode_manifest(&manifest)?.file_len;
        let partial_local = shard_path(&local, shard).with_extension("partial");
        let partial_len = fs::metadata(&partial_local)
            .map_err(|err| format!("stat {}: {err}", partial_local.display()))?
            .len();
        if partial_len != expected_len {
            return Err(format!("local partial {shard:03} is {partial_len} bytes, manifest {expected_len}"));
        }
        self.upload = Some((
            shard,
            std::thread::spawn(move || {
                let start = Instant::now();
                let partial = format!("tiles-{shard:03}.partial");
                remote.upload(&partial_local, &partial, streams)?;
                let encoded = base64_encode(&manifest);
                remote.run(&format!(
                    "echo '{encoded}' | base64 -d > tiles-{shard:03}.mkrepack-pending.tmp; sync tiles-{shard:03}.mkrepack-pending.tmp; \
                     mv tiles-{shard:03}.mkrepack-pending.tmp tiles-{shard:03}.mkrepack-pending; \
                     sync '{partial}'; mv '{partial}' '{}'; \
                     cp tiles-{shard:03}.mkrepack-pending tiles-{shard:03}.mkrepack.tmp; sync tiles-{shard:03}.mkrepack.tmp; \
                     mv tiles-{shard:03}.mkrepack.tmp tiles-{shard:03}.mkrepack; rm -f tiles-{shard:03}.mkrepack-pending; sync",
                    shard_name(shard)
                ))?;
                write_atomic(&manifest_path(&local, shard), &manifest)?;
                // The mirror copy of the original and the local partial
                // are no longer needed (borrowed blobs live in the stash).
                remove_if_present(&partial_local)?;
                let mirror = shard_path(&local, shard);
                OpenOptions::new()
                    .write(true)
                    .open(&mirror)
                    .and_then(|file| file.set_len(0))
                    .map_err(|err| format!("release {}: {err}", mirror.display()))?;
                println!(
                    "remote: shard {shard:03} published, {partial_len} bytes up in {:.0}s",
                    start.elapsed().as_secs_f64()
                );
                Ok(())
            }),
        ));
        Ok(())
    }

    fn publish_root(&mut self, shard_count: u32) -> Result<(), String> {
        self.join_upload()?;
        if let Some((_, handle)) = self.prefetch.take() {
            let _ = handle.join();
        }
        let root = self.local.join("root.mkidx");
        self.remote.upload(&root, "root.partial", 1)?;
        self.remote
            .run("sync root.partial; mv root.partial root.mkidx; rm -f tiles-*.mkrepack; sync")?;
        remove_if_present(&stash_index_path(&self.local))?;
        remove_if_present(&stash_data_path(&self.local))?;
        for shard in 0..shard_count {
            remove_if_present(&manifest_path(&self.local, shard))?;
            remove_if_present(&shard_path(&self.local, shard))?;
        }
        println!("remote: root.mkidx published; conversion complete");
        Ok(())
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0_u32;
    let mut bits = 0;
    for byte in text.trim().bytes() {
        if byte == b'=' {
            break;
        }
        let value = B64
            .iter()
            .position(|&c| c == byte)
            .ok_or_else(|| "bad base64 in remote journal".to_string())? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
