// Parse real Kenney GLBs and report what came out.
use makepad_render::model::StaticModel;
fn main() {
    let root = std::path::Path::new("apps/sandbox/resources/models/kenney");
    let mut total = 0usize;
    let mut ok = 0usize;
    let mut tris = 0usize;
    let mut fails: Vec<String> = Vec::new();
    for pack in std::fs::read_dir(root).unwrap().flatten() {
        let p = pack.path();
        if !p.is_dir() { continue; }
        for f in std::fs::read_dir(&p).unwrap().flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("glb") { continue; }
            total += 1;
            match StaticModel::parse_glb(&std::fs::read(&fp).unwrap()) {
                Ok(m) => {
                    ok += 1;
                    tris += m.triangle_count();
                    if ok <= 5 {
                        println!("{:40} v={:5} t={:5} h={:.2} tex={:?}",
                            fp.file_name().unwrap().to_string_lossy(),
                            m.vertex_count(), m.triangle_count(), m.height(), m.texture_uri);
                    }
                }
                Err(e) => if fails.len() < 5 { fails.push(format!("{}: {e}", fp.display())) },
            }
        }
    }
    println!("\nparsed {ok}/{total}, {tris} triangles total, avg {} tris/model", tris / ok.max(1));
    for f in &fails { println!("FAIL {f}"); }
}
