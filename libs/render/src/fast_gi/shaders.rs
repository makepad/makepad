//! Cascaded probe-GI kernels: fragment passes only. Diagnostic readback is opt-in.
//!
//! Frame: TRACE (per batch probe: place it off geometry, 64 rotated rays
//! through the voxel clipmap) -> RELIGHT (shade each hit: sun through the
//! voxels, clustered lights, last frame's field for further bounces; a miss
//! is marked, not lit) -> GATHER (L1 irradiance + 8x8 octahedral distance
//! moments, blended with the probe's history) -> SCATTER (copy the updated
//! probe rows into the persistent field; untouched probes are never
//! rewritten).
//!
//! Field: an RGBA16F atlas, per cascade and probe slot an 8x8 irradiance
//! tile (6x6 octahedral interior + guard ring: rgb the cosine-weighted
//! radiance the scene's surfaces send, a the cosine-weighted share of rays
//! that left the scene), a 10x10 moment tile (8x8 interior + guard, E[d]
//! and E[d^2]) and one info texel (relocation offset in cells, state).
//! Receivers read 3 texels per probe with hardware bilinear filtering.
//! state > 0 is valid (ramping in over the first updates), < 0 inside
//! geometry, 0 not traced.
//!
//! The sky is not in the field: a receiver's ambient is its OWN ambient
//! (the environment it lights with GI off: a hemisphere, a map's SH) times
//! the field's open share, plus the field's bounce. Open space therefore
//! looks exactly as with GI off; a cavity darkens; a coloured neighbour
//! adds its reflected light, never more than it receives.
//! A ray that runs out inside the padded scene box carries no data: it
//! adds neither sky nor bounce nor distance moments, and is no backface.
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
        // 1: receivers leave out probes behind their own surface
        // (`GiConfig::thin_walls`).
        gi_back_cut: uniform(0.0)
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
            // Level zero: no gradients, so the probe loop stays a loop.
            return self.gi_field.sample_lod(p/self.gi_atlas.xy,0.0)
        }
        // (relocation offset in cells, state): state > 0 valid (ramps in
        // over the first updates), < 0 inside geometry, 0 not traced.
        gi_info: fn(k:float,cell:vec3)->vec4 {
            let p=self.gi_tile(k,self.gi_slot(cell),2.0)+vec2(0.5,0.5)
            // At the texel's centre, level zero: its exact value.
            return self.gi_field.sample_lod(p/self.gi_atlas.xy,0.0)
        }
        // One probe's trilinear and maturity weight, adjusted for visibility and facing.
        gi_probe_weight: fn(k:float,cell:vec3,slot:float,info:vec4,wp:vec3,n:vec3,w:float)->float {
            if w<=0.0 {return 0.0}
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
            // The moments are distances to the voxel proxy (a cascade's
            // voxels are a quarter of its spacing, voxels.rs Layout), the
            // receiver is the true surface: its own proxy can sit up to a
            // voxel diagonal in front of it. A smaller allowance let a part
            // shadow itself in patches cut by the voxel faces.
            let difference=max(distance-mean-s*0.4330127,0.0)
            let variance=max(moments.y-mean*mean,s*s*0.0001)
            var visibility=variance/(variance+difference*difference)
            if mean<=0.0 {visibility=1.0}
            // Facing uses the UNBIASED receiver, bounded to a sub-cell
            // footprint so a probe almost on a wall makes no weight spike.
            let facing=pow(clamp(dot(n,to_probe)/max(probe_distance,s*0.25)*0.5+0.5,0.0,1.0),2.0)
            var weight=w*visibility*visibility*max(facing,0.02)
            // A host with walls thinner than the voxels leaves out the
            // probes behind the receiver's own plane: they see the inside.
            // Their weight ramps to zero AT the plane: a cut there printed
            // each probe's crossing of a curved surface's tangent planes as
            // a seam (a vertical line down a vase, a circle round a belly).
            if self.gi_back_cut>0.5 {weight=weight*smoothstep(0.0,0.3,dot(n,to_probe)/max(probe_distance,s*0.25))}
            return weight
        }
        // Probe bounce in RGB and the open share for the receiver's own ambient in A.
        gi_probe_light: fn(k:float,slot:float,n:vec3)->vec4 {
            if self.gi_debug>2.5 && self.gi_debug<3.5 {return vec4(self.gi_hash(vec3(slot,k,1.0)),0.0)}
            let t=self.gi_tile_sample(k,slot,0.0,n)
            return vec4(max(t.xyz,vec3(0.0,0.0,0.0)),clamp(t.w,0.0,1.0))
        }
        // Finest confident cascade first, then coarser, then the receiver's own
        // ambient: an untraced, occluded or out-of-volume receiver keeps the
        // look it has with GI off. The gather is linear in that ambient F:
        // `gi_gather(wp,n,F)` = rgb + F x a, rgb the bounce, a the open share
        // with the fallback floor and the GI blend folded in, so a cache can
        // hold (rgb, a) and each pixel add its own F. `weights` 1 returns
        // each cascade's share instead (the debug views): one loop serves both.
        gi_gather_split: fn(wp:vec3,n:vec3,weights:float)->vec4 {
            var acc=vec4(0.0,0.0,0.0,0.0)
            var shares=vec3(0.0,0.0,0.0)
            var remain=1.0
            for k in 0..3 {
                let kf=float(k)
                // Cascade 0 always; a coarser one while weight remains and
                // the volume has it.
                if kf<0.5 || (remain>0.002 && self.gi_home.w>kf+0.5) || weights>0.5 {
                    let c=self.gi_cascade(kf)
                    if c.w>0.0 {
                        // Interpolate on the AIR side of the receiver.
                        let local=(wp+n*(c.w*0.1))/c.w-self.gi_home.xyz-c.xyz
                        let upper=self.gi_grid.xyz-vec3(1.0,1.0,1.0)-local
                        let edge=min(min(min(local.x,local.y),local.z),min(min(upper.x,upper.y),upper.z))
                        if edge>0.0 {
                            let base=floor(local)
                            let f=local-base
                            let b0=c.xyz+base
                            // The eight corners in one loop (one call site each: a D3D
                            // compile inlines every site). Trilinear weight x maturity (a
                            // new probe fades in).
                            var available=0.0
                            var sum=vec4(0.0,0.0,0.0,0.0)
                            var total=0.0
                            for corner in 0..8 {
                                let q=float(corner)
                                let o=vec3(fract(q*0.5)*2.0,fract(floor(q*0.5)*0.5)*2.0,floor(q*0.25))
                                let tw=mix(vec3(1.0,1.0,1.0)-f,f,o)
                                let cell=b0+o
                                let slot=self.gi_slot(cell)
                                let info=self.gi_info(kf,cell)
                                let w=tw.x*tw.y*tw.z*max(info.w,0.0)
                                available=available+w
                                let pw=self.gi_probe_weight(kf,cell,slot,info,wp,n,w)
                                if w>0.0 {
                                    sum=sum+self.gi_probe_light(kf,slot,n)*pw
                                    total=total+pw
                                }
                            }
                            if available>0.000001 && total>0.00000001 {
                                // Coverage (enough traced, valid probes) times visibility (the
                                // receiver is not hidden from all of them); both fade to the
                                // next cascade, never to black.
                                let support=smoothstep(0.05,0.4,available)*smoothstep(0.0,0.05,total/available)
                                let confidence=support*clamp(edge,0.0,1.0)
                                let share=confidence*remain
                                acc=acc+(sum/total)*share
                                shares=shares+vec3(1.0-step(0.5,kf),step(0.5,kf)-step(1.5,kf),step(1.5,kf))*share
                                remain=remain*(1.0-confidence)
                            }
                        }
                    }
                }
            }
            if weights>0.5 {return vec4(shares,0.0)}
            let g=min(self.gi_on,1.0)
            return vec4(g*acc.xyz,(1.0-g)+g*(acc.w+self.gi_fallback_floor*remain))
        }
        gi_gather: fn(wp:vec3,n:vec3,fallback:vec3,weights:float)->vec3 {
            let g=self.gi_gather_split(wp,n,weights)
            if weights>0.5 {return g.xyz}
            return g.xyz+fallback*g.w
        }
        // The screen fill and a host's own G-buffer store bounce in RGB. Alpha
        // packs the rounded open share and the log distance from the cache eye.
        gi_screen_value: fn(wp:vec3,n:vec3,view_distance:float)->vec4 {
            let g=self.gi_gather_split(wp,normalize(n),0.0)
            return vec4(g.xyz,floor(clamp(g.w,0.0,1.0)*255.0+0.5)+log2(1.0+view_distance)/16.0)
        }
        gi_ambient: fn(wp:vec3,normal:vec3,fallback:vec3)->vec3 {
            if self.gi_on<=0.0 {return fallback}
            return self.gi_gather(wp,normalize(normal),fallback,0.0)
        }
        gi_screen_hit: fn(wp:vec3,n_geo:vec3)->float {return -1.0}
        gi_ambient_at: fn(wp:vec3,n_geo:vec3,n:vec3,fallback:vec3)->vec3 {
            return self.gi_ambient(wp,n,fallback)
        }
        // Reuse the diffuse sample, including its cascade confidence and
        // fallback. No additional field or environment texture reads.
        gi_specular_occlusion: fn(ambient:vec3,hemisphere:vec3)->float {
            if self.gi_on<=0.0 {return 1.0}
            let luma=vec3(0.2126,0.7152,0.0722)
            let spec_occ=clamp(dot(ambient,luma)/max(dot(hemisphere,luma),0.0001),0.0,1.0)
            return mix(1.0,spec_occ,min(self.gi_on,1.0))
        }
        // Display receivers supply output_dither; debug views keep their exact colour.
        gi_display: fn(color:vec4,wp:vec3,n:vec3)->vec4 {
            if color.w<0.999 {return color}
            if self.gi_on<=0.0 {return vec4(self.output_dither(color.xyz,wp),color.w)}
            if self.gi_debug>0.5 {
                if self.gi_debug>6.5 {return vec4(0.3,0.3,0.3,1.0)}
                let nn=normalize(n)
                // Each cascade's share (views 1, 2), or the GI under a
                // uniform grey ambient (3, 4): bounce + open share x 0.5.
                let g=self.gi_gather(wp,nn,vec3(0.5,0.5,0.5),1.0-step(2.5,self.gi_debug))
                if self.gi_debug<2.5 {
                    let w=g
                    if self.gi_debug>1.5 {return vec4(w,1.0)}
                    // Cells of the finest cascade that covers the pixel,
                    // tinted per cascade (red fine, green mid, blue coarse).
                    var k=0.0
                    if w.x<0.001 {k=1.0}
                    if w.x<0.001 && w.y<0.001 {k=2.0}
                    let c=self.gi_cascade(k)
                    let cell=floor(wp/max(c.w,0.001)-self.gi_home.xyz)
                    let tint=vec3(step(k,0.5),step(abs(k-1.0),0.5),step(1.5,k))
                    return vec4(mix(fract(cell*vec3(0.37,0.57,0.73)),tint,0.5),1.0)
                }
                if self.gi_debug>4.5 {
                    // Nearest probe of cascade 0: orange = inside geometry,
                    // grey = not traced, green = valid (dim while fading in).
                    let c=self.gi_c0
                    let cell=floor((wp+nn*(c.w*0.1))/max(c.w,0.001)-self.gi_home.xyz+vec3(0.5,0.5,0.5))
                    let info=self.gi_info(0.0,cell)
                    if info.w<0.0 {return vec4(1.0,0.4,0.0,1.0)}
                    if info.w==0.0 {return vec4(0.1,0.1,0.1,1.0)}
                    if self.gi_debug>5.5 {
                        let t=self.gi_tile_sample(0.0,self.gi_slot(cell),0.0,nn)
                        return vec4(max(t.xyz,vec3(0.0,0.0,0.0))+vec3(0.5,0.5,0.5)*clamp(t.w,0.0,1.0),1.0)
                    }
                    return vec4(0.0,0.8*info.w,0.1,1.0)
                }
                return vec4(g,1.0)
            }
            return vec4(self.output_dither(color.xyz,wp),color.w)
        }
        gi_display_at: fn(color:vec4,wp:vec3,n_geo:vec3,n:vec3)->vec4 {
            if self.gi_on>0.0 && color.w>=0.999 && self.gi_debug>6.5 {
                let hit=self.gi_screen_hit(wp,n_geo)
                if hit>0.5 {return vec4(0.1,0.8,0.1,1.0)}
                if hit>(-0.5) {return vec4(0.9,0.15,0.1,1.0)}
                return vec4(0.3,0.3,0.3,1.0)
            }
            return self.gi_display(color,wp,n)
        }
    }
    // Spread BEFORE FastGiSampling: a spread adds only missing keys, so this
    // gi_ambient_at (reprojecting into the fill's view) and gi_screen_hit win.
    let FastGiScreen = {
        gi_screen: texture_2d(float)
        // x enables the cache, yz gives its dimensions in texels.
        gi_screen_ctl: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_screen_clip: uniform(mat4x4f(1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,0.0,1.0))
        gi_screen_eye: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_screen_ray0: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_screen_ray_u: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_screen_ray_v: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_screen_fetch: fn(wp:vec3,n_geo:vec3)->vec4 {
            let cp=self.gi_screen_clip*vec4(wp,1.0)
            if cp.w<=0.0 {return vec4(0.0,0.0,0.0,-1.0)}
            let uv=vec2(cp.x/cp.w*0.5+0.5,0.5-cp.y/cp.w*0.5)
            let eye=self.gi_screen_eye.xyz
            let d=length(wp-eye)
            let size=self.gi_screen_ctl.yz
            let px=uv*size-vec2(0.5,0.5)
            let b=floor(px)
            let f=px-b
            var sum=vec4(0.0,0.0,0.0,0.0)
            var wsum=0.0
            for i in 0..4 {
                let q=float(i)
                let o=vec2(fract(q*0.5)*2.0,floor(q*0.5))
                let texel=b+o
                if texel.x>=0.0 && texel.y>=0.0 && texel.x<size.x && texel.y<size.y {
                    let tuv=(texel+vec2(0.5,0.5))/size
                    // Level zero, as the probe gather reads: an implicit-derivative
                    // read in a loop under per-pixel branches has no defined gradients.
                    let t=self.gi_screen.sample_nearest(tuv,0.0)
                    if t.w>=0.0 {
                        let td=exp2(fract(t.w)*16.0)-1.0
                        let ray=self.gi_screen_ray0.xyz+tuv.x*self.gi_screen_ray_u.xyz+tuv.y*self.gi_screen_ray_v.xyz
                        let tp=eye+normalize(ray)*td
                        if abs(dot(n_geo,tp-wp))<=0.002*d {
                            let tw=mix(vec2(1.0,1.0)-f,f,o)
                            let w=tw.x*tw.y
                            sum=sum+w*vec4(t.xyz,floor(t.w)/255.0)
                            wsum=wsum+w
                        }
                    }
                }
            }
            if wsum<0.2 {return vec4(0.0,0.0,0.0,-1.0)}
            return sum/wsum
        }
        gi_screen_hit: fn(wp:vec3,n_geo:vec3)->float {
            if self.gi_screen_ctl.x<0.5 {return -1.0}
            if self.gi_screen_fetch(wp,n_geo).w<0.0 {return 0.0}
            return 1.0
        }
        gi_ambient_at: fn(wp:vec3,n_geo:vec3,n:vec3,fallback:vec3)->vec3 {
            if self.gi_on<=0.0 {return fallback}
            if self.gi_screen_ctl.x<0.5 {return self.gi_gather(wp,normalize(n),fallback,0.0)}
            let c=self.gi_screen_fetch(wp,n_geo)
            if c.w>=0.0 {return c.xyz+fallback*c.w}
            let g=self.gi_gather_split(wp,n_geo,0.0)
            return g.xyz+fallback*g.w
        }
        // Receive once at the top of the pixel. Shading adds the surface's own fill as rgb + fallback * a.
        gi_receive_hoisted: fn(wp:vec3,n_geo:vec3)->vec4 {
            if self.gi_on<=0.0 {return vec4(0.0,0.0,0.0,1.0)}
            if self.gi_screen_ctl.x<0.5 {return self.gi_gather_split(wp,n_geo,0.0)}
            let c=self.gi_screen_fetch(wp,n_geo)
            if c.w>=0.0 {return c}
            return self.gi_gather_split(wp,n_geo,0.0)
        }
    }
    // All four passes share the same row map for this issue. Only the three
    // render targets have gaps; the scheduler's two-column list stays dense.
    let GiBatchLayout = {
        gi_batch: texture_2d(float)
        gi_batch_height: uniform(1.0)
        // Band height, render target height, ray length, frame seed.
        gi_pass: uniform(vec4(1.0,1.0,32.0,0.0))
        batch_y: fn(row:float)->float {return (row+0.5)/self.gi_batch_height}
        target_y: fn(row:float)->float {
            let band=floor(row/self.gi_pass.x)
            let physical=band*#(super::BAND_PITCH as f64)+(row-band*self.gi_pass.x)
            return (physical+0.5)/self.gi_pass.y
        }
    }
    // Logical batch row = (slot, cascade, hysteresis, -), (cell xyz, -).
    let GiBatch = {
        ..GiCascades,
        ..GiBatchLayout,
        gi_frag_pos: fragment_position()
        batch_row: fn()->float {
            let physical=floor(self.gi_frag_pos.y)
            let pitch=#(super::BAND_PITCH as f64)
            let band=floor(physical/pitch)
            return band*self.gi_pass.x+(physical-band*pitch)
        }
        batch_a: fn(row:float)->vec4 {return self.gi_batch.sample_nearest(vec2(0.25,self.batch_y(row)))}
        batch_b: fn(row:float)->vec4 {return self.gi_batch.sample_nearest(vec2(0.75,self.batch_y(row)))}
        // Spherical Fibonacci set under a random rotation per probe and
        // update: fixed directions printed probe-aligned stripes.
        gi_ray: fn(r:float,a:vec4)->vec3 {
            let v=self.gi_fib(r)
            let qv=self.gi_quat(a)
            return normalize(v+cross(qv.xyz,cross(qv.xyz,v)+v*qv.w)*2.0)
        }
        // Ray r of the unrotated set.
        gi_fib: fn(r:float)->vec3 {
            let z=1.0-(2.0*r+1.0)/64.0
            let q=sqrt(max(1.0-z*z,0.0))
            let phi=r*2.3999632
            return vec3(q*cos(phi),z,q*sin(phi))
        }
        // The probe's rotation for this update, a unit quaternion.
        gi_quat: fn(a:vec4)->vec4 {
            let u=self.gi_hash(vec3(a.x+0.5,a.y*7.0+1.0,self.gi_pass.w))
            let s1=sqrt(1.0-u.x)
            let s2=sqrt(u.x)
            return vec4(s1*sin(6.2831853*u.y),s1*cos(6.2831853*u.y),s2*sin(6.2831853*u.z),s2*cos(6.2831853*u.z))
        }
        // d turned back by the probe's rotation, so dot(gi_ray_axis(d,a), gi_fib(r)) is
        // dot(d, gi_ray(r,a)) to float rounding: the gather builds the rotation once per
        // texel instead of once per ray.
        gi_ray_axis: fn(d:vec3,a:vec4)->vec3 {
            let qv=self.gi_quat(a)
            return d+cross(qv.xyz,cross(qv.xyz,d)-d*qv.w)*2.0
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
        // Snapshot geometry bounds, padded by one coarsest voxel on the CPU.
        gi_scene_min: uniform(vec3(0.0,0.0,0.0))
        gi_scene_max: uniform(vec3(0.0,0.0,0.0))
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
        // Sample from the cell already wrapped into the window's texels (m in [0, dims)).
        vox_uv_wrapped: fn(l:float,m:vec3)->vec2 {
            let d=self.gi_vox_dims.xyz
            let tile=floor(m.z/self.gi_vox_tex.w)
            let x=m.x+d.x*(m.z-tile*self.gi_vox_tex.w)
            let y=m.y+d.y*tile+l*self.gi_vox_tex.z
            return vec2((x+0.5)/self.gi_vox_tex.x,(y+0.5)/self.gi_vox_tex.y)
        }
        // A fine step or at most one brick: a single wrap suffices.
        vox_wrap_step: fn(m:float,s:float,d:float)->float {
            let n=m+s
            if n<0.0 {return n+d}
            if n>=d {return n-d}
            return n
        }
        // A window face is also a coarser cell face. Its last distance bit
        // can change floor((ro+rd*t)/voxel) and thus the cell skipped at entry.
        // Replay just the exiting axis, once per window that used a jump.
        vox_window_exit: fn(first:float,delta:float,left:float)->float {
            var t=first
            var i=0.0
            loop {
                if i>=left {break}
                t=t+delta
                i=i+1.0
            }
            return t
        }
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
        // Both distance and resident-window endings use the actual endpoint.
        // Outside any face (including the top) is sky; inside is unknown.
        vox_end_status: fn(p:vec3)->float {
            let lo=self.gi_scene_min
            let hi=self.gi_scene_max
            if p.x<lo.x || p.y<lo.y || p.z<lo.z || p.x>hi.x || p.y>hi.y || p.z>hi.z {return 0.0}
            return 0.25
        }
        // DDA through the finest level first, then coarser levels from
        // where the ray left the finer window. vec4(t, level, entry axis
        // (+-1..3, 0 = start voxel), status: 1 hit / 0 left the scene /
        // 0.25 ran out inside the scene / -1 step budget spent).
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
                let cell=floor((ro+rd*t)/max(v.w,0.000001))
                if v.w>0.0 && self.vox_inside(level,cell) {
                    let first_next=((cell+max(stp,vec3(0.0,0.0,0.0)))*v.w-ro)*inv
                    var next=first_next
                    let delta=abs(inv)*v.w
                    let rate=vec3(1.0)/delta
                    var jumped=false
                    var axis=0.0
                    // The window test and the texel are carried from step to step instead of
                    // recomputed: the cells left ahead of the ray in the window on each axis,
                    // and the cell wrapped into the window's texels. Cells are whole numbers in
                    // floats, so both are exact: the same voxels, hit, axis and distance.
                    let d=self.gi_vox_dims.xyz
                    let ahead=step(vec3(0.0,0.0,0.0),stp)
                    let first_left=(v.xyz+d-vec3(1.0,1.0,1.0)-cell)*ahead+(cell-v.xyz)*(vec3(1.0,1.0,1.0)-ahead)
                    var left=first_left
                    var m=cell-floor(cell/d)*d
                    loop {
                        steps=steps+1.0
                        if steps>320.0 {return vec4(t,level,axis,-1.0)}
                        // Entering a coarser level mid-ray, the voxel we are
                        // already in holds the finer geometry we just passed.
                        var code=0.0
                        if !skip {
                            let w=self.gi_vox_a.sample_nearest(self.vox_uv_wrapped(level,m)).w
                            if w>0.5 {return vec4(t,level,axis,1.0)}
                            code=floor(w*255.0+0.5)
                        }
                        skip=false
                        if code==32.0 || code==64.0 {
                            let s=code*0.125
                            let offset=m-floor(m/s)*s
                            let n=mix(offset+vec3(1.0),vec3(s)-offset,ahead)
                            let face=next+(n-vec3(1.0))*delta
                            let exit_t=min(face.x,min(face.y,face.z))
                            if exit_t<tmax {
                                // k is the index of the nearest crossing. Count it if it
                                // precedes the exit, including equal times on axes that
                                // the fine step would take first (z, then y, then x).
                                let k=clamp(floor((vec3(exit_t)-next)*rate+vec3(0.5)),vec3(0.0),n-vec3(1.0))
                                let near_t=next+k*delta
                                var crossed=k+step(near_t,vec3(exit_t))
                                var entry=stp.x
                                if face.x<face.y && face.x<face.z {
                                    crossed.x=n.x
                                } else if face.y<face.z {
                                    crossed.x=k.x+1.0-step(exit_t,near_t.x)
                                    crossed.y=n.y
                                    entry=stp.y*2.0
                                } else {
                                    crossed.x=k.x+1.0-step(exit_t,near_t.x)
                                    crossed.y=k.y+1.0-step(exit_t,near_t.y)
                                    crossed.z=n.z
                                    entry=stp.z*3.0
                                }
                                // The current fine step has already been charged. Walk a
                                // partial brick normally if the budget ends inside it.
                                let after=steps-1.0+crossed.x+crossed.y+crossed.z
                                if after<=320.0 {
                                    jumped=true
                                    steps=after
                                    t=exit_t
                                    axis=entry
                                    let move_by=crossed*stp
                                    m=vec3(self.vox_wrap_step(m.x,move_by.x,d.x),self.vox_wrap_step(m.y,move_by.y,d.y),self.vox_wrap_step(m.z,move_by.z,d.z))
                                    left=left-crossed
                                    next=next+crossed*delta
                                    if min(min(left.x,left.y),left.z)<0.0 {break}
                                    continue
                                }
                            }
                        }
                        if next.x<next.y && next.x<next.z {
                            t=next.x
                            m.x=self.vox_wrap_step(m.x,stp.x,d.x)
                            left.x=left.x-1.0
                            next.x=next.x+delta.x
                            axis=stp.x
                        } else if next.y<next.z {
                            t=next.y
                            m.y=self.vox_wrap_step(m.y,stp.y,d.y)
                            left.y=left.y-1.0
                            next.y=next.y+delta.y
                            axis=stp.y*2.0
                        } else {
                            t=next.z
                            m.z=self.vox_wrap_step(m.z,stp.z,d.z)
                            left.z=left.z-1.0
                            next.z=next.z+delta.z
                            axis=stp.z*3.0
                        }
                        if t>=tmax || min(min(left.x,left.y),left.z)<0.0 {break}
                    }
                    if jumped {
                        if left.x<0.0 {t=self.vox_window_exit(first_next.x,delta.x,first_left.x)}
                        else if left.y<0.0 {t=self.vox_window_exit(first_next.y,delta.y,first_left.y)}
                        else if left.z<0.0 {t=self.vox_window_exit(first_next.z,delta.z,first_left.z)}
                    }
                }
                level=level+1.0
                skip=true
            }
            // The next voxel boundary may overshoot tmax. A window exit
            // can instead be much nearer: never classify its untraced tail.
            let end=min(t,tmax)
            return vec4(end,level,0.0,self.vox_end_status(ro+rd*end))
        }
    }
    mod.draw.DrawGiTrace = set_type_default() do #(DrawGiTrace::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..GiBatch,
        ..GiVoxels,
        alpha_blend: false
        color_format: @Rgba32F
        gi_skyline: texture_2d(float)
        gi_skyline_rect: uniform(vec4(0.0,0.0,1.0,0.0))
        gi_skyline_size: uniform(vec2(1.0,1.0))
        // Continue an unresolved voxel ray from its endpoint. The skyline
        // gives a conservative blocker height even outside the clipmap.
        sky_march: fn(p:vec3,rd:vec3)->vec2 {
            let rect=self.gi_skyline_rect
            let size=self.gi_skyline_size
            var d=rect.z
            var t=0.0
            for i in 0..12 {
                t=t+d
                d=d*1.5
                let q=p+rd*t
                if self.vox_end_status(q)==0.0 {return vec2(t,0.0)}
                let texel=clamp(floor((q.xz-rect.xy)/rect.z),vec2(0.0,0.0),size-vec2(1.0,1.0))
                let height=self.gi_skyline.sample_nearest((texel+vec2(0.5,0.5))/size).x
                if q.y<height {return vec2(t,0.5)}
            }
            return vec2(t,0.25)
        }
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
            // y = -1 sky, -2 invalid or unresolved, -3 closed far.
            if probe.w<0.0 {return vec4(0.0,-2.0,0.0,0.0)}
            let rd=self.gi_ray(col,a)
            let hit=self.vox_trace(probe.xyz,rd,self.gi_pass.z,a.y)
            if hit.w<0.0 {return vec4(0.0,-2.0,0.0,0.0)}
            if hit.w==0.25 {
                let m=self.sky_march(probe.xyz+rd*hit.x,rd)
                if m.y==0.0 {return vec4(self.gi_pass.z,-1.0,0.0,0.0)}
                if m.y==0.5 {return vec4(hit.x+m.x,-3.0,0.0,0.0)}
                return vec4(0.0,-2.0,0.0,0.0)
            }
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
        gi_bounce: uniform(1.0)
        gi_light_ids0: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        gi_light_ids1: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        hit_at: fn(ray:float,target_y:float)->vec4 {return self.gi_hits.sample_nearest(vec2((ray+0.5)/65.0,target_y))}
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
            let target_y=self.target_y(row)
            let probe=self.hit_at(64.0,target_y)
            let hit=self.hit_at(ray,target_y)
            // radiance: (rgb, distance); w = -1 backface, -2 invalid.
            // Sky has x = -1 for the receiver's ambient. Closed far has
            // zero RGB and a positive blocker distance for visibility.
            if hit.y<(-2.5) && probe.w>=0.0 {return vec4(0.0,0.0,0.0,hit.x)}
            if probe.w<0.0 || (hit.y<(-1.5) && hit.y>(-2.5)) {return vec4(0.0,0.0,0.0,-2.0)}
            if hit.y<(-0.5) {return vec4(-1.0,0.0,0.0,hit.x)}
            let a=self.batch_a(row)
            let rd=self.gi_ray(ray,a)
            if hit.z<0.0 {return vec4(0.0,0.0,0.0,-1.0)}
            var albedo=min(self.gi_unpack(hit.y),vec3(0.9,0.9,0.9))
            if self.gi_relight.y>0.5 {albedo=srgb_to_linear(albedo)}
            let n=normalize(self.gi_unpack(hit.z)*2.0-vec3(1.0,1.0,1.0))
            // Emission: 5-bit hue per channel + 8-bit intensity m/(1+m).
            // The voxel worker already weights radiance by emissive surface
            // coverage. Do not restore full material emission at this hit.
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
                // Preserve sun visibility: distance/window exhaustion is
                // unshadowed, even inside the scene (0.25). Hits and the
                // existing step-budget failure remain shadowed.
                if shadow.w<0.5 && shadow.w>(-0.5) {light=self.gi_sun_color*ndl}
            }
            light=light+self.local_bounce(p,n,self.gi_light_ids0.x)+self.local_bounce(p,n,self.gi_light_ids0.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids0.z)+self.local_bounce(p,n,self.gi_light_ids0.w)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.x)+self.local_bounce(p,n,self.gi_light_ids1.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.z)+self.local_bounce(p,n,self.gi_light_ids1.w)
            // Further bounces: last frame's field AT THE HIT, no recursion,
            // its open share under the rig's hemisphere.
            if self.gi_relight.x>0.0 {
                let hemisphere=mix(self.gi_ground,self.gi_sky,clamp(n.y*0.5+0.5,0.0,1.0))
                light=light+min(self.gi_ambient(p,n,hemisphere),vec3(64.0,64.0,64.0))*self.gi_relight.x
            }
            return vec4(min(albedo*light*self.gi_bounce+emission,vec3(64.0,64.0,64.0)),hit.x)
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
        radiance: fn(r:float,target_y:float)->vec4 {return self.gi_radiance.sample_nearest(vec2((r+0.5)/64.0,target_y))}
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
        // Around d, cosine-weighted: the radiance the surfaces send (E/pi)
        // and the share of rays that left the scene. w < 0 (including
        // ran out) contributes to neither sum nor the normalization.
        irradiance: fn(d:vec3,target_y:float,a:vec4)->vec4 {
            let axis=self.gi_ray_axis(d,a)
            var sum=vec3(0.0,0.0,0.0)
            var open=0.0
            var total=0.0
            var r=0.0
            loop {
                if r>63.5 {break}
                let sample=self.radiance(r,target_y)
                if sample.w>=0.0 {
                    let w=max(dot(axis,self.gi_fib(r)),0.0)
                    if sample.x<0.0 {open=open+w} else {sum=sum+sample.xyz*w}
                    total=total+w
                }
                r=r+1.0
            }
            if total<=0.0001 {return vec4(0.0,0.0,0.0,-1.0)}
            return vec4(sum/total,open/total)
        }
        // Filtered (E[d], E[d^2]) around d from this update's valid rays;
        // ran-out rays have w = -2 and carry no distance or weight.
        moments: fn(d:vec3,target_y:float,a:vec4,s:float)->vec4 {
            let axis=self.gi_ray_axis(d,a)
            var m=vec2(0.0,0.0)
            var total=0.0
            var r=0.0
            loop {
                if r>63.5 {break}
                let sample=self.radiance(r,target_y)
                if sample.w>=0.0 {
                    let c=max(dot(axis,self.gi_fib(r)),0.0)
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
            let target_y=self.target_y(row)
            let probe=self.gi_hits.sample_nearest(vec2(64.5/65.0,target_y))
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
                    let w=self.radiance(r,target_y).w
                    if w<(-0.5) && w>(-1.5) {back=back+1.0}
                    r=r+1.0
                }
                // Mostly backfaces: the probe sits inside closed geometry
                // (below terrain, inside a wall box). Invalid / ran-out
                // samples (w = -2) do not count against the probe.
                if probe.w<0.0 || back>16.0 {return vec4(0.0,0.0,0.0,-1.0)}
                var state=0.34
                if history {state=min(old_info.w+0.34,1.0)}
                return vec4(offset,state)
            }
            if col<63.5 {
                let x=col-floor(col/8.0)*8.0
                let y=floor(col/8.0)
                let old=self.gi_field.sample_nearest((self.gi_tile(k,a.x,0.0)+vec2(x+0.5,y+0.5))/self.gi_atlas.xy)
                let v=self.irradiance(self.tile_dir(x,y,6.0),target_y,a)
                if v.w<0.0 {
                    if h>0.0 {return old}
                    return vec4(0.0,0.0,0.0,0.0)
                }
                return mix(v,old,h)
            }
            let c=col-64.0
            let x=c-floor(c/10.0)*10.0
            let y=floor(c/10.0)
            let old=self.gi_field.sample_nearest((self.gi_tile(k,a.x,1.0)+vec2(x+0.5,y+0.5))/self.gi_atlas.xy)
            let m=self.moments(self.tile_dir(x,y,8.0),target_y,a,s)
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
        ..GiBatchLayout,
        alpha_blend: false
        color_format: @Rgba16F
        gi_atlas: uniform(vec4(640.0,1824.0,64.0,32.0))
        // Immutable instances identify a batch row and tile region; only
        // the two-column batch texture changes from one frame to the next.
        vertex: fn() {
            var p=self.rect_pos
            if self.gi_tile.x>=0.0 {
                let a=self.gi_batch.sample_nearest(vec2(0.25,self.batch_y(self.gi_tile.x)),0.0)
                let x=a.x-floor(a.x/self.gi_atlas.z)*self.gi_atlas.z
                let y=floor(a.x/self.gi_atlas.z)
                let base=a.y*self.gi_atlas.w*19.0
                let w=self.gi_tile.z
                var offset=0.0
                if self.gi_tile.w>0.5 {offset=self.gi_atlas.w*8.0}
                if self.gi_tile.w>1.5 {offset=self.gi_atlas.w*18.0}
                p=vec2(x*w,base+offset+y*w)
            }
            // Scatter records bypass turtle alignment, which normally fills
            // DrawQuad.draw_clip. Its cached zero clip would collapse every
            // tile (including clears). These are atlas pixels, not widgets.
            self.pos=self.geom.pos
            let vertex=p+self.geom.pos*self.rect_size
            self.world=self.draw_list.view_transform*vec4(vertex.x,vertex.y,self.draw_depth+self.draw_call.zbias,1.0)
            self.vertex_pos=self.draw_pass.camera_projection*(self.draw_pass.camera_view*self.world)
        }
        gi_update: texture_2d(float)
        pixel: fn(){
            if self.gi_tile.x<0.0 {return vec4(0.0,0.0,0.0,0.0)}
            let w=self.gi_tile.z
            let col=self.gi_tile.y+min(floor(self.pos.x*w),w-1.0)+min(floor(self.pos.y*w),w-1.0)*w
            return self.gi_update.sample_nearest(vec2((col+0.5)/#(super::GATHER_WIDTH as f64),self.target_y(self.gi_tile.x)))
        }
        fragment: fn(){self.fb0=self.pixel()}
    }

    // Diagnostic cadence only. One pixel per cascade, two little-endian
    // u16 counts (valid RG, invalid BA); untraced is the remainder. This
    // fits the existing UNORM8 ticket readback without copying the atlas.
    mod.draw.DrawGiProbeCounts = set_type_default() do #(DrawGiProbeCounts::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        alpha_blend: false
        color_format: @Bgra8NoBlend
        gi_field: texture_2d(float)
        gi_atlas: uniform(vec4(640.0,1824.0,64.0,32.0))
        // probes per cascade, cascade count
        gi_count_size: uniform(vec2(2048.0,3.0))
        pixel: fn() {
            let k=min(floor(self.pos.x*self.gi_count_size.y),self.gi_count_size.y-1.0)
            var valid=0.0
            var invalid=0.0
            var slot=0.0
            loop {
                if slot>=self.gi_count_size.x {break}
                let x=slot-floor(slot/self.gi_atlas.z)*self.gi_atlas.z
                let y=k*self.gi_atlas.w*19.0+self.gi_atlas.w*18.0+floor(slot/self.gi_atlas.z)
                let state=self.gi_field.sample_nearest((vec2(x,y)+vec2(0.5,0.5))/self.gi_atlas.xy,0.0).w
                if state>0.0 {valid=valid+1.0}
                if state<0.0 {invalid=invalid+1.0}
                slot=slot+1.0
            }
            let high=floor(vec2(valid,invalid)/256.0)
            let low=vec2(valid,invalid)-high*256.0
            return vec4(low.x,high.x,low.y,high.y)/255.0
        }
        fragment: fn(){self.fb0=self.pixel()}
    }

    mod.draw.FastGiScreen = FastGiScreen
    mod.draw.FastGiSampling = FastGiSampling
}

