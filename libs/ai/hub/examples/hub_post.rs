//! Post one generate request to a hub node and wait for it — the smallest
//! client for checking a model or a prompt writer against a real node
//! (fleet TLS + this machine's credential when the URL is https).
//!
//!   hub_post https://10.0.0.149:8123 request.json [--save DIR]
//!
//! Prints progress lines, the final job record, and every text artifact.
use makepad_ai_hub::client::mark_if_fleet;
use makepad_ai_hub::http_client::{http_fetch, HttpClientRequest};
use std::time::{Duration, Instant};

fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = http_fetch(&HttpClientRequest::get(url)).map_err(|e| e.to_string())?;
    let status = resp.status;
    let body = resp.read_body_to_vec(256 << 20).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("GET {url}: {status} {}", String::from_utf8_lossy(&body)));
    }
    Ok(body)
}

fn get(url: &str) -> Result<String, String> {
    get_bytes(url).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        stripped.find('"').map(|end| &stripped[..end])
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hub_post <base url> <request.json> [--save DIR]");
        std::process::exit(2);
    }
    let base = args[1].trim_end_matches('/');
    let save = args.iter().position(|a| a == "--save").and_then(|i| args.get(i + 1));
    mark_if_fleet(base);
    let body = std::fs::read(&args[2]).expect("request file");
    let url = format!("{base}/generate");
    let resp = http_fetch(&HttpClientRequest::post(&url, "application/json", &body))
        .unwrap_or_else(|e| panic!("POST {url}: {e}"));
    let status = resp.status;
    let text = String::from_utf8_lossy(&resp.read_body_to_vec(1 << 20).unwrap()).into_owned();
    if status != 200 {
        eprintln!("POST {url}: {status} {text}");
        std::process::exit(1);
    }
    let job = field(&text, "job_id").expect("job_id").to_string();
    let started = Instant::now();
    let mut last = String::new();
    let record = loop {
        let record = get(&format!("{base}/job/{job}")).unwrap();
        let state = field(&record, "state").unwrap_or("").to_string();
        let line = format!("{state} {}", field(&record, "stage").unwrap_or(""));
        if line != last {
            eprintln!("{:6.1}s {line}", started.elapsed().as_secs_f64());
            last = line;
        }
        if matches!(state.as_str(), "done" | "failed" | "cancelled") {
            break record;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    println!("{record}");
    let mut rest = record.as_str();
    let mut index = 0;
    while let Some(at) = rest.find("\"url\":\"") {
        let tail = &rest[at + 7..];
        let end = tail.find('"').unwrap();
        let path = &tail[..end];
        let ctype = field(tail, "content_type").unwrap_or("");
        let data = get_bytes(&format!("{base}{path}"));
        if let Ok(data) = &data {
            if ctype.starts_with("text/") || ctype.contains("json") {
                println!("--- {path} ({ctype})\n{}", String::from_utf8_lossy(data));
            }
        }
        if let (Some(dir), Ok(data)) = (save, data) {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(format!("{dir}/{job}-{index}.out"), data).unwrap();
        }
        index += 1;
        rest = &tail[end..];
    }
}
