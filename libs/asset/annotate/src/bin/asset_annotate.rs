//! The annotation pass runner.
//!
//! Reads the store's catalog to pick assets, pulls each one's turntable sheet
//! over the store's data plane, hands the batch to a vision executor, and
//! publishes the parsed result back through `PUT /v1/assets/{id}/annotation`.
//!
//! Nothing here creates an asset revision, uploads a blob, or touches an
//! alias: the pass writes only the mutable annotation record, so a full wipe
//! and redo of annotations leaves the imported library byte-identical.
//! `--verify-nondestructive` asserts exactly that.
//!
//! The executor is a subprocess speaking the batch protocol in
//! `libs/ai/llm/src/bin/vlm_annotate.rs`. That is the one seam to change when
//! the model moves from Metal-local to the CUDA fleet: point `--executor` at a
//! different program and bump `--version`.

use makepad_asset_annotate::pass::{self, SheetPrep};
use makepad_asset_annotate::plan::{Annotator, BaseAnnotation};
use makepad_asset_annotate::{
    executor, needs_annotation, parse_record, plan_upload, worker, ANNOTATOR_VERSION,
};
use makepad_asset_client::api::{AnnotationUpload, Api, ApiEndpoints};
use makepad_asset_client::http::HttpLimits;
use makepad_asset_data::{AssetId, AssetKind};
use makepad_sqlite::{Database, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const DEFAULT_STORE: &str = "local/asset-ui/asset-server";

struct Config {
    store: PathBuf,
    work: PathBuf,
    kit: Option<String>,
    kind: Option<String>,
    person: bool,
    aliases: Vec<String>,
    limit: usize,
    sheet_size: usize,
    exposure: f32,
    version: u32,
    model_tag: String,
    executor: Vec<String>,
    all: bool,
    dry_run: bool,
    /// Curation: rewrite ONLY the catalog kind of the selected assets (no
    /// VLM, everything else carried). `quake3 rig fragments published as
    /// kind=character` is the shape this exists for.
    set_kind: Option<String>,
    verify: bool,
    wipe: bool,
    /// Print `<kit>\t<live>\t<needing>` for every kit of `source` and exit.
    /// The asset-ui import queue's "Annotate all" reads this instead of
    /// linking a SQL engine of its own.
    list_kits: bool,
    /// The collection label whose kits are listed (`kenney`).
    source: String,
    /// Drain the store's `annotate.asset` job queue instead of running one
    /// filtered pass. This is the mode that matters: the queue is central,
    /// the server fills it on every publish, and nobody has to remember to
    /// run anything.
    worker: bool,
    /// Sheets per executor invocation in worker mode.
    batch: usize,
}

fn usage() -> ! {
    eprintln!(
        "usage: makepad-asset-annotate [options]

  --store DIR         asset-server state dir (default {DEFAULT_STORE})
  --work DIR          scratch dir for sheets and batch files
  --kit NAME          annotate assets carrying this category label
  --kind K            annotate assets of this catalog kind (e.g. character)
  --person            person-description prompt variant (for characters)
  --alias A           annotate exactly this canon alias (repeatable)
  --limit N           stop after N assets (default 10)
  --sheet-size N      downscale sheets to NxN before the model (default 512)
  --exposure G        gamma lift on subject pixels, 1.0 disables (default 1.8)
  --version N         annotator version (default {ANNOTATOR_VERSION})
  --model-tag SLUG    label-safe model identity (default qwen35-9b)
  --executor CMD...   executor argv; everything after it is the command
  --all               re-annotate even assets already at this version
  --dry-run           do everything except the publish
  --verify-nondestructive  snapshot the import, wipe+redo, prove it untouched
  --wipe              remove this pass's tags and clear its descriptions
  --worker            drain the store's annotate.asset job queue and stay
  --batch N           sheets per executor invocation in --worker (default 16)
  --list-kits         print `<kit>\\t<live>\\t<needing>` per kit and exit
  --source LABEL      collection label --list-kits groups under (default kenney)
"
    );
    std::process::exit(2)
}

fn parse_config() -> Config {
    let mut c = Config {
        store: PathBuf::from(DEFAULT_STORE),
        work: PathBuf::from("local/annotate"),
        kit: None,
        kind: None,
        person: false,
        aliases: Vec::new(),
        limit: 10,
        sheet_size: 512,
        exposure: 1.8,
        version: ANNOTATOR_VERSION,
        model_tag: "qwen35-9b".to_string(),
        executor: Vec::new(),
        all: false,
        dry_run: false,
        set_kind: None,
        verify: false,
        wipe: false,
        list_kits: false,
        source: "kenney".to_string(),
        worker: false,
        batch: 16,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let mut next = |i: &mut usize| -> String {
            *i += 1;
            if *i >= args.len() {
                usage();
            }
            args[*i].clone()
        };
        match args[i].as_str() {
            "--store" => c.store = PathBuf::from(next(&mut i)),
            "--work" => c.work = PathBuf::from(next(&mut i)),
            "--kit" => c.kit = Some(next(&mut i)),
            "--kind" => c.kind = Some(next(&mut i)),
            "--person" => c.person = true,
            "--alias" => c.aliases.push(next(&mut i)),
            "--limit" => c.limit = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--sheet-size" => c.sheet_size = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--exposure" => c.exposure = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--version" => c.version = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--model-tag" => c.model_tag = next(&mut i),
            "--all" => c.all = true,
            "--dry-run" => c.dry_run = true,
            "--set-kind" => c.set_kind = Some(next(&mut i)),
            "--verify-nondestructive" => c.verify = true,
            "--wipe" => c.wipe = true,
            "--worker" => c.worker = true,
            "--batch" => c.batch = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--list-kits" => c.list_kits = true,
            "--source" => c.source = next(&mut i),
            "--executor" => {
                c.executor = args[i + 1..].to_vec();
                if c.executor.is_empty() {
                    usage();
                }
                break;
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("unknown argument {other}");
                usage();
            }
        }
        i += 1;
    }
    c
}

// ---- store access ----------------------------------------------------------

/// One asset the pass may work on, read from the catalog.
struct Candidate {
    asset_hex: String,
    alias: String,
    base: BaseAnnotation,
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(t) => t.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Blob(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        Value::Null => String::new(),
    }
}

fn rows(db: &mut Database, sql: &str) -> Result<Vec<Vec<Value>>, String> {
    let stmt = db.prepare(sql).map_err(|e| format!("prepare: {e}"))?;
    let mut out = Vec::new();
    stmt.for_each(db, &[], |row| {
        out.push(row.to_vec());
        Ok(true)
    })
    .map_err(|e| format!("query: {e}"))?;
    Ok(out)
}

fn open_catalog(store: &Path) -> Result<Database, String> {
    let path = store.join("catalog.sqlite3");
    let mut db = Database::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
    db.refresh().map_err(|e| format!("refresh: {e}"))?;
    Ok(db)
}

/// The WHERE clause selecting the pass's working set.
///
/// The filters AND together. They used to be an if/else ladder, which made
/// "the characters of the mini-dungeon kit" inexpressible — and that is
/// exactly the second pass every kit needs (`--kit X --kind character
/// --person`), so the on-import job could not have been written without it.
/// `--alias` stays absolute: naming assets by alias means those assets and
/// nothing else.
fn candidate_filter(
    aliases: &[String],
    kit: Option<&str>,
    kind: Option<&str>,
) -> Result<String, String> {
    if !aliases.is_empty() {
        let list = aliases
            .iter()
            .map(|a| format!("'{}'", a.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        return Ok(format!("a.canon_alias IN ({list})"));
    }
    let mut clauses: Vec<String> = Vec::new();
    if let Some(kit) = kit {
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM search_labels l WHERE l.asset_id = a.asset_id \
             AND l.kind = 'category' AND l.label = '{}')",
            kit.replace('\'', "''")
        ));
    }
    if let Some(kind) = kind {
        clauses.push(format!("a.kind = '{}'", kind.replace('\'', "''")));
    }
    if clauses.is_empty() {
        return Err("need --kit, --kind or --alias".to_string());
    }
    Ok(clauses.join(" AND "))
}

