//! Makepad Asset Importer — the headless import/publish tool beside the
//! Asset Server. The generation coordinator that used to be its default
//! mode is gone (aicore: the store stores, the client creates — apps drive
//! the fleet themselves via `makepad-asset-creator`); what remains are the
//! import modes. With `--import-ai-library <dir>` it runs one idempotent
//! import pass over the GENERATED rows of an existing ai-content library
//! (see the lib's `import` module) and exits. With `--watch-ai-library
//! <dir>` it continuously publishes stable new/changed library artifacts
//! until clean shutdown (see the lib's `watch` module). With `--import-pack
//! <dir>` it compiles a licensed local pack into canonical
//! `SourceCollection` / `ImportManifest` bytes plus a local upload plan and
//! exits without contacting the server (see `pack_import.rs`).
//!
//! Credentials never leave this host: the bearer token is read from a
//! local file (typically the server root's `admin-token`, or a scoped
//! publish token).

#[cfg(target_arch = "wasm32")]
fn main() {}

// This CLI drives native files, sockets, and directory watchers; VJ depends on the library only.
#[cfg(not(target_arch = "wasm32"))]
mod native {

// The library importer/watcher now lives in the crate's lib so the Asset UI
// can run the same publication loop in-process against its embedded server.
use makepad_asset_importer::{classic_import, games_import, import, music_import, pack_import, watch};

use makepad_asset_client::{wire, AnnotationUpload, ApiEndpoints, AssetClient, ClientConfig, PublishRights};
use makepad_asset_data::{AssetKind, BlobId, DerivativePolicy, Redistribution};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const USAGE: &str = "\
makepad-asset-importer --server <ip:controlport:dataport> --token-file <path> [options]
makepad-asset-importer --import-pack <dir> --out <dir> --source-config <file>
makepad-asset-importer --import-pack <dir> --out <dir> [source/rights flags]
makepad-asset-importer --convert-classic <source-id> [--pack <dir>] --out <staged-dir>
makepad-asset-importer --import-classic <source-id> [--pack <dir>] --server <ip:c:d> \
                       --token-file <path> --server-id <32-hex> [--fresh]
makepad-asset-importer --server <ip:c:d> --token-file <p> --import-music <dir> \
                       [--namespace <ns>]

Modes:
  --import-ai-library <dir>     One idempotent import pass over an
                                ai-content library directory, then exit.
  --watch-ai-library <dir>      Continuously publish stable new/changed
                                library artifacts until SIGINT/SIGTERM.
  --import-games <dir>          Publish every <slug>/game.splash folder
                                under <dir> as a `game` asset (alias
                                <namespace>/games/<slug>; unchanged
                                sources skip, changed ones become a new
                                revision), then exit. Rights default to the
                                named CC0 grant (own-authored sandbox
                                games) unless --license/--redistribution/
                                --derivatives are given.
  --import-music <dir>          Publish every audio file under <dir> as an
                                `audio` asset, one row per track. EVERY
                                relative directory name becomes a catalog
                                tag (plus the constant `music` tag), and
                                ID3/Vorbis title/artist/album name the row
                                when the file carries them. Alias
                                <namespace>/music/<artist>/<title>;
                                unchanged tracks skip, changed bytes become
                                a new revision of the same asset. mp3/ogg/
                                wav are published, flac/m4a/… are listed as
                                unsupported. Rights default to the personal
                                library terms (all rights reserved, LAN
                                local, local-preview derivatives) unless
                                --license/--redistribution/--derivatives
                                are given.
  --import-pack <dir>           Compile a licensed local pack (Kenney, …)
                                into canonical SourceCollection and
                                ImportManifest bytes plus a local upload
                                plan, then exit. Does not contact the
                                server. Supported files: png/jpeg/wav/mp4/glb.
  --convert-classic <id>       Convert a classic pack into --out without a
                                server or GUI. IDs: doom, freedoom, quake,
                                librequake, quake2, quake3, duke3d, darkmod,
                                cnc, ra, ts, d2k.
  --import-classic <id>        Convert, compile, and publish a classic pack
                                through the licensed pack import path.

Options:
  --server <ip:ctrl:data>       Asset Server endpoints. Or env
                                ASSET_WORKER_SERVER / ASSET_WORKER_SERVERS.
                                Required except --import-pack
                                and --convert-classic.
  --token-file <path>           File holding the bearer token (mpat_…).
                                Required (or env ASSET_WORKER_TOKEN) except
                                --import-pack and --convert-classic.
  --server-id <32 hex>          Pin the server identity (required for
                                --import-classic).
  --namespace <ns>              Namespace for imports (default gen;
                                --import-games defaults to sandbox).
  --cache <dir>                 Client cache root (default
                                ~/.makepad-asset-importer). Watch mode owns
                                a separate child.
  --out <dir>                   Output directory for --import-pack or staged
                                output for --convert-classic.
  --pack <dir>                  Classic source folder (default
                                <repo>/local/packs/<source-id>).
  --fresh                       --import-classic: discard cached conversion
                                under local/asset-importer/classic-<id>/.
  --source-config <path>        Source-only JSON identity/rights file for
                                --import-pack (no file list). CLI overrides.
  --source-id <slug>            Approved source collection id.
  --source-title <text>         Source collection title.
  --pack-name <slug>            Pack name (catalog alias segment).
  --pack-version <ver>          Pack version ([a-z0-9._-]).
  --terms-digest <64 hex>       SHA-256 of the immutable terms text.
  --terms-url <url>             URL of the immutable terms document.
  --license-revision <text>     Optional license revision qualifier.
  --source-archive <64 hex>     Optional SHA-256 of the upstream archive.
  --license <id>                Exact license identifier of the published
                                content (e.g. CC0-1.0, CC-BY-4.0). REQUIRED
                                for --import-ai-library/--watch-ai-library/
                                --import-pack: rights are never invented.
  --redistribution <policy>     allowed | attribution-required | forbidden.
                                Required with --license.
  --derivatives <policy>        allowed | attribution-required | forbidden.
                                Required with --license.
  --credits <text>              Required attribution line (mandatory when a
                                policy is attribution-required; required
                                for --import-pack).
  --source <text>               Upstream origin (pack name/URL) of the
                                imported content. Required for --import-pack.
  --quiet                       No stderr logging.
  --help                        This text.
";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: i32) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    #[cfg(unix)]
    {
        extern "C" {
            fn signal(signum: i32, handler: usize) -> usize;
        }
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        unsafe {
            signal(SIGINT, on_signal as *const () as usize);
            signal(SIGTERM, on_signal as *const () as usize);
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("makepad-asset-importer: {message}");
    eprintln!("{USAGE}");
    std::process::exit(2);
}

struct Args {
    endpoints: ApiEndpoints,
    server_id: Option<[u8; 16]>,
    token: String,
    import: Option<PathBuf>,
    watch: Option<PathBuf>,
    /// Splash game folders -> `game` assets. Exclusive with every other mode.
    import_games: Option<PathBuf>,
    /// A music directory tree -> `audio` assets. Exclusive with every other
    /// mode.
    import_music: Option<PathBuf>,
    namespace: String,
    cache: PathBuf,
    /// The operator's explicit rights declaration.
    rights: Option<PublishRights>,
    /// Local licensed-pack compiler. Exclusive with every networked mode.
    import_pack: Option<PathBuf>,
    pack_out: Option<PathBuf>,
    source_config: Option<PathBuf>,
    pack_source: pack_import::PackSourceSpec,
    classic: Option<ClassicCliArgs>,
    log: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassicCliMode {
    Convert,
    Import,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClassicCliArgs {
    mode: ClassicCliMode,
    source: classic_import::ClassicSource,
    pack: Option<PathBuf>,
    out: Option<PathBuf>,
    fresh: bool,
}

fn parse_classic_args(raw: &[String]) -> Result<Option<ClassicCliArgs>, String> {
    let mut mode = None;
    let mut source = None;
    let mut pack = None;
    let mut out = None;
    let mut fresh = false;
    let mut saw_pack = false;
    let mut index = 0usize;
    while index < raw.len() {
        let flag = raw[index].as_str();
        let takes_value = matches!(
            flag,
            "--convert-classic" | "--import-classic" | "--pack" | "--out"
        );
        let value = if takes_value {
            index += 1;
            Some(
                raw.get(index)
                    .ok_or_else(|| format!("{flag} needs a value"))?
                    .as_str(),
            )
        } else {
            None
        };
        match flag {
            "--convert-classic" | "--import-classic" => {
                if mode.is_some() {
                    return Err("choose exactly one classic mode".into());
                }
                mode = Some(if flag == "--convert-classic" {
                    ClassicCliMode::Convert
                } else {
                    ClassicCliMode::Import
                });
                let id = value.expect("mode value");
                source = Some(classic_import::ClassicSource::from_id(id).ok_or_else(|| {
                    format!("unknown classic source {id}")
                })?);
            }
            "--pack" => {
                if saw_pack {
                    return Err("--pack may be given once".into());
                }
                saw_pack = true;
                pack = Some(PathBuf::from(value.expect("pack value")));
            }
            "--out" => out = Some(PathBuf::from(value.expect("out value"))),
            "--fresh" => fresh = true,
            _ => {}
        }
        index += 1;
    }
    let Some(mode) = mode else {
        if saw_pack || fresh {
            return Err("--pack / --fresh require --convert-classic or --import-classic".into());
        }
        return Ok(None);
    };
    if mode == ClassicCliMode::Convert && out.is_none() {
        return Err("--out is required with --convert-classic".into());
    }
    if mode == ClassicCliMode::Import && out.is_some() {
        return Err("--out is only valid with --convert-classic or --import-pack".into());
    }
    if mode == ClassicCliMode::Convert && fresh {
        return Err("--fresh is only valid with --import-classic".into());
    }
    Ok(Some(ClassicCliArgs {
        mode,
        source: source.expect("classic mode has source"),
        pack,
        out,
        fresh,
    }))
}

fn parse_endpoints(spec: &str) -> Option<ApiEndpoints> {
    let mut parts = spec.split(':');
    let ip: IpAddr = parts.next()?.parse().ok()?;
    let control: u16 = parts.next()?.parse().ok()?;
    let data: u16 = parts.next()?.parse().ok()?;
    Some(ApiEndpoints {
        control: SocketAddr::new(ip, control),
        data: SocketAddr::new(ip, data),
    })
}

fn from_hex16(text: &str) -> Option<[u8; 16]> {
    makepad_asset_client::util::from_hex_exact::<16>(text.trim())
}

fn parse_args() -> Args {
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    let classic = parse_classic_args(&raw).unwrap_or_else(|error| fail(&error));
    let mut args = raw.into_iter();
    let mut servers: Vec<String> = Vec::new();
    if let Ok(one) = std::env::var("ASSET_WORKER_SERVER") {
        if !one.trim().is_empty() {
            servers.push(one);
        }
    }
    if let Ok(many) = std::env::var("ASSET_WORKER_SERVERS") {
        for part in many.split(',') {
            let spec = part.trim();
            if !spec.is_empty() && !servers.iter().any(|s| s == spec) {
                servers.push(spec.to_string());
            }
        }
    }
    let mut token = std::env::var("ASSET_WORKER_TOKEN").ok();
    let mut token_file: Option<PathBuf> = None;
    let mut server_id = None;
    let mut import = None;
    let mut watch = None;
    let mut import_games = None;
    let mut import_music = None;
    let mut namespace: Option<String> = None;
    let mut cache: Option<PathBuf> = None;
    let mut license: Option<String> = None;
    let mut redistribution: Option<String> = None;
    let mut derivatives: Option<String> = None;
    let mut credits = String::new();
    let mut source = String::new();
    let mut import_pack = None;
    let mut pack_out = None;
    let mut source_config = None;
    let mut pack_source = pack_import::PackSourceSpec::default();
    let mut log = true;
    let value_of = |name: &str, args: &mut dyn Iterator<Item = String>| -> String {
        match args.next() {
            Some(value) => value,
            None => fail(&format!("{name} needs a value")),
        }
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => {
                let spec = value_of("--server", &mut args);
                if !servers.iter().any(|s| s == &spec) {
                    servers.push(spec);
                }
            }
            "--token-file" => {
                token_file = Some(PathBuf::from(value_of("--token-file", &mut args)))
            }
            "--server-id" => {
                let value = value_of("--server-id", &mut args);
                server_id =
                    Some(from_hex16(&value).unwrap_or_else(|| fail("malformed --server-id")));
            }
            "--import-ai-library" => {
                import = Some(PathBuf::from(value_of("--import-ai-library", &mut args)))
            }
            "--watch-ai-library" => {
                watch = Some(PathBuf::from(value_of("--watch-ai-library", &mut args)))
            }
            "--import-games" => {
                import_games = Some(PathBuf::from(value_of("--import-games", &mut args)))
            }
            "--import-music" => {
                import_music = Some(PathBuf::from(value_of("--import-music", &mut args)))
            }
            "--import-pack" => {
                import_pack = Some(PathBuf::from(value_of("--import-pack", &mut args)))
            }
            "--convert-classic" | "--import-classic" => {
                let _ = value_of(&arg, &mut args);
            }
            "--pack" => {
                let _ = value_of("--pack", &mut args);
            }
            "--fresh" => {}
            "--out" => pack_out = Some(PathBuf::from(value_of("--out", &mut args))),
            "--source-config" => {
                source_config = Some(PathBuf::from(value_of("--source-config", &mut args)))
            }
            "--source-id" => {
                pack_source.source_id = Some(value_of("--source-id", &mut args).trim().to_string())
            }
            "--source-title" => {
                pack_source.source_title =
                    Some(value_of("--source-title", &mut args).trim().to_string())
            }
            "--pack-name" => {
                pack_source.pack_name = Some(value_of("--pack-name", &mut args).trim().to_string())
            }
            "--pack-version" => {
                pack_source.pack_version =
                    Some(value_of("--pack-version", &mut args).trim().to_string())
            }
            "--terms-digest" => {
                pack_source.terms_digest =
                    Some(value_of("--terms-digest", &mut args).trim().to_string())
            }
            "--terms-url" => {
                pack_source.terms_url = Some(value_of("--terms-url", &mut args).trim().to_string())
            }
            "--license-revision" => {
                pack_source.license_revision =
                    Some(value_of("--license-revision", &mut args).trim().to_string())
            }
            "--source-archive" => {
                pack_source.source_archive =
                    Some(value_of("--source-archive", &mut args).trim().to_string())
            }
            "--namespace" => namespace = Some(value_of("--namespace", &mut args)),
            "--cache" => cache = Some(PathBuf::from(value_of("--cache", &mut args))),
            "--license" => license = Some(value_of("--license", &mut args).trim().to_string()),
            "--redistribution" => {
                redistribution = Some(value_of("--redistribution", &mut args).trim().to_string())
            }
            "--derivatives" => {
                derivatives = Some(value_of("--derivatives", &mut args).trim().to_string())
            }
            "--credits" => credits = value_of("--credits", &mut args).trim().to_string(),
            "--source" => source = value_of("--source", &mut args).trim().to_string(),
            "--quiet" => log = false,
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => fail(&format!("unknown flag {other}")),
        }
    }
    if let Some(path) = token_file {
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => token = Some(text.trim().to_string()),
            Ok(_) => fail("token file is empty"),
            Err(error) => fail(&format!("token file {}: {error}", path.display())),
        }
    }
    if license.is_some() {
        pack_source.license = license.clone();
    }
    if redistribution.is_some() {
        pack_source.redistribution = redistribution.clone();
    }
    if derivatives.is_some() {
        pack_source.derivatives = derivatives.clone();
    }
    if !credits.is_empty() {
        pack_source.credits = Some(credits.clone());
    }
    if !source.is_empty() {
        pack_source.source = Some(source.clone());
    }
    let classic_convert = classic
        .as_ref()
        .is_some_and(|classic| classic.mode == ClassicCliMode::Convert);
    let classic_import = classic
        .as_ref()
        .is_some_and(|classic| classic.mode == ClassicCliMode::Import);
    let classic_mode = classic.is_some();
    if import.is_some() && watch.is_some() {
        fail("--import-ai-library and --watch-ai-library are mutually exclusive");
    }
    if import_games.is_some()
        && (import.is_some()
            || watch.is_some()
            || import_pack.is_some()
            || classic_mode)
    {
        fail("--import-games is exclusive with every other mode");
    }
    if import_music.is_some()
        && (import.is_some()
            || watch.is_some()
            || import_pack.is_some()
            || classic_mode
            || import_games.is_some())
    {
        fail("--import-music is exclusive with every other mode");
    }
    let namespace = namespace.unwrap_or_else(|| {
        if import_games.is_some() {
            "sandbox"
        } else if import_music.is_some() {
            "music"
        } else {
            "gen"
        }
        .to_string()
    });
    if import_pack.is_some() && (import.is_some() || watch.is_some() || classic_mode) {
        fail("--import-pack is exclusive with library import/watch modes");
    }
    if classic_mode && (import.is_some() || watch.is_some()) {
        fail("classic modes are exclusive with library import/watch modes");
    }
    if import_pack.is_some() && pack_out.is_none() {
        fail("--out is required with --import-pack");
    }
    if classic_mode && (source_config.is_some() || pack_source.has_pack_identity()) {
        fail("classic modes use ClassicSource identity; pack identity flags are not accepted");
    }
    if classic_mode
        && (license.is_some()
            || redistribution.is_some()
            || derivatives.is_some()
            || !credits.is_empty()
            || !source.is_empty())
    {
        fail("classic modes use the source collection and rights from ClassicSource::pack_spec");
    }
    if import_pack.is_none()
        && !classic_convert
        && (pack_out.is_some()
            || source_config.is_some()
            || pack_source.has_pack_identity())
    {
        fail("--out / --source-config / pack identity flags require --import-pack");
    }
    if classic_import && server_id.is_none() {
        fail("--server-id is required with --import-classic");
    }
    let (endpoints, token) = if import_pack.is_some() || classic_convert {
        // Pack compile is local: no server session, no bearer token.
        (
            ApiEndpoints {
                control: SocketAddr::from(([127, 0, 0, 1], 0)),
                data: SocketAddr::from(([127, 0, 0, 1], 0)),
            },
            String::new(),
        )
    } else {
        if servers.is_empty() {
            fail("--server is required");
        }
        let mut parsed = Vec::new();
        for spec in &servers {
            let Some(ep) = parse_endpoints(spec) else {
                fail("malformed --server (want ip:controlport:dataport)")
            };
            parsed.push(ep);
        }
        let endpoints = parsed.remove(0);
        let Some(token) = token else { fail("--token-file is required") };
        (endpoints, token)
    };
    let cache = cache.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(".makepad-asset-importer")
    });
    // Rights are an explicit typed operator input. Import/watch REQUIRE the
    // complete declaration — the library index records no rights and this
    // tool never invents any.
    // --import-pack uses the fuller source contract in pack_import (terms
    // digest/URL included) and must not apply this publish-rights trio, or
    // a source-config plus a lone --license would fail closed incorrectly.
    let rights = if import_pack.is_some() || classic_mode {
        None
    } else {
        match (&license, &redistribution, &derivatives) {
            (None, None, None) => {
                if import.is_some() || watch.is_some() {
                    fail(
                        "--license, --redistribution and --derivatives are required \
                         for library import/watch (rights are never invented)",
                    );
                }
                None
            }
            (Some(license), Some(redistribution), Some(derivatives)) => {
                let declared = PublishRights::declared(
                    license.clone(),
                    credits.clone(),
                    source.clone(),
                    parse_policy_redistribution(redistribution),
                    parse_policy_derivatives(derivatives),
                );
                if (declared.redistribution == Redistribution::AttributionRequired
                    || declared.derivatives == DerivativePolicy::AttributionRequired)
                    && declared.credits.is_empty()
                {
                    fail("attribution-required rights need --credits");
                }
                Some(declared)
            }
            _ => fail("--license, --redistribution and --derivatives must be stated together"),
        }
    };
    Args {
        endpoints,
        server_id,
        token,
        import,
        watch,
        import_games,
        import_music,
        namespace,
        cache,
        rights,
        import_pack,
        pack_out,
        source_config,
        pack_source,
        classic,
        log,
    }
}

