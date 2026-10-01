//! Cascaded probe-GI kernels: fragment passes only (no compute, no readback).
//!
//! Frame: TRACE (per batch probe: place it off geometry, 64 rotated rays
//! through the voxel clipmap) -> RELIGHT (shade each hit: sun through the
//! voxels, clustered lights, sky on miss, last frame's field for further
//! bounces) -> GATHER (L1 irradiance + 8x8 octahedral distance moments,
//! blended with the probe's history) -> SCATTER (copy the updated probe
//! rows into the persistent field; untouched probes are never rewritten).
//!
//! Field: an RGBA16F atlas, per cascade and probe slot an 8x8 irradiance
//! tile (6x6 octahedral interior + guard ring, cosine-weighted radiance),
//! a 10x10 moment tile (8x8 interior + guard, E[d] and E[d^2]) and one info
//! texel (relocation offset in cells, state). Receivers read 3 texels per
//! probe with hardware bilinear filtering. state > 0 is valid (ramping in
//! over the first updates), < 0 inside geometry, 0 not traced.
use makepad_draw::*;
script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shared.*
    // Cascade windows; shared by producers and receivers.
    let GiCascades = {
        // x,y,z probes per cascade, w = probes per cascade
        gi_grid: uniform(vec4(16.0,8.0,16.0,2048.0))
        // Per cascade: min cell of the window (xyz), spacing (w; 0 = off)
        gi_c0: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_c1: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_c2: uniform(vec4(0.0,0.0,0.0,0.0))
        // Probe home inside its cell (xyz, in cells), w = cascade count
        gi_home: uniform(vec4(0.43,0.47,0.41,0.0))
        gi_cascade: fn(k:float)->vec4 {
            if k<0.5 {return self.gi_c0}
            if k<1.5 {return self.gi_c1}
            return self.gi_c2
        }
        gi_mod: fn(a:vec3,b:vec3)->vec3 {return a-floor(a/b)*b}
        gi_slot: fn(cell:vec3)->float {
            let m=self.gi_mod(cell,self.gi_grid.xyz)
            return m.x+self.gi_grid.x*(m.y+self.gi_grid.y*m.z)
        }
        gi_hash: fn(p:vec3)->vec3 {
            return fract(sin(vec3(dot(p,vec3(127.1,311.7,74.7)),dot(p,vec3(269.5,183.3,246.1)),dot(p,vec3(113.5,271.9,124.6))))*43758.5453)
        }
    }
    let FastGiSampling = {
        ..GiCascades,
        gi_field: texture_2d(float)
        gi_on: uniform(0.0)
        // Where no cascade is confident a RECEIVER keeps this share of its
        // ordinary ambient (1: never darker than GI off). Producers leave 0:
        // an uncertain probe must not inject sky into the bounce feedback.
        gi_fallback_floor: uniform(0.0)
        gi_debug: uniform(0.0)
        // Atlas: width, height, tiles per row, tile rows per cascade.
        gi_atlas: uniform(vec4(640.0,1824.0,64.0,32.0))
        // Texel origin of probe `slot`'s tile in region `region` (0 irradiance
        // 8x8, 1 moments 10x10, 2 info 1x1) of cascade k.
        gi_tile: fn(k:float,slot:float,region:float)->vec2 {
            let r=self.gi_atlas.z
            let t=self.gi_atlas.w
            let tile=vec2(slot-floor(slot/r)*r,floor(slot/r))
            let base=k*t*19.0
            if region<0.5 {return vec2(tile.x*8.0,base+tile.y*8.0)}
            if region<1.5 {return vec2(tile.x*10.0,base+t*8.0+tile.y*10.0)}
            return vec2(tile.x,base+t*18.0+tile.y)
        }
        gi_oct: fn(d:vec3)->vec2 {
            var o=d.xy/max(abs(d.x)+abs(d.y)+abs(d.z),0.00001)
            if d.z<0.0 {o=(vec2(1.0,1.0)-abs(o.yx))*(step(vec2(0.0,0.0),o)*2.0-vec2(1.0,1.0))}
            return o*0.5+vec2(0.5,0.5)
        }
        // Bilinear lookup inside a tile's interior; the guard ring holds the
        // octahedral neighbours, so filtering never bleeds across tiles.
        gi_tile_sample: fn(k:float,slot:float,region:float,d:vec3)->vec4 {
            var n=6.0
            if region>0.5 {n=8.0}
            let p=self.gi_tile(k,slot,region)+vec2(1.0,1.0)+self.gi_oct(d)*n
            return self.gi_field.sample(p/self.gi_atlas.xy)
        }
        // (relocation offset in cells, state): state > 0 valid (ramps in
        // over the first updates), < 0 inside geometry, 0 not traced.
        gi_info: fn(k:float,cell:vec3)->vec4 {
            let p=self.gi_tile(k,self.gi_slot(cell),2.0)+vec2(0.5,0.5)
            return self.gi_field.sample_nearest(p/self.gi_atlas.xy)
        }
        // One probe's contribution: vec4(irradiance*weight, weight).
        gi_probe: fn(k:float,cell:vec3,info:vec4,wp:vec3,n:vec3,w:float)->vec4 {
            if w<=0.0 {return vec4(0.0,0.0,0.0,0.0)}
            let slot=self.gi_slot(cell)
            let s=self.gi_cascade(k).w
            let probe=(cell+self.gi_home.xyz+info.xyz)*s
            // Do not push the receiver past a nearby probe: a fixed normal
            // bias made a visible probe look behind the wall (dark dots).
            let to_probe=probe-wp
            let probe_distance=length(to_probe)
            let bias=min(s*0.03,probe_distance*0.25)
            let delta=wp+n*bias-probe
            let distance=length(delta)
            let moments=self.gi_tile_sample(k,slot,1.0,delta/max(distance,0.00001)).xy
            let mean=moments.x
            let difference=max(distance-mean-s*0.05,0.0)
            let variance=max(moments.y-mean*mean,0.0001)
            var visibility=variance/(variance+difference*difference)
            if mean<=0.0 {visibility=1.0}
            // Facing uses the UNBIASED receiver, bounded to a sub-cell
            // footprint so a probe almost on a wall makes no weight spike.
            let facing=pow(clamp(dot(n,to_probe)/max(probe_distance,s*0.25)*0.5+0.5,0.0,1.0),2.0)
            let weight=w*visibility*visibility*max(facing,0.02)
            var light=max(self.gi_tile_sample(k,slot,0.0,n).xyz,vec3(0.0,0.0,0.0))
            if self.gi_debug>2.5 && self.gi_debug<3.5 {light=self.gi_hash(vec3(slot,k,1.0))}
            return vec4(light*weight,weight)
        }
        // One cascade: vec4(irradiance, confidence incl. the window fade).
        gi_cascade_sample: fn(k:float,wp:vec3,n:vec3)->vec4 {
            let c=self.gi_cascade(k)
            if c.w<=0.0 {return vec4(0.0,0.0,0.0,0.0)}
            // Interpolate on the AIR side of the receiver.
            let local=(wp+n*(c.w*0.1))/c.w-self.gi_home.xyz-c.xyz
            let g=self.gi_grid.xyz-vec3(1.0,1.0,1.0)-local
            let edge=min(min(min(local.x,local.y),local.z),min(min(g.x,g.y),g.z))
            if edge<=0.0 {return vec4(0.0,0.0,0.0,0.0)}
            let base=floor(local)
            let f=local-base
            let t=vec3(1.0,1.0,1.0)-f
            let b0=c.xyz+base
            let p0=self.gi_info(k,b0)
            let p1=self.gi_info(k,b0+vec3(1.0,0.0,0.0))
            let p2=self.gi_info(k,b0+vec3(0.0,1.0,0.0))
            let p3=self.gi_info(k,b0+vec3(1.0,1.0,0.0))
            let p4=self.gi_info(k,b0+vec3(0.0,0.0,1.0))
            let p5=self.gi_info(k,b0+vec3(1.0,0.0,1.0))
            let p6=self.gi_info(k,b0+vec3(0.0,1.0,1.0))
            let p7=self.gi_info(k,b0+vec3(1.0,1.0,1.0))
            // Trilinear weight x maturity (a new probe fades in).
            let w0=t.x*t.y*t.z*max(p0.w,0.0)
            let w1=f.x*t.y*t.z*max(p1.w,0.0)
            let w2=t.x*f.y*t.z*max(p2.w,0.0)
            let w3=f.x*f.y*t.z*max(p3.w,0.0)
            let w4=t.x*t.y*f.z*max(p4.w,0.0)
            let w5=f.x*t.y*f.z*max(p5.w,0.0)
            let w6=t.x*f.y*f.z*max(p6.w,0.0)
            let w7=f.x*f.y*f.z*max(p7.w,0.0)
            let available=w0+w1+w2+w3+w4+w5+w6+w7
            if available<=0.000001 {return vec4(0.0,0.0,0.0,0.0)}
            let sum=self.gi_probe(k,b0,p0,wp,n,w0)+self.gi_probe(k,b0+vec3(1.0,0.0,0.0),p1,wp,n,w1)
                +self.gi_probe(k,b0+vec3(0.0,1.0,0.0),p2,wp,n,w2)+self.gi_probe(k,b0+vec3(1.0,1.0,0.0),p3,wp,n,w3)
                +self.gi_probe(k,b0+vec3(0.0,0.0,1.0),p4,wp,n,w4)+self.gi_probe(k,b0+vec3(1.0,0.0,1.0),p5,wp,n,w5)
                +self.gi_probe(k,b0+vec3(0.0,1.0,1.0),p6,wp,n,w6)+self.gi_probe(k,b0+vec3(1.0,1.0,1.0),p7,wp,n,w7)
            if sum.w<=0.00000001 {return vec4(0.0,0.0,0.0,0.0)}
            // Coverage (enough traced, valid probes) times visibility (the
            // receiver is not hidden from all of them); both fade to the
            // next cascade, never to black.
            let confidence=smoothstep(0.05,0.4,available)*smoothstep(0.0,0.05,sum.w/available)
            return vec4(sum.xyz/sum.w,confidence*clamp(edge,0.0,1.0))
        }
        gi_ambient_weights: fn(wp:vec3,n:vec3)->vec3 {
            let s0=self.gi_cascade_sample(0.0,wp,n).w
            let s1=self.gi_cascade_sample(1.0,wp,n).w
            let s2=self.gi_cascade_sample(2.0,wp,n).w
            return vec3(s0,s1*(1.0-s0),s2*(1.0-s0)*(1.0-s1))
        }
        // Finest confident cascade first, then coarser, then the ordinary
        // ambient: an untraced, occluded or out-of-volume receiver keeps
        // the look it has with GI off.
        gi_ambient: fn(wp:vec3,normal:vec3,fallback:vec3)->vec3 {
            if self.gi_on<=0.0 {return fallback}
            let n=normalize(normal)
            let s0=self.gi_cascade_sample(0.0,wp,n)
            var acc=s0.xyz*s0.w
            var remain=1.0-s0.w
            if remain>0.002 && self.gi_home.w>1.5 {
                let s1=self.gi_cascade_sample(1.0,wp,n)
                acc=acc+s1.xyz*(s1.w*remain)
                remain=remain*(1.0-s1.w)
                if remain>0.002 && self.gi_home.w>2.5 {
                    let s2=self.gi_cascade_sample(2.0,wp,n)
                    acc=acc+s2.xyz*(s2.w*remain)
                    remain=remain*(1.0-s2.w)
                }
            }
            let gi=acc+fallback*(self.gi_fallback_floor*remain)
            return mix(fallback,gi,min(self.gi_on,1.0))
        }
        // Last operation before writing DISPLAY colour into an 8-bit scene
        // target. One quantization step of zero-mean world-space noise
        // breaks broad dark gradient contours.
        gi_display: fn(color:vec4,wp:vec3,n:vec3)->vec4 {
            if self.gi_on<=0.0 || color.w<0.999 {return color}
            if self.gi_debug>0.5 {
                let nn=normalize(n)
                if self.gi_debug<1.5 {
                    // Cells of the finest cascade that covers the pixel,
                    // tinted per cascade (red fine, green mid, blue coarse).
                    let w=self.gi_ambient_weights(wp,nn)
                    var k=0.0
                    if w.x<0.001 {k=1.0}
                    if w.x<0.001 && w.y<0.001 {k=2.0}
                    let c=self.gi_cascade(k)
                    let cell=floor(wp/max(c.w,0.001)-self.gi_home.xyz)
                    let tint=vec3(step(k,0.5),step(abs(k-1.0),0.5),step(1.5,k))
                    return vec4(mix(fract(cell*vec3(0.37,0.57,0.73)),tint,0.5),1.0)
                }
                if self.gi_debug<2.5 {return vec4(self.gi_ambient_weights(wp,nn),1.0)}
                if self.gi_debug>4.5 {
                    // Nearest probe of cascade 0: orange = inside geometry,
                    // grey = not traced, green = valid (dim while fading in).
                    let c=self.gi_c0
                    let cell=floor((wp+nn*(c.w*0.1))/max(c.w,0.001)-self.gi_home.xyz+vec3(0.5,0.5,0.5))
                    let info=self.gi_info(0.0,cell)
                    if info.w<0.0 {return vec4(1.0,0.4,0.0,1.0)}
                    if info.w==0.0 {return vec4(0.1,0.1,0.1,1.0)}
                    if self.gi_debug>5.5 {return vec4(max(self.gi_tile_sample(0.0,self.gi_slot(cell),0.0,nn).xyz,vec3(0.0,0.0,0.0)),1.0)}
                    return vec4(0.0,0.8*info.w,0.1,1.0)
                }
                return vec4(self.gi_ambient(wp,nn,vec3(0.0,0.0,0.0)),1.0)
            }
            let noise=fract(sin(dot(wp,vec3(127.1,311.7,74.7)))*43758.5453)-0.5
            return vec4(max(color.xyz+vec3(noise,noise,noise)*(1.0/255.0),vec3(0.0,0.0,0.0)),color.w)
        }
    }
    // This frame's probe list: row r = (slot, cascade, hysteresis, -),
    // (cell xyz, -). gi_pass = (rows, row capacity, ray length, frame seed).
    let GiBatch = {
        ..GiCascades,
        gi_batch: texture_2d(float)
        gi_pass: uniform(vec4(1.0,1.0,32.0,0.0))
        batch_row: fn()->float {return min(floor(self.pos.y*self.gi_pass.x),self.gi_pass.x-1.0)}
        batch_a: fn(row:float)->vec4 {return self.gi_batch.sample_nearest(vec2(0.25,(row+0.5)/self.gi_pass.y))}
        batch_b: fn(row:float)->vec4 {return self.gi_batch.sample_nearest(vec2(0.75,(row+0.5)/self.gi_pass.y))}
        // Spherical Fibonacci set under a random rotation per probe and
        // update: fixed directions printed probe-aligned stripes.
        gi_ray: fn(r:float,a:vec4)->vec3 {
            let z=1.0-(2.0*r+1.0)/64.0
            let q=sqrt(max(1.0-z*z,0.0))
            let phi=r*2.3999632
            let v=vec3(q*cos(phi),z,q*sin(phi))
            let u=self.gi_hash(vec3(a.x+0.5,a.y*7.0+1.0,self.gi_pass.w))
            let s1=sqrt(1.0-u.x)
            let s2=sqrt(u.x)
            let qv=vec4(s1*sin(6.2831853*u.y),s1*cos(6.2831853*u.y),s2*sin(6.2831853*u.z),s2*cos(6.2831853*u.z))
            return normalize(v+cross(qv.xyz,cross(qv.xyz,v)+v*qv.w)*2.0)
        }
        gi_unpack: fn(p:float)->vec3 {
            let x=abs(p)
            return vec3(x-floor(x/256.0)*256.0,floor(x/256.0)-floor(x/65536.0)*256.0,floor(x/65536.0))/255.0
        }
    }
    // The voxel clipmap: level l = (min voxel xyz, voxel size; 0 = absent).
    let GiVoxels = {
        gi_vox_a: texture_2d(float)
        gi_vox_b: texture_2d(float)
        gi_vox_c: texture_2d(float)
        gi_vox_dims: uniform(vec4(64.0,32.0,64.0,0.0))
        // texture width, height, rows per level, z slices per row
        gi_vox_tex: uniform(vec4(512.0,768.0,256.0,8.0))
        gi_vox0: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_vox1: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_vox2: uniform(vec4(0.0,0.0,0.0,0.0))
        vox_level: fn(l:float)->vec4 {
            if l<0.5 {return self.gi_vox0}
            if l<1.5 {return self.gi_vox1}
            return self.gi_vox2
        }
        vox_uv: fn(l:float,c:vec3)->vec2 {
            let d=self.gi_vox_dims.xyz
            let m=c-floor(c/d)*d
            let tile=floor(m.z/self.gi_vox_tex.w)
            let x=m.x+d.x*(m.z-tile*self.gi_vox_tex.w)
            let y=m.y+d.y*tile+l*self.gi_vox_tex.z
            return vec2((x+0.5)/self.gi_vox_tex.x,(y+0.5)/self.gi_vox_tex.y)
        }
        vox_inside: fn(l:float,c:vec3)->bool {
            let o=self.vox_level(l).xyz
            let d=self.gi_vox_dims.xyz
            return c.x>=o.x && c.y>=o.y && c.z>=o.z && c.x<o.x+d.x && c.y<o.y+d.y && c.z<o.z+d.z
        }
        vox_solid: fn(l:float,c:vec3)->bool {return self.gi_vox_a.sample_nearest(self.vox_uv(l,c)).w>0.5}
        // Finest level holding p: vec4(level, cell), level = -1 if none.
        vox_find: fn(p:vec3,first:float)->vec4 {
            var l=first
            loop {
                if l>=self.gi_vox_dims.w {break}
                let v=self.vox_level(l)
                if v.w>0.0 {
                    let c=floor(p/v.w)
                    if self.vox_inside(l,c) {return vec4(l,c)}
                }
                l=l+1.0
            }
            return vec4(-1.0,0.0,0.0,0.0)
        }
        vox_free: fn(p:vec3,first:float)->bool {
            let f=self.vox_find(p,first)
            if f.x<0.0 {return true}
            return !self.vox_solid(f.x,f.yzw)
        }
        // DDA through the finest level first, then coarser levels from
        // where the ray left the finer window. vec4(t, level, entry axis
        // (+-1..3, 0 = start voxel), 1 hit / 0 miss / -1 step budget spent).
        vox_trace: fn(ro:vec3,rd:vec3,tmax:float,first:float)->vec4 {
            let stp=step(vec3(0.0,0.0,0.0),rd)*2.0-vec3(1.0,1.0,1.0)
            let inv=stp/max(abs(rd),vec3(0.000001,0.000001,0.000001))
            var level=first
            var t=0.0
            var steps=0.0
            var skip=false
            loop {
                if level>=self.gi_vox_dims.w || t>=tmax {break}
                let v=self.vox_level(level)
                var cell=floor((ro+rd*t)/max(v.w,0.000001))
                if v.w>0.0 && self.vox_inside(level,cell) {
                    var next=((cell+max(stp,vec3(0.0,0.0,0.0)))*v.w-ro)*inv
                    let delta=abs(inv)*v.w
                    var axis=0.0
                    loop {
                        steps=steps+1.0
                        if steps>320.0 {return vec4(t,level,axis,-1.0)}
                        // Entering a coarser level mid-ray, the voxel we are
                        // already in holds the finer geometry we just passed.
                        if !skip && self.vox_solid(level,cell) {return vec4(t,level,axis,1.0)}
                        skip=false
                        if next.x<next.y && next.x<next.z {
                            t=next.x
                            cell.x=cell.x+stp.x
                            next.x=next.x+delta.x
                            axis=stp.x
                        } else if next.y<next.z {
                            t=next.y
                            cell.y=cell.y+stp.y
                            next.y=next.y+delta.y
                            axis=stp.y*2.0
                        } else {
                            t=next.z
                            cell.z=cell.z+stp.z
                            next.z=next.z+delta.z
                            axis=stp.z*3.0
                        }
                        if t>=tmax || !self.vox_inside(level,cell) {break}
                    }
                }
                level=level+1.0
                skip=true
            }
            return vec4(tmax,level,0.0,0.0)
        }
    }
    mod.draw.DrawGiTrace = set_type_default() do #(DrawGiTrace::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..GiBatch,
        ..GiVoxels,
        alpha_blend: false
        color_format: @Rgba32F
        pack: fn(v:vec3)->float {
            let b=floor(clamp(v,vec3(0.0,0.0,0.0),vec3(1.0,1.0,1.0))*255.0+vec3(0.5,0.5,0.5))
            return b.x+b.y*256.0+b.z*65536.0
        }
        // Move a probe home out of solid voxels: the nearest free candidate
        // within 0.45 cells, else the probe is inside geometry (state -1).
        place: fn(k:float,home:vec3)->vec4 {
            let s=self.gi_cascade(k).w
            var i=0.0
            loop {
                if i>12.5 {break}
                var o=vec3(0.0,0.0,0.0)
                if i>0.5 {
                    let j=floor((i-1.0)*0.5)
                    let axis=j-floor(j/3.0)*3.0
                    var mag=0.3
                    if i>6.5 {mag=0.45}
                    let sgn=1.0-2.0*((i-1.0)-floor((i-1.0)*0.5)*2.0)
                    // +y first: probes are most often buried in floors.
                    if axis<0.5 {o=vec3(0.0,sgn*mag,0.0)} else if axis<1.5 {o=vec3(sgn*mag,0.0,0.0)} else {o=vec3(0.0,0.0,sgn*mag)}
                }
                let p=home+o*s
                if self.vox_free(p,k) {return vec4(p,1.0)}
                i=i+1.0
            }
            return vec4(home,-1.0)
        }
        pixel: fn(){
            let row=self.batch_row()
            let col=floor(self.pos.x*65.0)
            let a=self.batch_a(row)
            let b=self.batch_b(row)
            let s=self.gi_cascade(a.y).w
            let probe=self.place(a.y,(b.xyz+self.gi_home.xyz)*s)
            if col>63.5 {return probe}
            // hits: (t, albedo, normal (negative = backface), emission);
            // y = -1 miss, -2 invalid (probe or ray unusable).
            if probe.w<0.0 {return vec4(0.0,-2.0,0.0,0.0)}
            let rd=self.gi_ray(col,a)
            let hit=self.vox_trace(probe.xyz,rd,self.gi_pass.z,a.y)
            if hit.w<0.0 {return vec4(0.0,-2.0,0.0,0.0)}
            if hit.w<0.5 {return vec4(self.gi_pass.z,-1.0,0.0,0.0)}
            let v=self.vox_level(hit.y)
            let cell=floor((probe.xyz+rd*(hit.x+v.w*0.01))/v.w)
            let uv=self.vox_uv(hit.y,cell)
            let albedo=self.gi_vox_a.sample_nearest(uv).xyz
            let stored=self.gi_vox_b.sample_nearest(uv).xyz*2.0-vec3(1.0,1.0,1.0)
            let emission=self.gi_vox_c.sample_nearest(uv)
            var face=rd*(-1.0)
            let ax=abs(hit.z)
            if ax>0.5 {
                if ax<1.5 {face=vec3(0.0-sign(hit.z),0.0,0.0)} else if ax<2.5 {face=vec3(0.0,0.0-sign(hit.z),0.0)} else {face=vec3(0.0,0.0,0.0-sign(hit.z))}
            }
            // Averaged voxel normals: a coherent one decides the side; two
            // faces of a thin wall cancel out and the entered face is used.
            var n=face
            var back=false
            if length(stored)>0.5 {
                let facing=dot(normalize(stored),rd)
                if facing<(-0.1) {n=normalize(stored)}
                if facing>0.3 {back=true}
            }
            var packed=self.pack(n*0.5+vec3(0.5,0.5,0.5))
            if back {packed=0.0-1.0-packed}
            let e=floor(clamp(emission,vec4(0.0,0.0,0.0,0.0),vec4(1.0,1.0,1.0,1.0))*vec4(31.0,31.0,31.0,255.0)+vec4(0.5,0.5,0.5,0.5))
            return vec4(hit.x,self.pack(albedo),packed,e.x+e.y*32.0+e.z*1024.0+e.w*32768.0)
        }
        fragment: fn(){self.fb0=self.pixel()}
    }
    mod.draw.DrawGiRelight = set_type_default() do #(DrawGiRelight::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..FastGiSampling,
        ..GiBatch,
        ..GiVoxels,
        ..mod.draw.ClusteredLighting,
        alpha_blend: false
        color_format: @Rgba32F
        gi_hits: texture_2d(float)
        gi_sun_dir: uniform(vec3(0.0,1.0,0.0))
        gi_sun_color: uniform(vec3(0.0,0.0,0.0))
        gi_sky: uniform(vec3(0.1,0.1,0.1))
        gi_ground: uniform(vec3(0.1,0.1,0.1))
        // feedback, linear albedo, emission gain, sun ray length
        gi_relight: uniform(vec4(0.85,0.0,0.0,48.0))
        gi_light_ids0: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        gi_light_ids1: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        hit_at: fn(ray:float,row:float)->vec4 {return self.gi_hits.sample_nearest(vec2((ray+0.5)/65.0,(row+0.5)/self.gi_pass.y))}
        local_bounce: fn(p:vec3,n:vec3,index:float)->vec3 {
            if index<0.0 {return vec3(0.0,0.0,0.0)}
            let pos=self.cluster_fetch(self.cluster_z.w+index*3.0)
            let color=self.cluster_fetch(self.cluster_z.w+index*3.0+1.0)
            let direction=self.cluster_fetch(self.cluster_z.w+index*3.0+2.0)
            let delta=pos.xyz-p
            let distance=max(length(delta),0.02)
            if distance>=pos.w{return vec3(0.0,0.0,0.0)}
            let l=delta/distance
            var attenuation=pow(max(1.0-distance/pos.w,0.0),2.0)
            if direction.w>=0.0 || (direction.w < -1.5 && direction.w > -2.5) {
                let ratio=distance/pos.w
                let cutoff=max(1.0-ratio*ratio*ratio*ratio,0.0)
                attenuation=cutoff*cutoff/(distance*distance)
            }
            var cone=1.0
            if direction.w>=0.0 || direction.w < -2.5 {
                var inner=direction.w
                if direction.w < -2.5 {inner=-3.0-direction.w}
                cone=pow(clamp((dot(direction.xyz,l*(-1.0))-color.w)/max(inner-color.w,0.00001),0.0,1.0),2.0)
            } else if direction.w > -1.5 && color.w>0.001 {
                cone=mix(1.0,pow(max(dot(direction.xyz,l*(-1.0)),0.0),2.0),color.w)
            }
            return color.xyz*(attenuation*cone*max(dot(n,l),0.0)*self.local_shadow_visibility(index,p,n,pos.xyz,pos.w))
        }
        pixel: fn(){
            let row=self.batch_row()
            let ray=min(floor(self.pos.x*64.0),63.0)
            let probe=self.hit_at(64.0,row)
            let hit=self.hit_at(ray,row)
            // radiance: (rgb, distance); w = -1 backface, -2 invalid.
            if probe.w<0.0 || hit.y<(-1.5) {return vec4(0.0,0.0,0.0,-2.0)}
            let a=self.batch_a(row)
            let rd=self.gi_ray(ray,a)
            if hit.y<(-0.5) {return vec4(mix(self.gi_ground,self.gi_sky,clamp(rd.y*0.5+0.5,0.0,1.0)),hit.x)}
            if hit.z<0.0 {return vec4(0.0,0.0,0.0,-1.0)}
            var albedo=min(self.gi_unpack(hit.y),vec3(0.9,0.9,0.9))
            if self.gi_relight.y>0.5 {albedo=srgb_to_linear(albedo)}
            let n=normalize(self.gi_unpack(hit.z)*2.0-vec3(1.0,1.0,1.0))
            // Emission: 5-bit hue per channel + 8-bit intensity m/(1+m).
            // The linear lane decodes the hue like an albedo and applies the
            // lanes' glow gain (gi_relight.z).
            let level=floor(hit.w/32768.0)
            let rest=hit.w-level*32768.0
            var hue=vec3(rest-floor(rest/32.0)*32.0,floor(rest/32.0)-floor(rest/1024.0)*32.0,floor(rest/1024.0))/31.0
            if self.gi_relight.y>0.5 {hue=srgb_to_linear(hue)}
            let glow=min(level/255.0,0.996)
            let emission=hue*(glow/(1.0-glow))*self.gi_relight.z
            let p=probe.xyz+rd*hit.x
            var light=vec3(0.0,0.0,0.0)
            let ndl=dot(n,self.gi_sun_dir)
            if ndl>0.0 && max(max(self.gi_sun_color.x,self.gi_sun_color.y),self.gi_sun_color.z)>0.0 {
                // Sun visibility through the same voxels: works without a
                // shadow map, in every shadow tier and beyond the CSM.
                let v=self.vox_level(a.y)
                let origin=p+n*(v.w*0.75)-rd*(v.w*0.25)
                let shadow=self.vox_trace(origin,self.gi_sun_dir,self.gi_relight.w,a.y)
                if shadow.w<0.5 && shadow.w>(-0.5) {light=self.gi_sun_color*ndl}
            }
            light=light+self.local_bounce(p,n,self.gi_light_ids0.x)+self.local_bounce(p,n,self.gi_light_ids0.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids0.z)+self.local_bounce(p,n,self.gi_light_ids0.w)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.x)+self.local_bounce(p,n,self.gi_light_ids1.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.z)+self.local_bounce(p,n,self.gi_light_ids1.w)
            // Further bounces: last frame's field AT THE HIT, no recursion.
            if self.gi_relight.x>0.0 {
                light=light+min(self.gi_ambient(p,n,vec3(0.0,0.0,0.0)),vec3(64.0,64.0,64.0))*self.gi_relight.x
            }
            return vec4(min(albedo*light+emission,vec3(64.0,64.0,64.0)),hit.x)
        }
        fragment: fn(){self.fb0=self.pixel()}
    }
    mod.draw.DrawGiGather = set_type_default() do #(DrawGiGather::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..FastGiSampling,
        ..GiBatch,
        alpha_blend: false
        color_format: @Rgba16F
        gi_hits: texture_2d(float)
        gi_radiance: texture_2d(float)
        radiance: fn(r:float,row:float)->vec4 {return self.gi_radiance.sample_nearest(vec2((r+0.5)/64.0,(row+0.5)/self.gi_pass.y))}
        // Direction of texel (x, y) of an n x n interior with a guard ring:
        // guard texels fold onto their octahedral neighbours.
        tile_dir: fn(x:float,y:float,n:float)->vec3 {
            var b=vec2(x-1.0,y-1.0)
            if b.x<0.0 {b=vec2(-1.0-b.x,n-1.0-b.y)}
            if b.x>n-1.0 {b=vec2(2.0*n-1.0-b.x,n-1.0-b.y)}
            if b.y<0.0 {b=vec2(n-1.0-b.x,-1.0-b.y)}
            if b.y>n-1.0 {b=vec2(n-1.0-b.x,2.0*n-1.0-b.y)}
            let f=(b+vec2(0.5,0.5))/n*2.0-vec2(1.0,1.0)
            var d=vec3(f.x,f.y,1.0-abs(f.x)-abs(f.y))
            if d.z<0.0 {d=vec3((1.0-abs(f.y))*sign(f.x),(1.0-abs(f.x))*sign(f.y),d.z)}
            return normalize(d)
        }
        // Cosine-weighted radiance around d (E/pi): irradiance texel.
        irradiance: fn(d:vec3,row:float,a:vec4)->vec4 {
            var sum=vec3(0.0,0.0,0.0)
            var total=0.0
            var r=0.0
            loop {
                if r>63.5 {break}
                let sample=self.radiance(r,row)
                if sample.w>=0.0 {
                    let w=max(dot(d,self.gi_ray(r,a)),0.0)
                    sum=sum+sample.xyz*w
                    total=total+w
                }
                r=r+1.0
            }
            if total<=0.0001 {return vec4(0.0,0.0,0.0,-1.0)}
            return vec4(sum/total,1.0)
        }
        // Filtered (E[d], E[d^2]) around d from this update's rays.
        moments: fn(d:vec3,row:float,a:vec4,s:float)->vec4 {
            var m=vec2(0.0,0.0)
            var total=0.0
            var r=0.0
            loop {
                if r>63.5 {break}
                let sample=self.radiance(r,row)
                if sample.w>=0.0 {
                    let c=max(dot(d,self.gi_ray(r,a)),0.0)
                    let c2=c*c
                    let c4=c2*c2
                    let w=c4*c4*c4
                    let dist=min(sample.w,s*2.0)
                    m=m+vec2(dist,dist*dist)*w
                    total=total+w
                }
                r=r+1.0
            }
            if total<=0.0001 {return vec4(0.0,0.0,0.0,-1.0)}
            return vec4(m/total,0.0,1.0)
        }
        // Columns: 0..63 irradiance tile (8x8), 64..163 moment tile (10x10),
        // 164 info. Each texel blends with the same texel of the field.
        pixel: fn(){
            let row=self.batch_row()
            let col=min(floor(self.pos.x*165.0),164.0)
            let a=self.batch_a(row)
            let b=self.batch_b(row)
            let k=a.y
            let s=self.gi_cascade(k).w
            let probe=self.gi_hits.sample_nearest(vec2(64.5/65.0,(row+0.5)/self.gi_pass.y))
            let home=(b.xyz+self.gi_home.xyz)*s
            let offset=(probe.xyz-home)/s
            let old_info=self.gi_field.sample_nearest((self.gi_tile(k,a.x,2.0)+vec2(0.5,0.5))/self.gi_atlas.xy)
            let od=abs(old_info.xyz-offset)
            // History only from this very probe (same cell, same place);
            // hysteresis 0 marks a slot that just changed cells.
            let history=a.z>0.0 && old_info.w>0.0 && probe.w>0.0 && max(max(od.x,od.y),od.z)<0.05
            var h=a.z
            if !history {h=0.0}
            if col>163.5 {
                var back=0.0
                var r=0.0
                loop {
                    if r>63.5 {break}
                    let w=self.radiance(r,row).w
                    if w<(-0.5) && w>(-1.5) {back=back+1.0}
                    r=r+1.0
                }
                // Mostly backfaces: the probe sits inside closed geometry
                // (below terrain, inside a wall box).
                if probe.w<0.0 || back>16.0 {return vec4(0.0,0.0,0.0,-1.0)}
                var state=0.34
                if history {state=min(old_info.w+0.34,1.0)}
                return vec4(offset,state)
            }
            if col<63.5 {
                let x=col-floor(col/8.0)*8.0
                let y=floor(col/8.0)
                let old=self.gi_field.sample_nearest((self.gi_tile(k,a.x,0.0)+vec2(x+0.5,y+0.5))/self.gi_atlas.xy)
                let v=self.irradiance(self.tile_dir(x,y,6.0),row,a)
                if v.w<0.0 {
                    if h>0.0 {return old}
                    return vec4(0.0,0.0,0.0,0.0)
                }
                return vec4(mix(v.xyz,old.xyz,h),1.0)
            }
            let c=col-64.0
            let x=c-floor(c/10.0)*10.0
            let y=floor(c/10.0)
            let old=self.gi_field.sample_nearest((self.gi_tile(k,a.x,1.0)+vec2(x+0.5,y+0.5))/self.gi_atlas.xy)
            let m=self.moments(self.tile_dir(x,y,8.0),row,a,s)
            if m.w<0.0 {
                if h>0.0 {return old}
                return vec4(0.0,0.0,0.0,0.0)
            }
            if h>0.0 && old.w>0.5 {return vec4(mix(m.xy,old.xy,h),0.0,1.0)}
            return m
        }
        fragment: fn(){self.fb0=self.pixel()}
    }
    // Copies one gathered tile (x = batch row, y = first column, z = tile
    // width) into the persistent field; a negative row writes zeros (the
    // info texel of a slot that just changed cells).
    mod.draw.DrawGiScatter = set_type_default() do #(DrawGiScatter::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        alpha_blend: false
        color_format: @Rgba16F
        gi_update: texture_2d(float)
        gi_update_size: uniform(vec2(165.0,1.0))
        pixel: fn(){
            if self.gi_tile.x<0.0 {return vec4(0.0,0.0,0.0,0.0)}
            let w=self.gi_tile.z
            let col=self.gi_tile.y+min(floor(self.pos.x*w),w-1.0)+min(floor(self.pos.y*w),w-1.0)*w
            return self.gi_update.sample_nearest(vec2((col+0.5)/self.gi_update_size.x,(self.gi_tile.x+0.5)/self.gi_update_size.y))
        }
        fragment: fn(){self.fb0=self.pixel()}
    }

    mod.draw.FastGiSampling = FastGiSampling
}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiTrace {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiRelight {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiGather {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiScatter {#[deref] pub quad:DrawQuad, #[live(vec4(0.0,0.0,1.0,0.0))] pub gi_tile:Vec4f}