/// Load candidates plus their current annotation and tag set.
fn load_candidates(db: &mut Database, cfg: &Config) -> Result<Vec<Candidate>, String> {
    let filter = candidate_filter(&cfg.aliases, cfg.kit.as_deref(), cfg.kind.as_deref())?;
    let sql = format!(
        "SELECT hex(a.asset_id), a.canon_alias, a.title, a.description, a.kind, \
                a.creator, a.generator, a.backend, a.model, a.prompt, a.provenance, a.visibility \
         FROM search_annotations a \
         WHERE a.live = 1 AND a.canon_alias <> '' AND {filter} \
         ORDER BY a.canon_alias"
    );
    let mut out = Vec::new();
    for r in rows(db, &sql)? {
        let asset_hex = text(&r[0]);
        let labels = rows(
            db,
            &format!(
                "SELECT kind, label FROM search_labels WHERE asset_id = x'{asset_hex}' ORDER BY kind, label"
            ),
        )?;
        let mut categories = Vec::new();
        let mut tags = Vec::new();
        for l in &labels {
            match text(&l[0]).as_str() {
                "category" => categories.push(text(&l[1])),
                "tag" => tags.push(text(&l[1])),
                _ => {}
            }
        }
        let kind = text(&r[4]);
        out.push(Candidate {
            asset_hex,
            alias: text(&r[1]),
            base: BaseAnnotation {
                title: text(&r[2]),
                description: text(&r[3]),
                kind: (!kind.is_empty()).then_some(kind),
                categories,
                tags,
                creator: text(&r[5]),
                generator: text(&r[6]),
                backend: text(&r[7]),
                model: text(&r[8]),
                prompt: text(&r[9]),
                provenance: text(&r[10]),
                private: text(&r[11]) == "private",
            },
        });
    }
    Ok(out)
}

