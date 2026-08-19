//! Makepad Asset Worker — the headless process beside the Asset Server.
//!
//! Default mode: the claim→dispatch→publish coordinator for every wired
//! generation kind (see `gen_kinds.rs` and `coordinator.rs`), running one
//! claim loop per fleet box and advertising the fleet's currently
//! executable job profiles to each server it is attached to. With
//! `--import-ai-library <dir>` it instead runs one idempotent import pass
//! over the GENERATED rows of an existing ai-content library (see the lib's
//! `import` module) and exits. With `--watch-ai-library <dir>` it
//! continuously publishes stable new/changed library artifacts until clean
//! shutdown (see the lib's `watch` module). With
//! `--import-pack <dir>` it compiles a licensed local pack into canonical
//! `SourceCollection` / `ImportManifest` bytes plus a local upload plan
//! and exits without contacting the server (see `pack_import.rs`).
//!
//! Credentials never leave this host: the bearer token is read from a
//! local file (typically the server root's `admin-token`, or a scoped
//! worker token minted for `job_worker`/publish capabilities).

// The derivation seam (Deriver trait + DeriveCoordinator) has no production
// deriver wired into a CLI mode yet — the scripted contract executor lives
// in its tests; compiling it always keeps the seam from rotting.
#[cfg_attr(not(test), allow(dead_code))]
mod derive;
mod depth_from_image;
mod mesh_from_image;

// The library importer/watcher now lives in the crate's lib so the Asset UI
// can run the same publication loop in-process against its embedded server.
use makepad_asset_importer::{games_import, import, pack_import, watch};

use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig, PublishRights};
use makepad_asset_data::{DerivativePolicy, Redistribution};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const USAGE: &str = "\
makepad-asset-importer --server <ip:controlport:dataport> --token-file <path> [options]
makepad-asset-importer --import-pack <dir> --out <dir> --source-config <file>
makepad-asset-importer --import-pack <dir> --out <dir> [source/rights flags]

Modes:
  (default)                     Claim + dispatch every wired generation kind
                                (image/video/audio/music/speech/mesh/splat/
                                world/edit/upscale/... — see gen_kinds) to the
                                GPU fleet, one claim loop per fleet box;
                                publish results to the catalog and advertise
                                the executable profiles to the server(s).
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
  --import-pack <dir>           Compile a licensed local pack (Kenney, …)
                                into canonical SourceCollection and
                                ImportManifest bytes plus a local upload
                                plan, then exit. Does not contact the
                                server. Supported files: png/jpeg/wav/mp4/glb.
  --mesh-from-image             Claim op.mesh.from_image.v1 operation jobs
                                and execute them on the fleet's TRELLIS
                                image->mesh model; results land through the
                                server's atomic operation finalizer.
  --depth-from-image            Claim op.depth.from_image.v1 operation jobs
                                and execute them on the fleet's native DA3
                                metric-depth model; results land through the
                                server's atomic operation finalizer.

