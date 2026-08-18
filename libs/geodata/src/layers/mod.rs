//! One module per overlay layer. Each layer declares its bulk sources and
//! knows how to build its own .mbtiles from the cached files.

pub mod buildings;
pub mod cbs_grid;
pub mod chargers;
pub mod flood;
pub mod nature;
pub mod noise;
pub mod terrain;
pub mod transit;
pub mod wijkbuurt;

use crate::fetch::SourceSpec;
use std::path::{Path, PathBuf};

pub struct BuildCtx {
    pub cache_dir: PathBuf,
    pub out_dir: PathBuf,
}

impl BuildCtx {
    pub fn cached(&self, spec: &SourceSpec) -> PathBuf {
        self.cache_dir.join(spec.filename)
    }
    pub fn out_file(&self, layer_id: &str) -> PathBuf {
        self.out_dir.join(format!("nl-{layer_id}.mbtiles"))
    }
}

pub struct BuildReport {
    pub out_path: PathBuf,
    pub features: u64,
    pub tiles: u64,
    pub bytes: u64,
}

pub trait Layer {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// Bulk files this layer needs. Empty for not-yet-wired layers.
    fn sources(&self) -> Vec<SourceSpec>;
    fn implemented(&self) -> bool {
        true
    }
    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String>;
}

/// Planned layers that are designed (see README) but not built yet. They are
/// listed so `geodata list` shows the roadmap, and refuse to build.
pub struct PlannedLayer {
    pub id: &'static str,
    pub description: &'static str,
}

impl Layer for PlannedLayer {
    fn id(&self) -> &'static str {
        self.id
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn sources(&self) -> Vec<SourceSpec> {
        Vec::new()
    }
    fn implemented(&self) -> bool {
        false
    }
    fn build(&self, _ctx: &BuildCtx) -> Result<BuildReport, String> {
        Err(format!(
            "layer '{}' is designed but not implemented yet (see libs/geodata/README.md)",
            self.id
        ))
    }
}

/// Registry of all layers, implemented and planned.
pub fn registry() -> Vec<Box<dyn Layer>> {
    vec![
        Box::new(nature::NatureLayer),
        Box::new(chargers::ChargersLayer),
        Box::new(cbs_grid::CbsGridLayer),
        Box::new(wijkbuurt::WijkBuurtLayer),
        Box::new(transit::TransitLayer),
        Box::new(buildings::BuildingsLayer),
        Box::new(terrain::TerrainLayer),
        Box::new(noise::NoiseLayer),
        Box::new(flood::FloodLayer),
    ]
}

pub fn find_layer(id: &str) -> Option<Box<dyn Layer>> {
    registry().into_iter().find(|l| l.id() == id)
}

/// Helper: run a closure over a gzip file's decompressed bytes.
pub fn read_gz(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    makepad_fast_inflate::gzip_decompress_vec(&bytes)
        .map_err(|e| format!("gunzip {}: {e}", path.display()))
}

/// Extract every .gpkg inside a zip into the cache dir (skipping members that
/// are already extracted and newer than the zip). Returns extracted paths.
pub fn unzip_gpkgs(zip_path: &Path, cache_dir: &Path) -> Result<Vec<PathBuf>, String> {
    use std::process::Command;
    let listing = Command::new("unzip")
        .arg("-Z1")
        .arg(zip_path)
        .output()
        .map_err(|e| format!("unzip -Z1: {e}"))?;
    if !listing.status.success() {
        return Err(format!("unzip -Z1 failed on {}", zip_path.display()));
    }
    let names = String::from_utf8_lossy(&listing.stdout);
    let gpkg_names: Vec<String> = names
        .lines()
        .filter(|l| l.to_lowercase().ends_with(".gpkg"))
        .map(|l| l.to_string())
        .collect();
    if gpkg_names.is_empty() {
        return Err(format!("no .gpkg inside {}", zip_path.display()));
    }
    let mut out_paths = Vec::new();
    for name in &gpkg_names {
        let out_path = cache_dir.join(
            Path::new(name)
                .file_name()
                .ok_or("bad zip entry name")?,
        );
        let fresh = match (out_path.metadata(), zip_path.metadata()) {
            (Ok(o), Ok(z)) => match (o.modified(), z.modified()) {
                (Ok(om), Ok(zm)) => om >= zm,
                _ => false,
            },
            _ => false,
        };
        if !fresh {
            let status = Command::new("unzip")
                .arg("-o")
                .arg("-j")
                .arg(zip_path)
                .arg(name)
                .arg("-d")
                .arg(cache_dir)
                .status()
                .map_err(|e| format!("unzip: {e}"))?;
            if !status.success() {
                return Err(format!("unzip failed on {}", zip_path.display()));
            }
        }
        out_paths.push(out_path);
    }
    Ok(out_paths)
}
