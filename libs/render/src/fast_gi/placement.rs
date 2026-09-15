//! Bounded static probe placement on the existing worker pool. This is scene
//! preparation, not CPU lighting: recurring transport stays on the GPU.
use super::*;

pub(super) struct Placement {
    pub positions: Vec<f32>,
    pub moved: usize,
    pub inactive: usize,
    pub exhausted: usize,
    pub ms: f64,
    pub static_visibility: StaticVisibility,
}

#[derive(Default)]
pub(super) struct StaticVisibility {
    pub boxes: Vec<Mover>,
    pub cells: Vec<[f32;4]>,
    pub fallbacks: usize,
}
impl StaticVisibility {
    fn build(boxes:&[Mover],origin:Vec3f,c:GiConfig)->Self {
        let pad=vec3f(0.6,0.6,0.6)*c.spacing;
        let lo=origin-pad;
        let hi=origin+vec3f(c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32)*c.spacing+pad;
        let center=origin+vec3f((c.grid[0]-1)as f32,(c.grid[1]-1)as f32,(c.grid[2]-1)as f32)*(c.spacing*0.5);
        let mut candidates:Vec<_>=boxes.iter().filter_map(|m|{
            let (a,b)=mover_bounds(m);
            if b.x<lo.x||a.x>hi.x||b.y<lo.y||a.y>hi.y||b.z<lo.z||a.z>hi.z{return None;}
            let nearest=vec3f(center.x.clamp(a.x,b.x),center.y.clamp(a.y,b.y),center.z.clamp(a.z,b.z));
            Some(((nearest-center).length(),*m,(a,b)))
        }).collect();
        // Stable ordering and a fixed local budget; all other static geometry
        // remains in the triangle BVH/moments, not discarded as an occluder.
        candidates.sort_by(|a,b|a.0.total_cmp(&b.0));
        let fallbacks=candidates.len().saturating_sub(MAX_STATIC_BLOCKERS);
        candidates.truncate(MAX_STATIC_BLOCKERS);
        let bounds:Vec<_>=candidates.iter().map(|v|v.2).collect();
        let mut cells=Vec::with_capacity(c.probe_count());
        for z in 0..c.grid[2] {for y in 0..c.grid[1] {for x in 0..c.grid[0] {
            cells.push(cell_blockers(origin+vec3f(x as f32,y as f32,z as f32)*c.spacing,c.spacing,&bounds));
        }}}
        Self{boxes:candidates.into_iter().map(|v|v.1).collect(),cells,fallbacks}
    }
}

fn survey(bvh:&Bvh,p:Vec3f,c:GiConfig)->(usize,bool,Option<(f32,Vec3f)>,Option<(f32,Vec3f)>) {
    let mut backfaces=0;let mut exhausted=false;
    let mut exit:Option<(f32,Vec3f)>=None;
    let mut front:Option<(f32,Vec3f)>=None;
    // Include cardinal axes: an octahedral ray lattice alone misses narrow
    // gaps parallel to its bins, exactly where placement matters most.
    for z in -1..=1 {for y in -1..=1 {for x in -1..=1 {
        if x==0&&y==0&&z==0 {continue;}
        let rd=vec3f(x as f32,y as f32,z as f32).normalize();
        let hit=bvh.trace(p,rd,c.ray_distance,false);
        if hit.truncated {exhausted=true;continue;}
        if !hit.is_hit(){continue;}
        let t=&bvh.tris[hit.tri as usize];
        let n=Vec3f::cross(t.v1-t.v0,t.v2-t.v0).normalize();
        if n.dot(rd)>0.0 {
            backfaces+=1;
            if exit.map_or(true,|v|hit.t<v.0){exit=Some((hit.t,rd));}
        } else {
            let distance=hit.t*(-n.dot(rd));
            if front.map_or(true,|v|distance<v.0){front=Some((distance,n));}
        }
    }}}
    (backfaces,exhausted,exit,front)
}

fn relocate(bvh:&Bvh,home:Vec3f,c:GiConfig)->(Vec3f,f32) {
    let clearance=c.spacing*0.2;
    let max_offset=c.spacing*0.45;
    let mut p=home;
    for _ in 0..4 {
        let (backfaces,exhausted,exit,front)=survey(bvh,p,c);
        if exhausted{return (p,-2.0);}
        let movement=if backfaces>2 {
            match exit {Some((distance,dir))=>dir*(distance+clearance),None=>return (p,-1.0)}
        }else if let Some((distance,n))=front {
            if distance>=clearance-0.001{return (p,1.0);}
            n*(clearance-distance+0.001)
        }else{return (p,1.0);};
        let offset=p+movement-home;
        if offset.length()>max_offset{return (p,if backfaces>2{-1.0}else{1.0});}
        p=p+movement;
    }
    let (backfaces,exhausted,_,_)=survey(bvh,p,c);
    (p,if exhausted{-2.0}else if backfaces>2{-1.0}else{1.0})
}