/// Every kit of `source` with its live-asset count and how many of those
/// still need this annotator version.
///
/// Three flat queries and the intersection in Rust rather than one grouped
/// join: the catalog engine here is our own, and the pass has no business
/// being the first caller to need GROUP BY.
fn list_kits(db: &mut Database, cfg: &Config) -> Result<(), String> {
    let live: std::collections::BTreeSet<String> = rows(
        db,
        "SELECT hex(asset_id) FROM search_annotations WHERE live = 1 AND canon_alias <> ''",
    )?
    .iter()
    .map(|r| text(&r[0]))
    .collect();
    let done: std::collections::BTreeSet<String> = rows(
        db,
        &format!(
            "SELECT hex(asset_id) FROM search_labels WHERE kind = 'tag' AND label = 'vlm-v{}'",
            cfg.version
        ),
    )?
    .iter()
    .map(|r| text(&r[0]))
    .collect();
    let mut by_asset: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in rows(db, "SELECT hex(asset_id), label FROM search_labels WHERE kind = 'category'")? {
        let id = text(&r[0]);
        if !live.contains(&id) {
            continue;
        }
        by_asset.entry(id).or_default().push(text(&r[1]));
    }
    let mut kits: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (id, labels) in &by_asset {
        if !labels.iter().any(|l| l == &cfg.source) {
            continue;
        }
        for label in labels.iter().filter(|l| *l != &cfg.source) {
            let e = kits.entry(label.clone()).or_insert((0, 0));
            e.0 += 1;
            if !done.contains(id) {
                e.1 += 1;
            }
        }
    }
    for (kit, (total, need)) in kits {
        println!("{kit}\t{total}\t{need}");
    }
    Ok(())
}

fn read_listen(store: &Path) -> Result<(SocketAddr, SocketAddr), String> {
    let raw = std::fs::read_to_string(store.join("listen"))
        .map_err(|e| format!("read listen: {e}"))?;
    let parts: Vec<&str> = raw.trim().split(':').collect();
    if parts.len() != 3 {
        return Err(format!("malformed listen file {raw:?}"));
    }
    let control = format!("{}:{}", parts[0], parts[1])
        .parse()
        .map_err(|e| format!("control addr: {e}"))?;
    let data = format!("{}:{}", parts[0], parts[2])
        .parse()
        .map_err(|e| format!("data addr: {e}"))?;
    Ok((control, data))
}

