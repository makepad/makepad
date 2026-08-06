use makepad_game_assets::{agent, AssetIndex, AssetKind};
use std::time::Instant;
fn main() {
    let t = Instant::now();
    let idx = AssetIndex::build(std::path::Path::new("apps/arcade/resources"));
    let build_ms = t.elapsed().as_millis();
    let kw: usize = idx.entries().iter().map(|e| e.keywords.len()).sum();
    let bytes: usize = idx.entries().iter().map(|e|
        e.id.len()+e.name.len()+e.path.as_os_str().len()+e.pack.len()
        + e.keywords.iter().map(|k| k.len()+24).sum::<usize>()
        + e.categories.iter().map(|c| c.len()+24).sum::<usize>() + 160).sum();
    println!("build: {build_ms} ms | entries {} ({} models, {} sounds, {} music)",
        idx.len(), idx.count_of(AssetKind::Model), idx.count_of(AssetKind::Sound), idx.count_of(AssetKind::Music));
    println!("keywords total {kw} (avg {:.1}/entry) | approx heap {:.1} MB", kw as f32/idx.len() as f32, bytes as f32/1048576.0);
    let s = agent::library_summary(&idx);
    println!("summary {} chars:\n{s}\n", s.len());
    let t2 = Instant::now();
    for q in ["truck","tree","something to drive"] { let _ = idx.find(q); }
    println!("3 queries: {} us", t2.elapsed().as_micros());
    for q in std::env::args().skip(1) {
        let (q, kind) = match q.split_once('#') { Some((a,b))=>(a.to_string(),Some(b.to_string())), None=>(q,None) };
        let mut p = agent::FindParams::new(&q);
        if let Some(k)=&kind { p = p.with_kind_str(k); }
        let r = agent::execute(&idx, &p);
        let top: Vec<String> = r.iter().take(3).map(|x| x.id.clone()).collect();
        println!("{:34} -> {}", q, if top.is_empty(){"(none)".into()}else{top.join(" | ")});
    }
}