impl Placement {
    pub fn build(bvh:&Bvh,origin:Vec3f,c:GiConfig,boxes:&[Mover])->Self {
        let start=Cx::monotonic_now();
        let mut result=Self{positions:Vec::with_capacity(c.probe_count()*4),moved:0,inactive:0,exhausted:0,ms:0.0,static_visibility:StaticVisibility::build(boxes,origin,c)};
        for z in 0..c.grid[2] {for y in 0..c.grid[1] {for x in 0..c.grid[0] {
            let home=origin+vec3f(x as f32,y as f32,z as f32)*c.spacing;
            let (p,state)=relocate(bvh,home,c);
            result.moved+=usize::from((p-home).length()>0.001);
            result.inactive+=usize::from(state<0.0);
            result.exhausted+=usize::from(state<(-1.5));
            result.positions.extend([p.x,p.y,p.z,state]);
        }}}
        result.ms=(Cx::monotonic_now()-start)*1e3;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_box_lists_are_local_bounded_and_keep_the_triangle_scene() {
        let mut instances=vec![super::super::tests::cube();MAX_STATIC_BLOCKERS+5];
        let mut distant=super::super::tests::cube();distant.transform.v[12]=1000.0;instances.push(distant);
        let prepared=Prepared::build(instances).unwrap();
        let c=GiConfig{grid:[2,2,2],..GiConfig::default()};
        let field=StaticVisibility::build(&prepared.static_boxes,vec3f(-0.75,-0.75,-0.75),c);
        assert_eq!(field.boxes.len(),MAX_STATIC_BLOCKERS);
        assert_eq!(field.fallbacks,5,"out-of-volume boxes consume no exact budget");
        assert_eq!(prepared.count,(MAX_STATIC_BLOCKERS+6)*12,"fallback boxes stay in the BVH");
        assert_eq!(field.cells.len(),c.probe_count());
        assert_eq!(field.cells[0][0],-2.0,"crowded cells scan the bounded selected bank");
        let empty=StaticVisibility::build(&prepared.static_boxes,vec3f(100.0,100.0,100.0),c);
        assert!(empty.boxes.is_empty());assert_eq!(empty.fallbacks,0);
        assert!(empty.cells.iter().all(|v|*v==[-1.0;4]));
    }
    #[test]
    fn arbitrary_mesh_bounds_are_not_promoted_to_solid_boxes() {
        let mut mesh=super::super::tests::cube();mesh.exact_box=false;
        let prepared=Prepared::build(vec![mesh]).unwrap();
        assert!(prepared.static_boxes.is_empty());assert_eq!(prepared.count,12);
    }
    fn box_bvh()->Bvh {let p=Prepared::build(vec![super::super::tests::cube()]).unwrap();Arc::try_unwrap(p.bvh).ok().unwrap()}
    #[test]
    fn pushes_nearby_probe_outward_without_crossing_its_cell() {
        let bvh=box_bvh();let c=GiConfig::default();
        let home=vec3f(0.53,0.0,0.0);
        let (p,state)=relocate(&bvh,home,c);
        assert_eq!(state,1.0);assert!(p.x>=0.799);assert!((p-home).length()<=c.spacing*0.45);
    }
    #[test]
    fn solid_center_cannot_escape_beyond_bounded_offset() {
        let (p,state)=relocate(&box_bvh(),Vec3f::default(),GiConfig::default());
        assert_eq!(state,-1.0);assert_eq!(p,Vec3f::default());
    }
    #[test]
    fn near_inside_probe_can_escape_and_empty_scene_preserves_lattice() {
        let c=GiConfig::default();let home=vec3f(0.45,0.0,0.0);
        let (p,state)=relocate(&box_bvh(),home,c);
        assert_eq!(state,1.0);assert!(p.x>=0.799);assert!((p-home).length()<=c.spacing*0.45);
        let p=Placement::build(&Bvh::build(&[]),vec3f(-2.0,-3.0,-4.0),GiConfig{grid:[2,2,2],..c},&[]);
        assert_eq!(p.moved,0);assert_eq!(p.inactive,0);assert_eq!(p.positions.len(),32);
    }
    #[test]
    fn fixture_ceiling_probe_gains_clearance_on_the_room_side() {
        let mut roof=super::super::tests::cube();
        roof.transform.v[0]=12.0;roof.transform.v[5]=0.4;roof.transform.v[10]=6.0;
        roof.transform.v[13]=6.0;roof.transform.v[14]=-2.0;
        let prepared=Prepared::build(vec![roof]).unwrap();
        let home=vec3f(0.0,5.6055,-2.0);
        let (p,state)=relocate(&prepared.bvh,home,GiConfig::default());
        assert_eq!(state,1.0);assert!(p.y<5.501&&p.y>5.49);
        assert!((p-home).length()<0.675);
    }
}
