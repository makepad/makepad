//! Rain radar sync — the app-embeddable "updated downloader".
//!
//! Unlike the static layers, radar is a rolling time series: KNMI publishes a
//! new 5-minute reflectivity composite every 5 minutes and a +2h nowcast
//! (`radar_forecast`) every 5 minutes. `RadarSync` maintains a small local
//! cache of the newest frames and is designed to be owned by the map app:
//! call `sync()` as often as you like — it never touches the network more
//! than once per `min_poll_secs`, downloads only frames it doesn't have,
//! and prunes old ones. `state()` returns the cached frames without any
//! network contact at all.
//!
//! Files are KNMI HDF5 (polar stereographic). Decoding to a renderable /
//! queryable raster is the next step and lives outside this module; the sync
//! layer's contract is just: freshest N frames on disk + an index.
//!
//! Auth: KNMI's Open Data API wants an API key. Priority: explicit config >
//! `KNMI_API_KEY` env var > the public anonymous key KNMI documents on the
//! developer portal (shared, 50 req/min across all anonymous users — fine
//! for one poll per 5 minutes, but register a free personal key for real
//! deployments).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Public anonymous key from developer.dataplatform.knmi.nl (shared quota).
pub const KNMI_ANONYMOUS_KEY: &str = "eyJvcmciOiI1ZTU1NGUxOTI3NGE5NjAwMDEyYTNlYjEiLCJpZCI6IjUzYTg1ZDBhMmQ5YzRkYzJiYWNlNzQ4NTQ2Zjk4ODExIiwiaCI6Im11cm11cjEyOCJ9";

const API_BASE: &str = "https://api.dataplatform.knmi.nl/open-data/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadarDataset {
    /// 5-minute real-time reflectivity composite (one frame per file).
    ReflectivityComposite,
    /// +2h precipitation nowcast, refreshed every 5 minutes (whole animation
    /// in one file) — the most useful single file for the map app.
    Forecast,
    /// Raw polar volume of the Herwijnen radar (RAD_NL62, ~21 MB / 5 min):
    /// 223.5 m range bins on the low scans — the hi-res "now" source.
    VolumeHerwijnen,
    /// Raw polar volume of the Den Helder radar (RAD_NL61, ~31 MB / 5 min).
    /// NB the current dataset version is 2.0 (1.0 is gone).
    VolumeDenHelder,
}

impl RadarDataset {
    fn api_path(&self) -> (&'static str, &'static str) {
        match self {
            RadarDataset::ReflectivityComposite => ("radar_reflectivity_composites", "2.0"),
            RadarDataset::Forecast => ("radar_forecast", "1.0"),
            RadarDataset::VolumeHerwijnen => ("radar_volume_full_herwijnen", "1.0"),
            RadarDataset::VolumeDenHelder => ("radar_volume_denhelder", "2.0"),
        }
    }
    fn cache_subdir(&self) -> &'static str {
        match self {
            RadarDataset::ReflectivityComposite => "reflectivity",
            RadarDataset::Forecast => "forecast",
            RadarDataset::VolumeHerwijnen => "volume_herwijnen",
            RadarDataset::VolumeDenHelder => "volume_denhelder",
        }
    }
}

pub struct RadarConfig {
    pub cache_dir: PathBuf,
    pub dataset: RadarDataset,
    pub api_key: Option<String>,
    /// Never contact the network more often than this. The data itself
    /// refreshes every 300 s, so the default 240 s guarantees at most one
    /// poll per new upstream file.
    pub min_poll_secs: u64,
    /// Keep at most this many newest frames on disk.
    pub max_frames: usize,
}