// ---- the pass --------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cfg = parse_config();
    if cfg.list_kits {
        // No server, no token: listing is a read of the catalog file.
        let mut db = open_catalog(&cfg.store)?;
        return list_kits(&mut db, &cfg);
    }
    let token = std::fs::read_to_string(cfg.store.join("admin-token"))
        .map_err(|e| format!("read admin-token: {e}"))?
        .trim()
        .to_string();
    let (control, data) = read_listen(&cfg.store)?;
    let api = Api::new(
        ApiEndpoints { control, data },
        HttpLimits::default_v1(),
        Some(token.clone()),
    )
    .map_err(|e| format!("api: {e:?}"))?;
    if cfg.worker {
        return run_worker(&cfg, token, control, data);
    }
    let annotator = Annotator { version: cfg.version, model: cfg.model_tag.clone() };
    let mut db = open_catalog(&cfg.store)?;

    if cfg.verify {
        return verify_nondestructive(&mut db, &cfg, &api, &annotator, &token, data);
    }

    let candidates = load_candidates(&mut db, &cfg)?;
    if let Some(kind) = &cfg.set_kind {
        return set_kind(&api, &candidates, kind, cfg.dry_run);
    }
    if cfg.wipe {
        return wipe(&api, &candidates, cfg.dry_run);
    }

    let todo: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| cfg.all || needs_annotation(&c.base.tags, &annotator))
        .take(cfg.limit)
        .collect();
    println!(
        "{} candidates, {} need annotation at v{} (limit {})",
        candidates.len(),
        todo.len(),
        cfg.version,
        cfg.limit
    );
    if todo.is_empty() {
        return Ok(());
    }

    let replies = run_executor(&cfg, &api, &todo, &token, data)?;

    let mut published = 0usize;
    let mut skipped = 0usize;
    for c in &todo {
        let Some(reply) = replies.get(&c.asset_hex) else {
            eprintln!("  {} — no reply", c.alias);
            skipped += 1;
            continue;
        };
        let rec = parse_record(reply);
        if !rec.is_useful() {
            eprintln!("  {} — unusable reply, left unannotated", c.alias);
            skipped += 1;
            continue;
        }
        let up = plan_upload(&c.base, &rec, &annotator);
        println!("  {} -> {}", c.alias, up.description);
        if cfg.dry_run {
            continue;
        }
        put(&api, &c.asset_hex, &up)?;
        published += 1;
    }
    println!("published {published}, skipped {skipped}");
    Ok(())
}

/// `--worker`: claim, annotate, publish, repeat, until killed.
///
/// The executor is `--executor` when the operator pinned one, else the
/// standard ladder (env override, then the fleet box, then this machine's
/// Metal tower) — so a worker started with no flags at all still picks the
/// best model actually present.
fn run_worker(
    cfg: &Config,
    token: String,
    control: SocketAddr,
    data: SocketAddr,
) -> Result<(), String> {
    let (executor, model_tag) = if cfg.executor.is_empty() {
        let repo = std::env::var("MAKEPAD_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| repo.join("target/release"));
        let choice = executor::choose_real(&executor::ExecutorEnv::from_env(repo, exe_dir))?;
        let tag = choice.model_tag.clone();
        (choice, tag)
    } else {
        (
            executor::ExecutorChoice {
                argv: cfg.executor.clone(),
                model_tag: cfg.model_tag.clone(),
                source: "flag".to_string(),
            },
            cfg.model_tag.clone(),
        )
    };
    let _ = model_tag;
    let stop = std::sync::atomic::AtomicBool::new(false);
    let wcfg = worker::WorkerConfig {
        endpoints: ApiEndpoints { control, data },
        server_id: None,
        token,
        cache_root: cfg.work.join("worker"),
        work: cfg.work.join("worker-batch"),
        data,
        executor,
        batch: cfg.batch.max(1),
        suffix: "annotate".to_string(),
        version: cfg.version,
        prep: SheetPrep { sheet_size: cfg.sheet_size, exposure: cfg.exposure },
        log: true,
    };
    worker::run(&wcfg, &stop);
    Ok(())
}

/// Catalog ids are stored as 16 raw bytes; the pass moves them as hex.
fn asset_id_from_hex(hex: &str) -> Result<AssetId, String> {
    if hex.len() != 32 {
        return Err(format!("asset id {hex}: expected 32 hex chars"));
    }
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("asset id {hex}: {e}"))?;
    }
    Ok(AssetId::from_bytes(bytes))
}

/// The catalog's stored kind names, mirrored so the carried-through kind
/// survives a round trip.
fn kind_parse(s: &str) -> Option<AssetKind> {
    Some(match s {
        "mesh" => AssetKind::Mesh,
        "character" => AssetKind::Character,
        "weapon" => AssetKind::Weapon,
        "vehicle" => AssetKind::Vehicle,
        "prop" => AssetKind::Prop,
        "texture" => AssetKind::Texture,
        "material" => AssetKind::Material,
        "audio" => AssetKind::Audio,
        "video" => AssetKind::Video,
        "skybox" => AssetKind::Skybox,
        "world" => AssetKind::World,
        "prefab" => AssetKind::Prefab,
        "billboard" => AssetKind::Billboard,
        "game" => AssetKind::Game,
        _ => return None,
    })
}