Options:
  --server <ip:ctrl:data>       Asset Server endpoints. Repeatable so one
                                worker claims from every live server. Or env
                                ASSET_WORKER_SERVER / ASSET_WORKER_SERVERS
                                (comma-separated). Required except --import-pack.
  --token-file <path>           File holding the bearer token (mpat_…).
                                Required (or env ASSET_WORKER_TOKEN) except
                                --import-pack.
  --server-id <32 hex>          Pin the server identity.
  --fleet <path>                Optional URL list of GPU boxes. Default:
                                LAN UDP discovery (same beacon as AI Content).
  --namespace <ns>              Namespace for imports (default gen;
                                --import-games defaults to sandbox).
  --suffix <name>               Worker lease suffix (default w1).
  --cache <dir>                 Client cache root (default
                                ~/.makepad-asset-importer). Watch mode owns a
                                separate child so it can coexist with the
                                coordinator's single-owner cache.
  --out <dir>                   Output directory for --import-pack.
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
                                Optional in coordinator mode (generated
                                output defaults to the named CC0 grant).
  --redistribution <policy>     allowed | attribution-required | forbidden.
                                Required with --license.
  --derivatives <policy>        allowed | attribution-required | forbidden.
                                Required with --license.
  --credits <text>              Required attribution line (mandatory when a
                                policy is attribution-required; required
                                for --import-pack).
  --source <text>               Upstream origin (pack name/URL) of the
                                imported content. Required for --import-pack.
  --once                        Coordinator: process at most one job, exit.
  --no-announce                 Coordinator: do not advertise this fleet's
                                executable job profiles to the server(s).
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
    extra_servers: Vec<ApiEndpoints>,
    server_id: Option<[u8; 16]>,
    token: String,
    fleet: Option<PathBuf>,
    import: Option<PathBuf>,
    watch: Option<PathBuf>,
    /// Splash game folders -> `game` assets. Exclusive with every other mode.
    import_games: Option<PathBuf>,
    namespace: String,
    suffix: String,
    cache: PathBuf,
    /// The operator's explicit rights declaration. `None` only in
    /// coordinator mode, where generated output uses the named CC0 grant.
    rights: Option<PublishRights>,
    /// Run the mesh.from_image operation executor instead of the video
    /// coordinator. Rights are never declared here: operation outputs
    /// inherit their input's rights server-side.
    mesh_ops: bool,
    /// Run the depth.from_image operation executor.
    depth_ops: bool,
    /// Local licensed-pack compiler. Exclusive with every networked mode.
    import_pack: Option<PathBuf>,
    pack_out: Option<PathBuf>,
    source_config: Option<PathBuf>,
    pack_source: pack_import::PackSourceSpec,
    once: bool,
    /// Skip the live profile advertisement (a worker that only drains a
    /// queue and must not touch what the server offers).
    no_announce: bool,
    log: bool,
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
    let mut args = std::env::args().skip(1);
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
    let mut fleet: Option<PathBuf> = None;
    let mut import = None;
    let mut watch = None;
    let mut import_games = None;
    let mut namespace: Option<String> = None;
    let mut suffix = "w1".to_string();
    let mut cache: Option<PathBuf> = None;
    let mut license: Option<String> = None;
    let mut redistribution: Option<String> = None;
    let mut derivatives: Option<String> = None;
    let mut credits = String::new();
    let mut source = String::new();
    let mut mesh_ops = false;
    let mut depth_ops = false;
    let mut import_pack = None;
    let mut pack_out = None;
    let mut source_config = None;
    let mut pack_source = pack_import::PackSourceSpec::default();
    let mut once = false;
    let mut no_announce = false;
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
            "--fleet" => fleet = Some(PathBuf::from(value_of("--fleet", &mut args))),
            "--import-ai-library" => {
                import = Some(PathBuf::from(value_of("--import-ai-library", &mut args)))
            }
            "--watch-ai-library" => {
                watch = Some(PathBuf::from(value_of("--watch-ai-library", &mut args)))
            }
            "--import-games" => {
                import_games = Some(PathBuf::from(value_of("--import-games", &mut args)))
            }
            "--import-pack" => {
                import_pack = Some(PathBuf::from(value_of("--import-pack", &mut args)))
            }
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
            "--suffix" => suffix = value_of("--suffix", &mut args),
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
            "--mesh-from-image" => mesh_ops = true,
            "--depth-from-image" => depth_ops = true,
            "--once" => once = true,
            "--no-announce" => no_announce = true,
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
    if import.is_some() && watch.is_some() {
        fail("--import-ai-library and --watch-ai-library are mutually exclusive");
    }
    if import_games.is_some()
        && (import.is_some()
            || watch.is_some()
            || import_pack.is_some()
            || mesh_ops
            || depth_ops
            || once)
    {
        fail("--import-games is exclusive with every other mode and --once");
    }
    let namespace = namespace.unwrap_or_else(|| {
        if import_games.is_some() { "sandbox" } else { "gen" }.to_string()
    });
    if mesh_ops && (import.is_some() || watch.is_some() || import_pack.is_some()) {
        fail("--mesh-from-image is exclusive with library import/watch/pack modes");
    }
    if depth_ops && (import.is_some() || watch.is_some() || import_pack.is_some()) {
        fail("--depth-from-image is exclusive with library import/watch/pack modes");
    }
    if mesh_ops && depth_ops {
        fail("--mesh-from-image and --depth-from-image are mutually exclusive");
    }
    if import_pack.is_some() && (import.is_some() || watch.is_some()) {
        fail("--import-pack is exclusive with library import/watch modes");
    }
    if import_pack.is_some() && once {
        fail("--once is only valid in coordinator mode");
    }
    if watch.is_some() && once {
        fail("--once is only valid in coordinator mode");
    }
    if import_pack.is_some() && pack_out.is_none() {
        fail("--out is required with --import-pack");
    }
    if import_pack.is_none()
        && (pack_out.is_some()
            || source_config.is_some()
            || pack_source.has_pack_identity())
    {
        fail("--out / --source-config / pack identity flags require --import-pack");
    }
    let (endpoints, extra_servers, token) = if import_pack.is_some() {
        // Pack compile is local: no server session, no bearer token.
        (
            ApiEndpoints {
                control: SocketAddr::from(([127, 0, 0, 1], 0)),
                data: SocketAddr::from(([127, 0, 0, 1], 0)),
            },
            Vec::new(),
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
        let extra_servers = parsed;
        let Some(token) = token else { fail("--token-file is required") };
        (endpoints, extra_servers, token)
    };
    let fleet = fleet.or_else(|| std::env::var("MAKEPAD_AI_FLEET").ok().map(PathBuf::from));
    let cache = cache.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(".makepad-asset-importer")
    });
    // Rights are an explicit typed operator input. Import/watch REQUIRE the
    // complete declaration — the library index records no rights and this
    // worker never invents any. Coordinator mode may omit it; generated
    // output then uses the NAMED CC0 grant, not a hidden default.
    // --import-pack uses the fuller source contract in pack_import (terms
    // digest/URL included) and must not apply this publish-rights trio, or
    // a source-config plus a lone --license would fail closed incorrectly.
    let rights = if import_pack.is_some() {
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
        extra_servers,
        server_id,
        token,
        fleet,
        import,
        watch,
        import_games,
        namespace,
        suffix,
        cache,
        rights,
        mesh_ops,
        depth_ops,
        import_pack,
        pack_out,
        source_config,
        pack_source,
        once,
        no_announce,
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

fn main() {
    let args = parse_args();
    install_signal_handlers();

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

    // ---- depth.from_image operation executor mode ----
    if args.depth_ops {
        let mut fleet = match args.fleet.as_deref() {
            Some(path) => {
                match depth_from_image::AssetAiDepthFleet::from_fleet_file(path, args.log) {
                    Ok(fleet) => fleet,
                    Err(error) => {
                        eprintln!("makepad-asset-importer: {error}");
                        std::process::exit(1);
                    }
                }
            }
            None => depth_from_image::AssetAiDepthFleet::from_lan(args.log),
        };
        let mut coordinator = depth_from_image::DepthOpCoordinator {
            client,
            fleet: &mut fleet,
            suffix: args.suffix,
            log: args.log,
        };
        if args.once {
            match coordinator.run_one(&STOP) {
                Ok(Some(outcome)) => println!("processed one operation job: {outcome:?}"),
                Ok(None) => println!("queue empty"),
                Err(error) => {
                    eprintln!("makepad-asset-importer: {error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        coordinator.run(&STOP);
        if args.log {
            eprintln!("[asset-worker] stopped");
        }
        return;
    }

    // ---- mesh.from_image operation executor mode ----
    if args.mesh_ops {
        let mut fleet = match args.fleet.as_deref() {
            Some(path) => match mesh_from_image::AssetAiMeshFleet::from_fleet_file(path, args.log)
            {
                Ok(fleet) => fleet,
                Err(error) => {
                    eprintln!("makepad-asset-importer: {error}");
                    std::process::exit(1);
                }
            },
            None => mesh_from_image::AssetAiMeshFleet::from_lan(args.log),
        };
        let mut coordinator = mesh_from_image::MeshOpCoordinator {
            client,
            fleet: &mut fleet,
            suffix: args.suffix,
            log: args.log,
        };
        if args.once {
            match coordinator.run_one(&STOP) {
                Ok(Some(outcome)) => println!("processed one operation job: {outcome:?}"),
                Ok(None) => println!("queue empty"),
                Err(error) => {
                    eprintln!("makepad-asset-importer: {error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        coordinator.run(&STOP);
        if args.log {
            eprintln!("[asset-worker] stopped");
        }
        return;
    }

    // ---- coordinator mode ----
    // The client built above only proves the endpoints/token work; the
    // per-box claim loops each own their own connection and cache.
    drop(client);
    let mut servers = vec![args.endpoints];
    servers.extend(args.extra_servers.iter().copied());
    let rights = args.rights.clone().unwrap_or_else(PublishRights::generated_cc0);
    run_fleet_coordinator(&args, &servers, rights);
}

/// Discover the GPU fleet and drive it through the shared generation
/// service: one claim loop PER BOX per server (so N queued jobs of a kind
/// fan out over the N boxes that serve it, one job per box), plus the
/// profile announcer that keeps each server's advertisement in step with
/// what the fleet can execute right now.
fn run_fleet_coordinator(args: &Args, servers: &[ApiEndpoints], rights: PublishRights) {
    use makepad_asset_importer::gen_service::{FleetSource, GenServiceConfig};
    let config = GenServiceConfig {
        servers: servers.to_vec(),
        server_id: args.server_id,
        token: args.token.clone(),
        cache_root: args.cache.clone(),
        namespace: args.namespace.clone(),
        suffix: args.suffix.clone(),
        rights,
        fleet: match args.fleet.as_deref() {
            Some(path) => FleetSource::File(path.to_path_buf()),
            None => FleetSource::Lan,
        },
        announce: !args.no_announce,
        log: args.log,
    };
    if args.once {
        match makepad_asset_importer::gen_service::run_once(&config, &STOP) {
            Ok(Some(outcome)) => println!("processed one job: {outcome}"),
            Ok(None) => println!("queue empty"),
            Err(error) => {
                eprintln!("makepad-asset-importer: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    makepad_asset_importer::gen_service::run(&config, &STOP);
}
