use makepad_geodata::{fetch_source, find_layer, registry, BuildCtx, FetchOptions};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "geodata — bulk open-geodata fetcher / overlay database builder

USAGE:
  geodata list                 show all layers and their sources
  geodata fetch <layer|all>    download (or revalidate) a layer's bulk sources
  geodata build <layer|all>    build the layer's .mbtiles (fetches if missing)
  geodata status               show cache and output state
  geodata query <layer> <lon> <lat>   query the features sidecar (LLM surface)

OPTIONS:
  --cache-dir <dir>   default: local/overlays/cache
  --out-dir <dir>     default: local/overlays
  --force             re-download even if the cache is fresh
  --radius <m>        query: search radius in meters (default: point query)
  --limit <n>         query: max results (default 10)"
    );
    std::process::exit(2);
}

struct Args {
    command: String,
    target: String,
    positional: Vec<String>,
    cache_dir: PathBuf,
    out_dir: PathBuf,
    force: bool,
    radius: Option<f64>,
    limit: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        command: String::new(),
        target: String::new(),
        positional: Vec::new(),
        cache_dir: PathBuf::from("local/overlays/cache"),
        out_dir: PathBuf::from("local/overlays"),
        force: false,
        radius: None,
        limit: 10,
    };
    let mut positional = Vec::new();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cache-dir" => args.cache_dir = PathBuf::from(iter.next().unwrap_or_default()),
            "--out-dir" => args.out_dir = PathBuf::from(iter.next().unwrap_or_default()),
            "--force" => args.force = true,
            "--radius" => args.radius = iter.next().and_then(|v| v.parse().ok()),
            "--limit" => args.limit = iter.next().and_then(|v| v.parse().ok()).unwrap_or(10),
            "-h" | "--help" => usage(),
            other => positional.push(other.to_string()),
        }
    }
    if positional.is_empty() {
        usage();
    }
    args.command = positional[0].clone();
    args.target = positional.get(1).cloned().unwrap_or_else(|| "all".into());
    args.positional = positional;
    args
}