fn parse_policy_redistribution(text: &str) -> Redistribution {
    match text {
        "allowed" => Redistribution::Allowed,
        "attribution-required" => Redistribution::AttributionRequired,
        "forbidden" => Redistribution::Forbidden,
        "lan-local" | "user-owned-local" => Redistribution::LanLocal,
        other => fail(&format!("unknown --redistribution policy {other}")),
    }
}

fn parse_policy_derivatives(text: &str) -> DerivativePolicy {
    match text {
        "allowed" => DerivativePolicy::Allowed,
        "attribution-required" => DerivativePolicy::AttributionRequired,
        "forbidden" => DerivativePolicy::Forbidden,
        "local-preview-only" | "local-preview" => DerivativePolicy::LocalPreview,
        other => fail(&format!("unknown --derivatives policy {other}")),
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            starts.push(parent.to_path_buf());
        }
    }
    for mut dir in starts {
        loop {
            if dir.join("local/packs").is_dir() {
                return Ok(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    Err("could not find repo root containing local/packs".into())
}

fn classic_pack_dir(root: &Path, classic: &ClassicCliArgs) -> PathBuf {
    classic
        .pack
        .clone()
        .unwrap_or_else(|| root.join("local/packs").join(classic.source.id()))
}

fn convert_stage_name(stage: classic_import::ConvertStage) -> &'static str {
    match stage {
        classic_import::ConvertStage::Expand => "expand",
        classic_import::ConvertStage::Convert => "convert",
        classic_import::ConvertStage::Ao => "ao",
    }
}

fn convert_classic_with_progress(
    pack: &Path,
    out: &Path,
    source: classic_import::ClassicSource,
) -> Result<classic_import::ClassicConvertReport, String> {
    let report = classic_import::convert_classic_ex(pack, out, source, |tick| {
        eprintln!(
            "[classic-import] {} {}/{} {}",
            convert_stage_name(tick.stage),
            tick.done,
            tick.total,
            tick.current
        );
        true
    })
    .map_err(|error| error.to_string())?;
    write_cpu_billboard_icons(out)?;
    Ok(report)
}

fn classic_kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
        AssetKind::Game => "game",
        AssetKind::VjEffect => "vj-effect",
        AssetKind::Data => "data",
        AssetKind::ModelProgram => "model-program",
    }
}