fn put(
    api: &Api,
    asset_hex: &str,
    up: &makepad_asset_annotate::Upload,
) -> Result<(), String> {
    let id = asset_id_from_hex(asset_hex)?;
    let upload = AnnotationUpload {
        title: up.title.clone(),
        description: up.description.clone(),
        kind: up.kind.as_deref().and_then(kind_parse),
        categories: up.categories.clone(),
        tags: up.tags.clone(),
        creator: up.creator.clone(),
        generator: up.generator.clone(),
        backend: up.backend.clone(),
        model: up.model.clone(),
        prompt: up.prompt.clone(),
        provenance: up.provenance.clone(),
        private: up.private,
    };
    api.put_annotation(&id, &upload).map_err(|e| format!("put {asset_hex}: {e:?}"))
}

/// Fetch sheets, write the batch files, run the executor, collect replies.
///
/// Every step here is [`makepad_asset_annotate::pass`] and
/// [`makepad_asset_annotate::executor`] — the same code the queue worker
/// runs — so an operator run and a queued run cannot describe the same
/// asset differently.
fn run_executor(
    cfg: &Config,
    _api: &Api,
    todo: &[&Candidate],
    token: &str,
    data: SocketAddr,
) -> Result<BTreeMap<String, String>, String> {
    if cfg.executor.is_empty() {
        return Err("--executor is required (see --help)".to_string());
    }
    let sheets = cfg.work.join("sheets");
    std::fs::create_dir_all(&sheets).map_err(|e| format!("mkdir {}: {e}", sheets.display()))?;
    let prep = SheetPrep { sheet_size: cfg.sheet_size, exposure: cfg.exposure };

    let mut jobs = String::new();
    for c in todo {
        let png = pass::thumbnail_sheet(data, token, &c.alias)?;
        let ppm = pass::sheet_to_ppm(&png, cfg.person, &prep)?;
        let path = sheets.join(format!("{}.ppm", c.asset_hex));
        std::fs::write(&path, ppm).map_err(|e| format!("write {}: {e}", path.display()))?;
        jobs.push_str(&pass::job_line(
            &c.asset_hex,
            &path,
            &pass::context_line(&c.alias, cfg.person),
        ));
    }
    let jobs_path = cfg.work.join("jobs.tsv");
    let prompt_path = cfg.work.join("prompt.txt");
    let out_path = cfg.work.join("replies.tsv");
    std::fs::write(&jobs_path, &jobs).map_err(|e| format!("write jobs: {e}"))?;
    std::fs::write(&prompt_path, pass::prompt_for(cfg.person))
        .map_err(|e| format!("write prompt: {e}"))?;

    println!("running executor over {} sheets...", todo.len());
    let never = std::sync::atomic::AtomicBool::new(false);
    let mut on_line = |line: &str, _parsed: executor::ExecutorLine| {
        if !line.trim().is_empty() {
            eprintln!("{}", line.trim());
        }
    };
    let replies = executor::run_batch(
        &cfg.executor,
        &jobs_path,
        &prompt_path,
        &out_path,
        &never,
        &mut on_line,
    )?;
    for (id, why) in &replies.err {
        eprintln!("  executor error for {id}: {why}");
    }
    Ok(replies.ok)
}

/// Clear the pass's own footprint: drop every `vlm-` tag and blank the
/// description it wrote. Used by `--verify-nondestructive` and available on
/// its own when an experimental prompt run should leave nothing behind.
/// Curation: rewrite ONLY the catalog kind of every selected asset —
/// everything else on the record is carried through untouched. Built for
/// the quake3 rig fragments (head/upper/lower + `_N` splits published as
/// kind=character; a 'zombie' ask wore a severed head): with
/// `--alias …/sarge-head --set-kind mesh` they become plain meshes,
/// invisible to character queries, statues for game.model.
fn set_kind(
    api: &Api,
    candidates: &[Candidate],
    kind: &str,
    dry_run: bool,
) -> Result<(), String> {
    if kind_parse(kind).is_none() {
        return Err(format!("--set-kind: unknown kind '{kind}'"));
    }
    let mut n = 0;
    for c in candidates {
        if c.base.kind.as_deref() == Some(kind) {
            continue;
        }
        let up = makepad_asset_annotate::Upload {
            title: c.base.title.clone(),
            description: c.base.description.clone(),
            kind: Some(kind.to_string()),
            categories: c.base.categories.clone(),
            tags: c.base.tags.clone(),
            creator: c.base.creator.clone(),
            generator: c.base.generator.clone(),
            backend: c.base.backend.clone(),
            model: c.base.model.clone(),
            prompt: c.base.prompt.clone(),
            provenance: c.base.provenance.clone(),
            private: c.base.private,
        };
        println!(
            "  {} {} -> {kind}{}",
            c.alias,
            c.base.kind.as_deref().unwrap_or("-"),
            if dry_run { " (dry run)" } else { "" }
        );
        if !dry_run {
            put(api, &c.asset_hex, &up)?;
        }
        n += 1;
    }
    println!("set-kind: {n} rewritten{}", if dry_run { " (dry run)" } else { "" });
    Ok(())
}