impl RadarConfig {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        RadarConfig {
            cache_dir: cache_dir.into(),
            dataset: RadarDataset::Forecast,
            api_key: None,
            min_poll_secs: 240,
            // One forecast file holds the whole +2h animation; keeping the
            // previous one covers the swap window. Reflectivity wants more
            // (one frame per file) — see `for_dataset`.
            max_frames: 2,
        }
    }

    pub fn for_dataset(cache_dir: impl Into<PathBuf>, dataset: RadarDataset) -> Self {
        let mut config = Self::new(cache_dir);
        config.dataset = dataset;
        if dataset == RadarDataset::ReflectivityComposite {
            config.max_frames = 13; // ~1 hour of 5-min frames
        }
        config
    }

    /// Newest frame of both radar volumes; the hi-res compositor wants the
    /// same-timestamp pair, so keep two frames per radar for the swap window.
    pub fn volume_pair(cache_dir: impl Into<PathBuf>) -> (Self, Self) {
        let cache_dir = cache_dir.into();
        (
            Self::for_dataset(cache_dir.clone(), RadarDataset::VolumeHerwijnen),
            Self::for_dataset(cache_dir, RadarDataset::VolumeDenHelder),
        )
    }
    fn resolved_key(&self) -> String {
        self.api_key
            .clone()
            .or_else(|| std::env::var("KNMI_API_KEY").ok().filter(|k| !k.is_empty()))
            .unwrap_or_else(|| KNMI_ANONYMOUS_KEY.to_string())
    }
    fn dir(&self) -> PathBuf {
        self.cache_dir.join(self.dataset.cache_subdir())
    }
}

#[derive(Debug, Clone)]
pub struct RadarFrame {
    pub filename: String,
    pub path: PathBuf,
    pub created_unix: u64,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct RadarState {
    /// Frames on disk, oldest first.
    pub frames: Vec<RadarFrame>,
    pub last_poll_unix: u64,
    /// Whether this call actually contacted the server.
    pub polled: bool,
    pub downloaded: usize,
}

pub struct RadarSync {
    config: RadarConfig,
}

impl RadarSync {
    pub fn new(config: RadarConfig) -> Self {
        RadarSync { config }
    }

    /// Cached frames without any network contact.
    pub fn state(&self) -> RadarState {
        let (last_poll, frames) = self.read_index();
        RadarState {
            frames,
            last_poll_unix: last_poll,
            polled: false,
            downloaded: 0,
        }
    }