fn classic_asset_summary(assets: &[classic_import::ClassicAsset]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for asset in assets {
        *counts.entry(classic_kind_name(asset.kind)).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn write_cpu_billboard_icons(staged: &Path) -> Result<usize, String> {
    const CANVAS: u32 = 256;
    const ART_MAX: u32 = 96;
    let mut manifests = Vec::new();
    let mut stack = vec![staged.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| format!("read {}: {error}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("billboard") {
                manifests.push(path);
            }
        }
    }
    manifests.sort();
    let mut written = 0usize;
    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("read {}: {error}", manifest.display()))?;
        let billboard = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&text)
            .map_err(|error| format!("parse {}: {error}", manifest.display()))?;
        let state = billboard
            .states
            .iter()
            .find(|state| state.name.eq_ignore_ascii_case("idle"))
            .or_else(|| billboard.states.iter().find(|state| state.name == billboard.preview))
            .or_else(|| billboard.states.first());
        let Some(frame) = state
            .and_then(|state| billboard.frames.get(state.first.min(state.last)))
            .or_else(|| billboard.frames.first())
        else {
            continue;
        };
        let dir = manifest.parent().unwrap_or(staged);
        let sheet_path = dir.join(&frame.file);
        let sheet = std::fs::read(&sheet_path)
            .map_err(|error| format!("read {}: {error}", sheet_path.display()))?;
        let (rgba, sheet_w, sheet_h) = classic_import::decode_png_stored(&sheet)
            .map_err(|error| format!("decode {}: {error}", sheet_path.display()))?;
        let (sx, sy, width, height) = billboard
            .frame_rect(frame)
            .unwrap_or((0, 0, frame.w.min(sheet_w), frame.h.min(sheet_h)));
        if width == 0 || height == 0 || sx + width > sheet_w || sy + height > sheet_h {
            continue;
        }
        let scale = (ART_MAX as f32 / width as f32)
            .min(ART_MAX as f32 / height as f32);
        let draw_w = ((width as f32 * scale).round() as u32).clamp(1, ART_MAX);
        let draw_h = ((height as f32 * scale).round() as u32).clamp(1, ART_MAX);
        let ox = (CANVAS - draw_w) / 2;
        let oy = (CANVAS - draw_h) / 2;
        let mut canvas = vec![0u8; (CANVAS * CANVAS * 4) as usize];
        for y in 0..draw_h {
            let source_y = sy + y * height / draw_h;
            for x in 0..draw_w {
                let source_x = if frame.flip {
                    sx + width - 1 - x * width / draw_w
                } else {
                    sx + x * width / draw_w
                };
                let source_at = ((source_y * sheet_w + source_x) * 4) as usize;
                let dest_at = (((oy + y) * CANVAS + ox + x) * 4) as usize;
                canvas[dest_at..dest_at + 4].copy_from_slice(&rgba[source_at..source_at + 4]);
            }
        }
        let stem = manifest
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("{} has no UTF-8 stem", manifest.display()))?;
        let icon = manifest.with_file_name(format!("{stem}_thumb.png"));
        let png = classic_import::encode_png_rgba(&canvas, CANVAS, CANVAS)?;
        std::fs::write(&icon, png)
            .map_err(|error| format!("write {}: {error}", icon.display()))?;
        written += 1;
    }
    Ok(written)
}