// The screen fill spreads VolumeFogCamera, which registers after the scene shaders (they spread
// FastGiSampling), so the fill is its own step after volume_fog (`script_mod_steps`).
pub mod screen_fill {
    use super::*;
    script_mod! {
        use mod.prelude.widgets_internal.*

        mod.draw.DrawGiScreenFill = set_type_default() do #(DrawGiScreenFill::script_shader(vm)) {
            ..mod.draw.DrawQuad,
            ..mod.draw.FastGiSampling,
            ..mod.draw.VolumeFogCamera,
            alpha_blend: false
            color_format: @Rgba32F
            depth_tex: texture_2d(float)
            // The cache's size in texels: the normal's depth neighbours are one texel apart.
            gi_fill_ctl: uniform(vec2(0.0, 0.0))

            gi_fill_distance: fn(p: vec3) -> float {
                return length(p - self.u_eye.xyz)
            }
            gi_fill_position: fn(uv: vec2) -> vec4 {
                if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
                let raw = self.depth_tex.sample_nearest(uv).x
                if self.background(raw) > 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
                let p = self.unproject(uv, raw)
                return vec4(p, self.gi_fill_distance(p))
            }
            // SSR's depth normal: choose the smaller valid depth jump on each axis.
            gi_fill_normal: fn(uv: vec2, p: vec3, d: float) -> vec3 {
                let texel = vec2(1.0, 1.0) / self.gi_fill_ctl
                let l = self.gi_fill_position(uv - vec2(texel.x, 0.0))
                let r = self.gi_fill_position(uv + vec2(texel.x, 0.0))
                let t = self.gi_fill_position(uv - vec2(0.0, texel.y))
                let b = self.gi_fill_position(uv + vec2(0.0, texel.y))
                var dx = r.xyz - p
                var dy = b.xyz - p
                if r.w <= 0.0 || (l.w > 0.0 && abs(l.w - d) < abs(r.w - d)) { dx = p - l.xyz }
                if b.w <= 0.0 || (t.w > 0.0 && abs(t.w - d) < abs(b.w - d)) { dy = p - t.xyz }
                var n = cross(dy, dx)
                let incident = self.ray_dir(uv)
                if length(n) <= 0.000001 * length(dx) * length(dy) { n = incident * -1.0 }
                n = normalize(n)
                if dot(n, incident) > 0.0 { n = n * -1.0 }
                return n
            }
            pixel: fn() -> vec4 {
                let texel = floor(self.pos * self.rect_size)
                // Use the same texel-centre ray the receiver uses to reconstruct p.
                let uv = (texel + vec2(0.5, 0.5)) / self.rect_size
                let raw = self.depth_tex.sample_nearest(uv).x
                if self.background(raw) > 0.5 { return vec4(0.0, 0.0, 0.0, -1.0) }
                let p = self.unproject(uv, raw)
                let d = self.gi_fill_distance(p)
                let n = self.gi_fill_normal(uv, p, d)
                return self.gi_screen_value(p, n, d)
            }
            fragment: fn() { self.fb0 = self.pixel() }
        }
    }
}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiTrace {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiRelight {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiGather {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiScatter {#[deref] pub quad:DrawQuad, #[live(vec4(0.0,0.0,1.0,0.0))] pub gi_tile:Vec4f}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiProbeCounts {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiScreenFill {#[deref] pub quad:DrawQuad}