    /// Poll for new frames if the poll gate allows; otherwise return cache.
    pub fn sync(&self) -> Result<RadarState, String> {
        let dir = self.config.dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let (last_poll, cached_frames) = self.read_index();
        let now = now_unix();
        if now.saturating_sub(last_poll) < self.config.min_poll_secs {
            return Ok(RadarState {
                frames: cached_frames,
                last_poll_unix: last_poll,
                polled: false,
                downloaded: 0,
            });
        }

        // Record the attempt BEFORE any network contact: even a failing sync
        // must arm the poll gate, or a broken API would get hammered.
        self.write_index(now, &cached_frames)?;

        let (dataset, version) = self.config.dataset.api_path();
        let key = self.config.resolved_key();
        let max_keys = self.config.max_frames.max(1);
        let list_url = format!(
            "{API_BASE}/datasets/{dataset}/versions/{version}/files?maxKeys={max_keys}&orderBy=created&sorting=desc"
        );
        let listing: serde_json::Value = api_get_json(&list_url, &key)?;
        let files = listing
            .get("files")
            .and_then(|f| f.as_array())
            .ok_or_else(|| format!("unexpected KNMI listing: {listing}"))?;

        let mut downloaded = 0usize;
        let mut frames: Vec<RadarFrame> = Vec::new();
        for file in files {
            let Some(filename) = file.get("filename").and_then(|f| f.as_str()) else {
                continue;
            };
            let created = file
                .get("created")
                .and_then(|c| c.as_str())
                .and_then(parse_iso8601_unix)
                .unwrap_or(0);
            let path = dir.join(filename);
            if !path.exists() {
                let url_url = format!(
                    "{API_BASE}/datasets/{dataset}/versions/{version}/files/{filename}/url"
                );
                let url_response: serde_json::Value = api_get_json(&url_url, &key)?;
                let Some(signed) = url_response
                    .get("temporaryDownloadUrl")
                    .and_then(|u| u.as_str())
                else {
                    return Err(format!("no download url for {filename}: {url_response}"));
                };
                download(signed, &path)?;
                downloaded += 1;
            }
            let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
            frames.push(RadarFrame {
                filename: filename.to_string(),
                path,
                created_unix: created,
                bytes,
            });
        }
        frames.sort_by_key(|f| f.created_unix);

        // Prune files no longer in the newest set.
        let keep: std::collections::HashSet<&str> =
            frames.iter().map(|f| f.filename.as_str()).collect();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".json") || keep.contains(name.as_ref()) {
                    continue;
                }
                let _ = std::fs::remove_file(entry.path());
            }
        }

        self.write_index(now, &frames)?;
        Ok(RadarState {
            frames,
            last_poll_unix: now,
            polled: true,
            downloaded,
        })
    }

    fn index_path(&self) -> PathBuf {
        self.config.dir().join("index.json")
    }

    fn read_index(&self) -> (u64, Vec<RadarFrame>) {
        let Ok(text) = std::fs::read_to_string(self.index_path()) else {
            return (0, Vec::new());
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return (0, Vec::new());
        };
        let last_poll = value
            .get("last_poll_unix")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let dir = self.config.dir();
        let frames = value
            .get("frames")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        let filename = f.get("filename")?.as_str()?.to_string();
                        let path = dir.join(&filename);
                        if !path.exists() {
                            return None;
                        }
                        Some(RadarFrame {
                            bytes: path.metadata().ok()?.len(),
                            path,
                            created_unix: f.get("created_unix")?.as_u64()?,
                            filename,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        (last_poll, frames)
    }

    fn write_index(&self, last_poll: u64, frames: &[RadarFrame]) -> Result<(), String> {
        let value = serde_json::json!({
            "last_poll_unix": last_poll,
            "dataset": self.config.dataset.cache_subdir(),
            "frames": frames.iter().map(|f| serde_json::json!({
                "filename": f.filename,
                "created_unix": f.created_unix,
                "bytes": f.bytes,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(
            self.index_path(),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .map_err(|e| format!("write index: {e}"))
    }
}

fn api_get_json(url: &str, key: &str) -> Result<serde_json::Value, String> {
    // Pace every request; on a 429 (the shared anonymous key saturates), back
    // off once and retry before giving up.
    for attempt in 0..2 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let output = Command::new("curl")
            .arg("-fsS")
            .arg("--connect-timeout")
            .arg("15")
            .arg("--max-time")
            .arg("60")
            .arg("-A")
            .arg(crate::fetch::USER_AGENT)
            .arg("-H")
            .arg(format!("Authorization: {key}"))
            .arg(url)
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout)
                .map_err(|e| format!("KNMI API json: {e}"));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if attempt == 0 && stderr.contains("429") {
            std::thread::sleep(std::time::Duration::from_secs(8));
            continue;
        }
        return Err(format!("KNMI API {url}: {} {stderr}", output.status));
    }
    unreachable!()
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let part = dest.with_extension("part");
    let status = Command::new("curl")
        .arg("-fsSL")
        .arg("--connect-timeout")
        .arg("15")
        .arg("-A")
        .arg(crate::fetch::USER_AGENT)
        .arg("-o")
        .arg(&part)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("download failed: {status}"));
    }
    std::fs::rename(&part, dest).map_err(|e| format!("rename: {e}"))
}

fn now_unix() -> u64 {
    crate::clock::now_unix()
}

/// Parse "2026-07-28T12:00:00+00:00" (KNMI `created`) to unix seconds.
/// Offsets other than +00:00/Z are honored.
fn parse_iso8601_unix(s: &str) -> Option<u64> {
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        s.get(range)?.parse().ok()
    };
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    // Days-from-civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    // Timezone offset
    let rest = &s[19..];
    if let Some(sign_pos) = rest.find(['+', '-']) {
        let sign = if rest.as_bytes()[sign_pos] == b'+' { 1 } else { -1 };
        let tz = &rest[sign_pos + 1..];
        if tz.len() >= 5 {
            let hours: i64 = tz[0..2].parse().ok()?;
            let minutes: i64 = tz[3..5].parse().ok()?;
            seconds -= sign * (hours * 3_600 + minutes * 60);
        }
    }
    u64::try_from(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_iso8601_unix;

    #[test]
    fn iso8601_epoch_math() {
        assert_eq!(parse_iso8601_unix("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_iso8601_unix("2026-07-28T12:00:00+00:00"), Some(1_785_240_000));
        // +02:00 is two hours earlier in UTC
        assert_eq!(
            parse_iso8601_unix("2026-07-28T12:00:00+02:00"),
            Some(1_785_240_000 - 7_200)
        );
    }
}