fn flush_publish_batch(
    client: &AssetClient,
    namespace: &str,
    batch: &mut Vec<(BlobId, Vec<u8>)>,
) -> Result<usize, String> {
    if batch.is_empty() {
        return Ok(0);
    }
    let count = batch.len();
    let refs = batch
        .iter()
        .map(|(digest, bytes)| (*digest, bytes.as_slice()))
        .collect::<Vec<_>>();
    client
        .upload_blob_batch_with_digests(namespace, &refs)
        .map_err(|error| format!("upload batch of {count}: {error}"))?;
    batch.clear();
    Ok(count)
}

fn publish_classic_pack(
    client: &mut AssetClient,
    pack_root: &Path,
    out: &Path,
    source: classic_import::ClassicSource,
    pack_name: &str,
    convert_assets: &[classic_import::ClassicAsset],
) -> Result<(bool, usize), String> {
    let collection = std::fs::read(out.join(pack_import::SOURCE_COLLECTION_FILE))
        .map_err(|error| format!("read source collection: {error}"))?;
    let manifest = std::fs::read(out.join(pack_import::IMPORT_MANIFEST_FILE))
        .map_err(|error| format!("read import manifest: {error}"))?;
    let plan_bytes = std::fs::read(out.join(pack_import::UPLOAD_PLAN_FILE))
        .map_err(|error| format!("read upload plan: {error}"))?;
    let plan = makepad_asset_client::json::parse(&plan_bytes)
        .map_err(|error| format!("upload plan json: {error}"))?;
    let namespace = plan
        .get("namespace")
        .and_then(makepad_asset_client::json::Value::as_str)
        .ok_or("upload plan missing namespace")?
        .to_string();
    let blobs = plan
        .get("blobs")
        .and_then(makepad_asset_client::json::Value::as_arr)
        .ok_or("upload plan missing blobs")?;
    client
        .register_source_collection(&collection)
        .map_err(|error| format!("register source: {error}"))?;

    let mut done = 0usize;
    let mut batch = Vec::<(BlobId, Vec<u8>)>::new();
    let mut batch_bytes = 0u64;
    for blob in blobs {
        let local = blob
            .get("local_path")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing local_path")?;
        let expect = blob
            .get("blob")
            .and_then(makepad_asset_client::json::Value::as_str)
            .ok_or("blob missing digest")?;
        let digest: BlobId = expect
            .parse()
            .map_err(|_| format!("blob digest malformed for {local}: {expect}"))?;
        let bytes = std::fs::read(pack_root.join(local))
            .map_err(|error| format!("read {local}: {error}"))?;
        let size = bytes.len() as u64;
        if size > wire::UPLOAD_BATCH_SAFE_BYTES {
            done += flush_publish_batch(client, &namespace, &mut batch)?;
            batch_bytes = 0;
            client
                .upload_blob_with_digest(&namespace, &bytes, digest)
                .map_err(|error| format!("upload {local}: {error}"))?;
            done += 1;
            eprintln!("[classic-import] publish {done}/{} {local}", blobs.len());
            continue;
        }
        if !batch.is_empty()
            && (batch.len() >= wire::MAX_UPLOAD_BATCH_ITEMS
                || batch_bytes + size > wire::UPLOAD_BATCH_SAFE_BYTES)
        {
            done += flush_publish_batch(client, &namespace, &mut batch)?;
            eprintln!("[classic-import] publish {done}/{}", blobs.len());
            batch_bytes = 0;
        }
        batch.push((digest, bytes));
        batch_bytes += size;
    }
    done += flush_publish_batch(client, &namespace, &mut batch)?;
    eprintln!("[classic-import] publish {done}/{}", blobs.len());

    let imported = client
        .run_import(&manifest)
        .map_err(|error| format!("run import: {error}"))?;
    let kind_by_key = convert_assets
        .iter()
        .map(|asset| (asset.key.clone(), asset.kind))
        .collect::<BTreeMap<_, _>>();
    let tags_by_key = convert_assets
        .iter()
        .map(|asset| (asset.key.clone(), asset.tags.clone()))
        .collect::<BTreeMap<_, _>>();
    for entry in &imported.entries {
        let key = entry.key.as_str();
        let title = key.rsplit('/').next().unwrap_or(key);
        let kind = kind_by_key
            .get(key)
            .copied()
            .unwrap_or_else(|| classic_import::kind_for_staged_path(key));
        let alias = entry
            .alias
            .as_ref()
            .map(|alias| alias.as_str().to_string())
            .unwrap_or_else(|| format!("{}/{pack_name}/{title}", source.id()));
        let mut tags = vec![source.id().to_string(), pack_name.to_string()];
        for tag in tags_by_key.get(key).map(Vec::as_slice).unwrap_or(&[]) {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        let annotation = AnnotationUpload {
            title: title.to_string(),
            description: format!(
                "{} {pack_name} · {alias} · {key} · {} · {}",
                source.title(),
                source.license(),
                source.credits()
            ),
            kind: Some(kind),
            categories: vec![source.id().into(), pack_name.to_string()],
            tags,
            creator: source.credits().to_string(),
            artist: String::new(),
            artist_url: String::new(),
            album: String::new(),
            source_url: String::new(),
            license: String::new(),
            license_url: String::new(),
            generator: "classic_import".into(),
            backend: "asset-importer-cli".into(),
            model: pack_name.to_string(),
            prompt: format!("imported {} pack {pack_name} asset {key}", source.title()),
            provenance: format!(
                "{} · {} · license {} · credits {}",
                source.title(),
                source.home(),
                source.license(),
                source.credits()
            ),
            private: false,
        };
        client
            .put_annotation(&entry.asset_id, &annotation)
            .map_err(|error| format!("annotate {key}: {error}"))?;
    }
    Ok((imported.created, imported.entries.len()))
}

pub(super) fn run() {
    let args = parse_args();
    install_signal_handlers();

    // ---- headless classic conversion (no server) ----
    if let Some(classic) = args
        .classic
        .as_ref()
        .filter(|classic| classic.mode == ClassicCliMode::Convert)
    {
        let root = repo_root().unwrap_or_else(|error| fail(&error));
        let pack = classic_pack_dir(&root, classic);
        let out = classic
            .out
            .as_deref()
            .expect("parse_classic_args requires --out for conversion");
        match convert_classic_with_progress(&pack, out, classic.source) {
            Ok(report) => {
                println!(
                    "convert-classic complete: {} assets ({})",
                    report.assets.len(),
                    classic_asset_summary(&report.assets)
                );
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: convert-classic failed: {error}");
                std::process::exit(1);
            }
        }
    }

    // ---- licensed local pack compiler (no server) ----
    if let Some(dir) = args.import_pack {
        let out = args
            .pack_out
            .as_ref()
            .expect("parse_args requires --out for --import-pack");
        match pack_import::compile_pack(
            &dir,
            out,
            args.pack_source,
            args.source_config.as_deref(),
            args.log,
        ) {
            Ok(report) => {
                println!(
                    "import-pack complete: {} assets, {} blobs, source {}, revision {}",
                    report.assets,
                    report.blobs,
                    report.source_digest,
                    report.import_revision
                );
                println!("  {}", report.source_path.display());
                println!("  {}", report.manifest_path.display());
                println!("  {}", report.plan_path.display());
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: import-pack failed: {error}");
                std::process::exit(1);
            }
        }
    }

    let cache_leaf = if args.watch.is_some() {
        "watch-cache"
    } else {
        "cache"
    };
    let mut config = ClientConfig::new(args.cache.join(cache_leaf));
    config.token = Some(args.token.clone());
    let mut client = match AssetClient::connect(config, args.endpoints, args.server_id) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("makepad-asset-importer: connect failed: {error}");
            std::process::exit(1);
        }
    };
    if args.log {
        eprintln!(
            "[asset-worker] connected to {} (server {})",
            args.endpoints.control,
            &makepad_asset_client::util::to_hex(&client.server_id())[..8]
        );
    }

    // ---- headless classic conversion + licensed publish ----
    if let Some(classic) = args
        .classic
        .as_ref()
        .filter(|classic| classic.mode == ClassicCliMode::Import)
    {
        let result = (|| -> Result<(usize, usize, bool, usize), String> {
            let root = repo_root()?;
            let pack = classic_pack_dir(&root, classic);
            let work = root
                .join("local/asset-importer")
                .join(format!("classic-{}", classic.source.id()));
            let staged = work.join("source");
            // The compiled bundle must not share an existing ancestor with the
            // staged pack (pack_import refuses an --out whose nearest existing
            // ancestor contains the pack), so it lives in a sibling directory
            // that is created up front.
            let bundle_root = root
                .join("local/asset-importer")
                .join(format!("classic-{}-out", classic.source.id()));
            std::fs::create_dir_all(&bundle_root)
                .map_err(|error| format!("create {}: {error}", bundle_root.display()))?;
            let bundle = bundle_root.join("bundle");
            let mut converted_assets = Vec::new();
            if classic.fresh && work.exists() {
                std::fs::remove_dir_all(&work)
                    .map_err(|error| format!("clear {}: {error}", work.display()))?;
            }
            if staged.is_dir() {
                eprintln!("[classic-import] reusing {}", staged.display());
                write_cpu_billboard_icons(&staged)?;
            } else {
                let report = convert_classic_with_progress(&pack, &staged, classic.source)?;
                converted_assets = report.assets;
            }
            if bundle.exists() {
                std::fs::remove_dir_all(&bundle)
                    .map_err(|error| format!("clear {}: {error}", bundle.display()))?;
            }
            let report = pack_import::compile_pack(
                &staged,
                &bundle,
                classic.source.pack_spec(classic.source.id()),
                None,
                args.log,
            )
            .map_err(|error| error.to_string())?;
            let (created, annotated) = publish_classic_pack(
                &mut client,
                &staged,
                &bundle,
                classic.source,
                classic.source.id(),
                &converted_assets,
            )?;
            Ok((report.assets, report.blobs, created, annotated))
        })();
        match result {
            Ok((assets, blobs, created, annotated)) => {
                println!(
                    "import-classic complete: {assets} assets, {blobs} blobs, created={created}, annotated={annotated}"
                );
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: import-classic failed: {error}");
                std::process::exit(1);
            }
        }
    }

    // ---- import mode ----
    if let Some(dir) = args.import {
        let rights = args.rights.expect("parse_args enforces rights for import mode");
        match import::import_library(&mut client, &dir, &args.namespace, &rights, args.log) {
            Ok(report) => {
                println!(
                    "import complete: {} published, {} already present, {} skipped by kind, \
                     {} not generated, {} failed",
                    report.published.len(),
                    report.skipped_existing.len(),
                    report.skipped_kind.len(),
                    report.skipped_scope.len(),
                    report.failed.len()
                );
                for (file, error) in &report.failed {
                    println!("  FAILED {file}: {error}");
                }
                std::process::exit(if report.failed.is_empty() { 0 } else { 1 });
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: import failed: {error}");
                std::process::exit(1);
            }
        }
    }

    // ---- games import mode ----
    if let Some(dir) = args.import_games {
        // Own-authored games carry the named CC0 grant unless the operator
        // declared otherwise; the trio is still honoured when present.
        let rights = args.rights.clone().unwrap_or_else(PublishRights::generated_cc0);
        match games_import::import_games(&mut client, &dir, &args.namespace, &rights, args.log) {
            Ok(report) => {
                println!(
                    "games import complete: {} published, {} updated, {} unchanged, {} failed",
                    report.published.len(),
                    report.updated.len(),
                    report.unchanged.len(),
                    report.failed.len()
                );
                for (slug, error) in &report.failed {
                    println!("  FAILED {slug}: {error}");
                }
                std::process::exit(if report.failed.is_empty() { 0 } else { 1 });
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: games import failed: {error}");
                std::process::exit(1);
            }
        }
    }

    // ---- music directory import mode ----
    if let Some(dir) = args.import_music {
        // A personal library states the honest terms unless the operator
        // declared better ones: held locally, servable on this LAN, never
        // redistributed off it.
        let rights = args
            .rights
            .clone()
            .unwrap_or_else(|| music_import::personal_library_rights(&dir));
        let mut progress = |p: music_import::MusicProgress| {
            if args.log && !p.current.is_empty() {
                let stage = match p.stage {
                    music_import::MusicStage::Reading => "reading",
                    music_import::MusicStage::Publishing => "publishing",
                };
                eprintln!("[music-import] {stage} {}/{} {}", p.done, p.total, p.current);
            }
        };
        let cancel = || STOP.load(Ordering::SeqCst);
        match music_import::import_music(
            &mut client,
            &dir,
            &args.namespace,
            &rights,
            args.log,
            &mut progress,
            &cancel,
        ) {
            Ok(report) => {
                println!(
                    "music import complete: {} published, {} updated, {} unchanged, \
                     {} skipped, {} failed{}",
                    report.published.len(),
                    report.updated.len(),
                    report.unchanged.len(),
                    report.skipped.len(),
                    report.failed.len(),
                    if report.cancelled { " (cancelled)" } else { "" }
                );
                for (rel, reason) in &report.skipped {
                    println!("  skipped {rel}: {reason}");
                }
                for (rel, error) in &report.failed {
                    println!("  FAILED {rel}: {error}");
                }
                std::process::exit(if report.failed.is_empty() { 0 } else { 1 });
            }
            Err(error) => {
                eprintln!("makepad-asset-importer: music import failed: {error}");
                std::process::exit(1);
            }
        }
    }

    // ---- continuous import mode ----
    if let Some(dir) = args.watch {
        if args.log {
            eprintln!("[asset-worker] watching {}", dir.display());
        }
        let rights = args.rights.expect("parse_args enforces rights for watch mode");
        watch::run(&mut client, &dir, &args.namespace, &rights, args.log, &STOP);
        if args.log {
            eprintln!("[asset-worker] watch stopped");
        }
        return;
    }

    // No mode flag matched. The generation coordinator that used to run
    // here is gone (aicore: apps drive the fleet themselves through
    // makepad-asset-creator); this tool only imports.
    fail("pick a mode: --import-ai-library / --watch-ai-library / --import-games / --import-music / --import-pack / --convert-classic / --import-classic");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn convert_classic_arg_parsing_is_hermetic() {
        let parsed = parse_classic_args(&strings(&[
            "--convert-classic",
            "cnc",
            "--pack",
            "fixture-pack",
            "--out",
            "fixture-out",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.mode, ClassicCliMode::Convert);
        assert_eq!(parsed.source, classic_import::ClassicSource::Cnc);
        assert_eq!(parsed.pack, Some(PathBuf::from("fixture-pack")));
        assert_eq!(parsed.out, Some(PathBuf::from("fixture-out")));
        assert!(!parsed.fresh);

        assert!(parse_classic_args(&strings(&["--convert-classic", "doom"]))
            .unwrap_err()
            .contains("--out"));
        assert!(parse_classic_args(&strings(&[
            "--import-classic",
            "ra",
            "--out",
            "not-allowed",
        ]))
        .unwrap_err()
        .contains("--out"));
    }

    #[test]
    fn classic_cli_billboard_icon_is_cpu_composited_with_bounded_art() {
        use makepad_asset_importer::stateful_billboard::{
            AnimState, SpriteFrame, SpriteRole, StatefulBillboard,
        };

        let staged = std::env::temp_dir().join(format!(
            "makepad-classic-cli-icon-{}",
            std::process::id()
        ));
        let actor_dir = staged.join("billboards/test");
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::create_dir_all(&actor_dir).unwrap();
        let mut rgba = vec![0u8; 20 * 10 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[220, 80, 40, 255]);
        }
        std::fs::write(
            actor_dir.join("actor.png"),
            classic_import::encode_png_rgba(&rgba, 20, 10).unwrap(),
        )
        .unwrap();
        let billboard = StatefulBillboard {
            prefix: "actor".into(),
            role: SpriteRole::Unit,
            preview: "idle".into(),
            facings: 1,
            states: vec![AnimState {
                name: "idle".into(),
                first: 0,
                last: 1,
                r#loop: true,
                fps: 1,
            }],
            frames: vec![SpriteFrame {
                letter: 'A',
                rot: 1,
                w: 20,
                h: 10,
                file: "actor.png".into(),
                flip: false,
                cell: None,
            }],
            ..Default::default()
        };
        std::fs::write(actor_dir.join("actor.billboard"), billboard.to_text()).unwrap();

        assert_eq!(write_cpu_billboard_icons(&staged).unwrap(), 1);
        let icon = std::fs::read(actor_dir.join("actor_thumb.png")).unwrap();
        let (pixels, width, height) = classic_import::decode_png_stored(&icon).unwrap();
        assert_eq!((width, height), (256, 256));
        let painted = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| pixel[3] != 0)
            .map(|(index, _)| ((index as u32) % width, (index as u32) / width))
            .collect::<Vec<_>>();
        let min_x = painted.iter().map(|(x, _)| *x).min().unwrap();
        let max_x = painted.iter().map(|(x, _)| *x).max().unwrap();
        let min_y = painted.iter().map(|(_, y)| *y).min().unwrap();
        let max_y = painted.iter().map(|(_, y)| *y).max().unwrap();
        assert!(max_x - min_x + 1 <= 96);
        assert!(max_y - min_y + 1 <= 96);
        assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
        let _ = std::fs::remove_dir_all(staged);
    }
}

}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::run();
}