fn main() {
    let args = parse_args();
    match args.command.as_str() {
        "list" => {
            for layer in registry() {
                let state = if layer.implemented() {
                    "ready"
                } else {
                    "planned"
                };
                println!("{:<14} [{}] {}", layer.id(), state, layer.description());
                for source in layer.sources() {
                    println!(
                        "    source {:<20} {} ({})",
                        source.id, source.url, source.license
                    );
                }
            }
        }
        "fetch" => {
            let opts = FetchOptions {
                cache_dir: args.cache_dir.clone(),
                force: args.force,
            };
            for layer in select(&args.target) {
                for source in layer.sources() {
                    match fetch_source(&opts, &source) {
                        Ok(outcome) => println!(
                            "{:<14} {:<20} {:?}",
                            layer.id(),
                            source.id,
                            outcome
                        ),
                        Err(error) => {
                            eprintln!("{:<14} {:<20} ERROR {error}", layer.id(), source.id);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        "build" => {
            let opts = FetchOptions {
                cache_dir: args.cache_dir.clone(),
                force: false,
            };
            let ctx = BuildCtx {
                cache_dir: args.cache_dir.clone(),
                out_dir: args.out_dir.clone(),
            };
            std::fs::create_dir_all(&ctx.out_dir).expect("create out dir");
            for layer in select(&args.target) {
                if !layer.implemented() {
                    if args.target != "all" {
                        eprintln!("{}: planned, not implemented yet", layer.id());
                    }
                    continue;
                }
                // Make sure sources exist (fresh cache is fine, no re-download).
                // A failed fetch is a warning; the build decides whether the
                // missing file is fatal (e.g. ocean-only DEM cells are not).
                for source in layer.sources() {
                    if let Err(error) = fetch_source(&opts, &source) {
                        eprintln!("{}: fetch {} failed: {error}", layer.id(), source.id);
                    }
                }
                let start = std::time::Instant::now();
                match layer.build(&ctx) {
                    Ok(report) => println!(
                        "{:<14} {} features -> {} tiles, {:.1} MB, {:.1}s -> {}",
                        layer.id(),
                        report.features,
                        report.tiles,
                        report.bytes as f64 / 1e6,
                        start.elapsed().as_secs_f64(),
                        report.out_path.display()
                    ),
                    Err(error) => {
                        eprintln!("{}: build failed: {error}", layer.id());
                        std::process::exit(1);
                    }
                }
            }
        }
        "radar-sync" => {
            let dataset = if args.target == "reflectivity" {
                makepad_geodata::radar::RadarDataset::ReflectivityComposite
            } else {
                makepad_geodata::radar::RadarDataset::Forecast
            };
            let config = makepad_geodata::radar::RadarConfig::for_dataset(
                args.out_dir.join("radar"),
                dataset,
            );
            match makepad_geodata::radar::RadarSync::new(config).sync() {
                Ok(state) => {
                    println!(
                        "polled: {}, downloaded: {}, frames on disk: {}",
                        state.polled,
                        state.downloaded,
                        state.frames.len()
                    );
                    for frame in &state.frames {
                        println!(
                            "  {} ({:.1} MB, created {})",
                            frame.filename,
                            frame.bytes as f64 / 1e6,
                            frame.created_unix
                        );
                    }
                }
                Err(error) => {
                    eprintln!("radar sync failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "query" => {
            let (Some(lon), Some(lat)) = (
                args.positional.get(2).and_then(|v| v.parse::<f64>().ok()),
                args.positional.get(3).and_then(|v| v.parse::<f64>().ok()),
            ) else {
                usage();
            };
            let path = if args.target.ends_with(".mbtiles") {
                PathBuf::from(&args.target)
            } else {
                args.out_dir.join(format!("nl-{}.mbtiles", args.target))
            };
            let mut db = match makepad_geodata::query::LayerDb::open(&path) {
                Ok(db) => db,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            };
            let result = match args.radius {
                Some(radius) => db.query_radius(lon, lat, radius, args.limit),
                None => db.query_point(lon, lat, args.limit),
            };
            match result {
                Ok(hits) => {
                    for hit in hits {
                        let mut attrs = hit.attrs.clone();
                        if let Some(map) = attrs.as_object_mut() {
                            map.remove("__ring");
                        }
                        let line = serde_json::json!({
                            "layer": hit.layer,
                            "name": hit.name,
                            "distance_m": hit.distance_m.map(|d| d.round()),
                            "center": [hit.center.0, hit.center.1],
                            "attrs": attrs,
                        });
                        println!("{line}");
                    }
                }
                Err(error) => {
                    eprintln!("query failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "status" => {
            println!("cache: {}", args.cache_dir.display());
            for layer in registry() {
                for source in layer.sources() {
                    let path = args.cache_dir.join(source.filename);
                    let state = match path.metadata() {
                        Ok(meta) => format!("{:.1} MB", meta.len() as f64 / 1e6),
                        Err(_) => "missing".into(),
                    };
                    println!("  {:<24} {}", source.filename, state);
                }
            }
            println!("outputs: {}", args.out_dir.display());
            for layer in registry() {
                let path = args.out_dir.join(format!("nl-{}.mbtiles", layer.id()));
                if let Ok(meta) = path.metadata() {
                    println!(
                        "  nl-{}.mbtiles {:.1} MB",
                        layer.id(),
                        meta.len() as f64 / 1e6
                    );
                }
            }
        }
        _ => usage(),
    }
}

fn select(target: &str) -> Vec<Box<dyn makepad_geodata::Layer>> {
    if target == "all" {
        registry()
    } else {
        match find_layer(target) {
            Some(layer) => vec![layer],
            None => {
                eprintln!("unknown layer '{target}' (see: geodata list)");
                std::process::exit(2);
            }
        }
    }
}
