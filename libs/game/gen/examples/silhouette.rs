use makepad_game_gen::*;
fn main(){
    for (name,h) in [("oak",4.0f32),("pine",5.0),("palm",6.0),("cactus",3.0)]{
        let m = tree(name, TreeParams{seed:2,height:h,..Default::default()});
        let (w,hh)=(37usize,17usize);
        let mut grid=vec![b' ';w*hh];
        let sx=m.max.x-m.min.x; let sy=m.max.y-m.min.y;
        for v in m.vertices.chunks_exact(6){
            let x=((v[0]-m.min.x)/sx.max(1e-6)*(w-1) as f32) as usize;
            let y=((v[1]-m.min.y)/sy.max(1e-6)*(hh-1) as f32) as usize;
            grid[(hh-1-y.min(hh-1))*w+x.min(w-1)]=b'#';
        }
        println!("--- {name} h={h} ({} tris, {:.1}x{:.1}) ---",m.triangle_count(),sx,sy);
        for r in 0..hh{ println!("{}", String::from_utf8_lossy(&grid[r*w..(r+1)*w])); }
    }
}