fn wipe(api: &Api, candidates: &[Candidate], dry_run: bool) -> Result<(), String> {
    let mut n = 0;
    for c in candidates {
        if !c.base.tags.iter().any(|t| t.starts_with(makepad_asset_annotate::VLM_PREFIX)) {
            continue;
        }
        let mut up = makepad_asset_annotate::Upload {
            title: c.base.title.clone(),
            description: String::new(),
            kind: c.base.kind.clone(),
            categories: c.base.categories.clone(),
            tags: c
                .base
                .tags
                .iter()
                .filter(|t| !t.starts_with(makepad_asset_annotate::VLM_PREFIX))
                .cloned()
                .collect(),
            creator: c.base.creator.clone(),
            generator: c.base.generator.clone(),
            backend: c.base.backend.clone(),
            model: c.base.model.clone(),
            prompt: c.base.prompt.clone(),
            provenance: c.base.provenance.clone(),
            private: c.base.private,
        };
        up.tags.sort();
        if dry_run {
            n += 1;
            continue;
        }
        put(api, &c.asset_hex, &up)?;
        n += 1;
    }
    println!("wiped {n} annotations");
    Ok(())
}

/// Prove the pass cannot damage the import: fingerprint every table that
/// holds imported content, run a wipe, and compare. Any difference in
/// revisions, aliases, blobs or asset identity fails loudly.
fn verify_nondestructive(
    db: &mut Database,
    cfg: &Config,
    api: &Api,
    annotator: &Annotator,
    _token: &str,
    _data: SocketAddr,
) -> Result<(), String> {
    const IMPORT_TABLES: &[&str] = &["assets", "asset_revisions", "asset_aliases", "blobs"];
    let fingerprint = |db: &mut Database| -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for t in IMPORT_TABLES {
            let r = rows(db, &format!("SELECT COUNT(*) FROM {t}"))?;
            out.push(format!("{t}={}", text(&r[0][0])));
        }
        // content identity, not just row counts
        let r = rows(
            db,
            "SELECT COUNT(*), SUM(byte_len) FROM blobs",
        )?;
        out.push(format!("blob_bytes={}", text(&r[0][1])));
        let r = rows(
            db,
            "SELECT group_concat(alias) FROM (SELECT alias FROM asset_aliases ORDER BY alias LIMIT 200)",
        )?;
        out.push(format!("alias_head_len={}", text(&r[0][0]).len()));
        let r = rows(db, "SELECT COUNT(*) FROM asset_revisions")?;
        out.push(format!("revisions={}", text(&r[0][0])));
        Ok(out)
    };

    let before = fingerprint(db)?;
    println!("import fingerprint before: {before:?}");

    let candidates = load_candidates(db, cfg)?;
    let annotated: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| !needs_annotation(&c.base.tags, annotator))
        .collect();
    println!("wiping {} annotated assets", annotated.len());
    let owned: Vec<Candidate> = annotated
        .into_iter()
        .map(|c| Candidate {
            asset_hex: c.asset_hex.clone(),
            alias: c.alias.clone(),
            base: c.base.clone(),
        })
        .collect();
    wipe(api, &owned, false)?;

    let mut db2 = open_catalog(&cfg.store)?;
    let after = fingerprint(&mut db2)?;
    println!("import fingerprint after:  {after:?}");
    if before != after {
        return Err(format!("IMPORT CHANGED: {before:?} != {after:?}"));
    }
    println!("OK: annotation wipe left every imported revision, alias and blob untouched");
    Ok(())
}
