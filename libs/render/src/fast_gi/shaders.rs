//! Incremental diffuse transport kernels. Not the progressive path tracer.
use makepad_draw::*;
script_mod! {
    use mod.prelude.widgets_internal.*
    let FastGiSampling = {
        gi_field: texture_2d(float)
        gi_transition: uniform(1.0)
        gi_previous_bank: uniform(1.0)
        gi_previous_origin: uniform(vec4(0.0,0.0,0.0,2.0))
        gi_previous_grid: uniform(vec4(10.0,6.0,10.0,600.0))
        gi_previous_blockers: uniform(0.0)
        gi_sample_origin: fn(previous:bool)->vec4 {if previous{return self.gi_previous_origin} return self.gi_origin}
        gi_sample_grid: fn(previous:bool)->vec4 {if previous{return self.gi_previous_grid} return self.gi_grid}
        gi_sample_blockers: fn(previous:bool)->float {if previous{return self.gi_previous_blockers} return self.gi_blocker_count}
        gi_on: uniform(0.0)
        gi_debug: uniform(0.0)
        gi_blocker_count: uniform(0.0)
        gi_origin: uniform(vec4(0.0,0.0,0.0,2.0))
        gi_grid: uniform(vec4(10.0,6.0,10.0,600.0))
        gi_fetch: fn(previous:bool,index:float,lane:float)->vec4 {
            let height=max(self.gi_sample_grid(previous).w,64.0)
            var bank=0.0
            if previous {bank=self.gi_previous_bank}
            return self.gi_field.sample_nearest(vec2((lane+0.5)/40.0,(index+0.5+bank*height)/(height*2.0)))
        }
        gi_segment_clear: fn(previous:bool,probe:vec3,receiver:vec3,blockers:vec4)->bool {
            if self.gi_sample_blockers(previous)<0.5 || blockers.x==(-1.0) {return true}
            var count=4.0
            if blockers.x<(-1.5) {count=self.gi_sample_blockers(previous)}
            var i=0.0
            while i<count {
                var id=blockers.x
                if i>0.5 {id=blockers.y}
                if i>1.5 {id=blockers.z}
                if i>2.5 {id=blockers.w}
                if blockers.x<(-1.5) {id=i}
                if id>=0.0 {
                    let a=self.gi_fetch(previous,id,37.0)
                    // Gather marks approximate actor bounds with zero rows.
                    // They are transport proxies, not proof of solid space.
                    if dot(a.xyz,a.xyz)>0.0 {
                    let b=self.gi_fetch(previous,id,38.0)
                    let c=self.gi_fetch(previous,id,39.0)
                    let p=vec3(dot(a,vec4(probe,1.0)),dot(b,vec4(probe,1.0)),dot(c,vec4(probe,1.0)))
                    let delta=receiver-probe
                    let d=vec3(dot(a.xyz,delta),dot(b.xyz,delta),dot(c.xyz,delta))
                    let inv=(step(vec3(0.0,0.0,0.0),d)*2.0-vec3(1.0,1.0,1.0))/max(abs(d),vec3(0.0000001,0.0000001,0.0000001))
                    let t0=(vec3(-1.0,-1.0,-1.0)-p)*inv
                    let t1=(vec3(1.0,1.0,1.0)-p)*inv
                    let lo=min(t0,t1)
                    let hi=max(t0,t1)
                    let near=max(max(lo.x,lo.y),max(lo.z,0.00001))
                    let far=min(min(hi.x,hi.y),min(hi.z,0.99999))
                    if near<far {return false}
                    }
                }
                i=i+1.0
            }
            return true
        }
        gi_wrap_oct: fn(cell:vec2)->vec2 {
            // Octahedral edges fold back onto the same edge with the other
            // coordinate mirrored. Clamping makes seams on the -Z hemisphere.
            var b=cell
            if b.x<0.0 {b=vec2(-1.0-b.x,7.0-b.y)}
            if b.x>7.0 {b=vec2(15.0-b.x,7.0-b.y)}
            if b.y<0.0 {b=vec2(7.0-b.x,-1.0-b.y)}
            if b.y>7.0 {b=vec2(7.0-b.x,15.0-b.y)}
            return b
        }
        gi_moment: fn(previous:bool,index:float,cell:vec2)->vec2 {
            let b=self.gi_wrap_oct(cell)
            let slot=b.x+b.y*8.0
            let moments=self.gi_fetch(previous,index,4.0+floor(slot*0.5))
            if slot-floor(slot*0.5)*2.0>0.5{return moments.zw}
            return moments.xy
        }
        gi_probe_info_field: fn(previous:bool,cell:vec3)->vec4 {
            let index=cell.x+self.gi_sample_grid(previous).x*(cell.y+self.gi_sample_grid(previous).y*cell.z)
            return self.gi_fetch(previous,index,3.0)
        }
        gi_probe_info: fn(cell:vec3)->vec4 {return self.gi_probe_info_field(false,cell)}
        gi_probe_eligible: fn(previous:bool,cell:vec3,receiver:vec3,blockers:vec4,w:float)->vec4 {
            let probe=self.gi_probe_info_field(previous,cell)
            if probe.w<0.5 || w<=0.0 {return vec4(probe.xyz,0.0)}
            if !self.gi_segment_clear(previous,probe.xyz,receiver,blockers) {return vec4(probe.xyz,0.0)}
            return probe
        }
        gi_moment_valid: fn(previous:bool,index:float,cell:vec2)->vec3 {
            let m=self.gi_moment(previous,index,cell)
            if m.x<0.0 {return vec3(0.0,0.0,0.0)}
            return vec3(m,1.0)
        }
        gi_probe: fn(previous:bool,cell:vec3,probe:vec4,wp:vec3,n:vec3,w:float)->vec4 {
            let index=cell.x+self.gi_sample_grid(previous).x*(cell.y+self.gi_sample_grid(previous).y*cell.z)
            if probe.w<0.5 || w<=0.0 {return vec4(0.0,0.0,0.0,0.0)}
            // Do not push the receiver past a nearby probe. A fixed normal
            // bias made an otherwise visible probe look behind the wall,
            // printing tiny dark dots at its projected grid position.
            let to_probe=probe.xyz-wp
            let probe_distance=length(to_probe)
            let bias=min(self.gi_sample_origin(previous).w*0.03,probe_distance*0.25)
            let delta=wp+n*bias-probe.xyz
            let distance=length(delta)
            let direction=delta/max(distance,0.00001)
            var oct=direction.xy/max(abs(direction.x)+abs(direction.y)+abs(direction.z),0.00001)
            if direction.z<0.0 {oct=(vec2(1.0,1.0)-abs(oct.yx))*(step(vec2(0.0,0.0),oct)*2.0-vec2(1.0,1.0))}
            let bin=(oct*0.5+vec2(0.5,0.5))*8.0-vec2(0.5,0.5)
            let b=floor(bin)
            let f=fract(bin)
            let filtered=mix(mix(self.gi_moment_valid(previous,index,b),self.gi_moment_valid(previous,index,b+vec2(1.0,0.0)),f.x),mix(self.gi_moment_valid(previous,index,b+vec2(0.0,1.0)),self.gi_moment_valid(previous,index,b+vec2(1.0,1.0)),f.x),f.y)
            if filtered.z<=0.000001 {return vec4(0.0,0.0,0.0,0.0)}
            let moments=filtered.xy/filtered.z
            let mean=moments.x
            let second=moments.y
            let difference=max(distance-mean-self.gi_sample_origin(previous).w*0.05,0.0)
            let variance=max(second-mean*mean,0.0001)
            let visibility=variance/(variance+difference*difference)
            // Facing uses the UNBIASED receiver. At a probe almost on a
            // wall, normalizing a tiny vector creates a point-sized weight
            // spike. Bound that angular variation to a sub-cell footprint.
            let facing=pow(clamp(dot(n,to_probe)/max(probe_distance,self.gi_sample_origin(previous).w*0.25)*0.5+0.5,0.0,1.0),2.0)
            let weight=w*visibility*visibility*max(facing,0.005)
            let basis=vec4(1.0,n)
            var light=max(vec3(dot(self.gi_fetch(previous,index,0.0),basis),dot(self.gi_fetch(previous,index,1.0),basis),dot(self.gi_fetch(previous,index,2.0),basis)),vec3(0.0,0.0,0.0))
            if self.gi_debug>2.5 && self.gi_debug<3.5 {light=fract(sin(vec3(index+1.0,index+17.0,index+43.0))*43758.5453)}
            return vec4(light*weight,weight)
        }
        gi_ambient_field: fn(previous:bool,wp:vec3,normal:vec3,fallback:vec3)->vec3 {
            if self.gi_on<=0.0{return fallback}
            let n=normalize(normal)
            // Interpolate on the AIR side of a receiver, not arbitrarily
            // close to a disabled probe inside the solid. Visibility still
            // tests the actual receiver below, without this larger offset.
            let local=(wp+n*(self.gi_sample_origin(previous).w*0.1)-self.gi_sample_origin(previous).xyz)/self.gi_sample_origin(previous).w
            let border=min(local,self.gi_sample_grid(previous).xyz-vec3(1.0,1.0,1.0)-local)
            let edge=min(min(border.x,border.y),border.z)
            if edge<=0.0{return fallback}
            let base=floor(local)
            var blockers=vec4(-1.0,-1.0,-1.0,-1.0)
            if self.gi_sample_blockers(previous)>0.5 {blockers=self.gi_fetch(previous,base.x+self.gi_sample_grid(previous).x*(base.y+self.gi_sample_grid(previous).y*base.z),36.0)}
            let f=fract(local)
            // Trilinear weights remain smooth as visible probes enter/leave
            // a cell. Four-probe tetrahedra exposed their diagonal boundaries
            // once directional visibility removed individual corners.
            let t=vec3(1.0,1.0,1.0)-f
            let w0=t.x*t.y*t.z
            let w1=f.x*t.y*t.z
            let w2=t.x*f.y*t.z
            let w3=f.x*f.y*t.z
            let w4=t.x*t.y*f.z
            let w5=f.x*t.y*f.z
            let w6=t.x*f.y*f.z
            let w7=f.x*f.y*f.z
            // Gate each probe once, before both irradiance and confidence.
            // A valid exterior/below-floor probe is still unavailable to this
            // receiver; counting its nominal weight darkens room corners.
            let receiver=wp+n*(self.gi_sample_origin(previous).w*0.001)
            let p0=self.gi_probe_eligible(previous,base,receiver,blockers,w0)
            let p1=self.gi_probe_eligible(previous,base+vec3(1.0,0.0,0.0),receiver,blockers,w1)
            let p2=self.gi_probe_eligible(previous,base+vec3(0.0,1.0,0.0),receiver,blockers,w2)
            let p3=self.gi_probe_eligible(previous,base+vec3(1.0,1.0,0.0),receiver,blockers,w3)
            let p4=self.gi_probe_eligible(previous,base+vec3(0.0,0.0,1.0),receiver,blockers,w4)
            let p5=self.gi_probe_eligible(previous,base+vec3(1.0,0.0,1.0),receiver,blockers,w5)
            let p6=self.gi_probe_eligible(previous,base+vec3(0.0,1.0,1.0),receiver,blockers,w6)
            let p7=self.gi_probe_eligible(previous,base+vec3(1.0,1.0,1.0),receiver,blockers,w7)
            let sum=self.gi_probe(previous,base,p0,wp,n,w0)+self.gi_probe(previous,base+vec3(1.0,0.0,0.0),p1,wp,n,w1)
                +self.gi_probe(previous,base+vec3(0.0,1.0,0.0),p2,wp,n,w2)+self.gi_probe(previous,base+vec3(1.0,1.0,0.0),p3,wp,n,w3)
                +self.gi_probe(previous,base+vec3(0.0,0.0,1.0),p4,wp,n,w4)+self.gi_probe(previous,base+vec3(1.0,0.0,1.0),p5,wp,n,w5)
                +self.gi_probe(previous,base+vec3(0.0,1.0,1.0),p6,wp,n,w6)+self.gi_probe(previous,base+vec3(1.0,1.0,1.0),p7,wp,n,w7)
            // Confidence retains moment visibility and facing relative to
            // eligible nominal support, not relative to final weights.
            // Reuse gated records: no extra fetches or segment tests.
            let available=step(0.5,p0.w)*w0+step(0.5,p1.w)*w1+step(0.5,p2.w)*w2+step(0.5,p3.w)*w3+step(0.5,p4.w)*w4+step(0.5,p5.w)*w5+step(0.5,p6.w)*w6+step(0.5,p7.w)*w7
            let confidence=smoothstep(0.0,0.025,sum.w/max(available,0.00000001))
            if self.gi_debug>1.5 && self.gi_debug<2.5 {return vec3(confidence,confidence,confidence)}
            // A hidden/missing field is not evidence of unoccluded sky.
            // Keep hemisphere fallback at the volume edge/Off only; interior
            // uncertainty must not inject energy (also used by feedback).
            return mix(fallback,sum.xyz/max(sum.w,0.00000001)*confidence,clamp(edge,0.0,1.0)*min(self.gi_on,1.0))
        }
        // Keep the old world-space field visible during placement and
        // warmup; blend only once the replacement has converged. Producers
        // bind transition=1, so old display data never enters GI feedback.
        gi_ambient: fn(wp:vec3,normal:vec3,fallback:vec3)->vec3 {
            if self.gi_on<=0.0 {return fallback}
            if self.gi_transition>=1.0 {return self.gi_ambient_field(false,wp,normal,fallback)}
            let old=self.gi_ambient_field(true,wp,normal,fallback)
            if self.gi_transition<=0.0 {return old}
            return mix(old,self.gi_ambient_field(false,wp,normal,fallback),self.gi_transition)
        }
        // Last operation before writing DISPLAY colour into an 8-bit scene
        // target. Never applied to probe/hit payloads. One quantization step
        // of zero-mean noise breaks broad dark gradient contours; world-space
        // seed is stable between frames/eyes, with no time or history input.
        gi_display: fn(color:vec4,wp:vec3,n:vec3)->vec4 {
            if self.gi_on<=0.0 || color.w<0.999 {return color}
            if self.gi_debug>0.5 {
                let local=(wp-self.gi_origin.xyz)/self.gi_origin.w
                if self.gi_debug<1.5 {return vec4(fract(floor(local)*vec3(0.37,0.57,0.73)),1.0)}
                if self.gi_debug>4.5 {
                    // Raw irradiance uses the largest nominal trilinear
                    // weight, matching the sampler's air-side cell selection.
                    // No visibility, facing, interpolation or confidence:
                    // disagreements here already exist in the cached SH.
                    var sample_local=local
                    if self.gi_debug>5.5 {sample_local=local+normalize(n)*0.1}
                    let cell=clamp(floor(sample_local+vec3(0.5,0.5,0.5)),vec3(0.0,0.0,0.0),self.gi_grid.xyz-vec3(1.0,1.0,1.0))
                    let state=self.gi_probe_info(cell).w
                    if state<(-1.5) {return vec4(1.0,0.0,1.0,1.0)}
                    if state<0.0 {return vec4(1.0,0.1,0.0,1.0)}
                    if state<0.5 {return vec4(0.1,0.1,0.1,1.0)}
                    if self.gi_debug>5.5 {
                        let index=cell.x+self.gi_grid.x*(cell.y+self.gi_grid.y*cell.z)
                        let basis=vec4(1.0,normalize(n))
                        let light=vec3(dot(self.gi_fetch(false,index,0.0),basis),dot(self.gi_fetch(false,index,1.0),basis),dot(self.gi_fetch(false,index,2.0),basis))
                        return vec4(max(light,vec3(0.0,0.0,0.0)),1.0)
                    }
                    return vec4(0.0,0.8,0.1,1.0)
                }
                return vec4(self.gi_ambient(wp,n,vec3(0.0,0.0,0.0)),1.0)
            }
            let noise=fract(sin(dot(wp,vec3(127.1,311.7,74.7)))*43758.5453)-0.5
            return vec4(max(color.xyz+vec3(noise,noise,noise)*(1.0/255.0),vec3(0.0,0.0,0.0)),color.w)
        }
    }
    let ProbeLayout = {
        gi_positions: texture_2d(float)
        gi_origin: uniform(vec4(0.0,0.0,0.0,2.0))
        gi_grid: uniform(vec4(10.0,6.0,10.0,600.0))
        gi_batch: uniform(vec4(0.0,64.0,32.0,256.0))
        // selected sources, position-row texels, ray-cache width, reserved.
        // Producer-only data; the forward field and its sampler stay unchanged.
        gi_emitters: uniform(vec4(0.0,1.0,64.0,0.0))
        position_data: fn(index:float,lane:float)->vec4 {
            return self.gi_positions.sample_nearest(vec2((lane+0.5)/self.gi_emitters.y,(index+0.5)/self.gi_grid.w))
        }
        probe_pos: fn(index: float)->vec3 {
            return self.position_data(index,0.0).xyz
        }
        emitter_ray: fn(index:float,target:float)->vec4 {
            return self.position_data(index,1.0+self.gi_emitters.x+target*2.0)
        }
        emitter_sh: fn(index:float,target:float)->vec4 {
            return self.position_data(index,2.0+self.gi_emitters.x+target*2.0)
        }
        emitter_sampled: fn(index:float,id:float)->bool {
            if id<=0.0 {return false}
            var e=0.0
            while e<self.gi_emitters.x {
                if abs(self.position_data(index,1.0+e).w-id)<0.5 {return true}
                e=e+1.0
            }
            return false
        }
        // One sample per 8x8 octahedral visibility bin. Bilinear moment
        // reconstruction combines four neighboring directions. This avoids
        // the enormous angular cones and false occlusion of the 4x4 field.
        ray_dir: fn(index:float)->vec4 {
            let x=index-floor(index/8.0)*8.0
            let y=floor(index/8.0)
            var f=(vec2(x,y)+vec2(0.5,0.5))*0.25-vec2(1.0,1.0)
            var d=vec3(f.x,f.y,1.0-abs(f.x)-abs(f.y))
            if d.z<0.0 {d=vec3((1.0-abs(f.y))*sign(f.x),(1.0-abs(f.x))*sign(f.y),d.z)}
            let len=length(d)
            return vec4(d/len,1.0/(len*len*len))
        }
    }
    let Geometry = {
        gi_nodes: texture_2d(float)
        gi_triangles: texture_2d(float)
        gi_scene: uniform(vec4(1.0,1.0,0.0,0.0))
        node: fn(at:float)->vec4 {
            let y=floor(at/256.0)
            return self.gi_nodes.sample_nearest(vec2((at-y*256.0+0.5)/256.0,(y+0.5)/self.gi_scene.x))
        }
        tri: fn(at:float)->vec4 {
            let y=floor(at/256.0)
            return self.gi_triangles.sample_nearest(vec2((at-y*256.0+0.5)/256.0,(y+0.5)/self.gi_scene.y))
        }
        reciprocal: fn(d:vec3)->vec3 {
            return (step(vec3(0.0,0.0,0.0),d)*2.0-vec3(1.0,1.0,1.0))/max(abs(d),vec3(0.0000001,0.0000001,0.0000001))
        }
        // x distance, y triangle+1 (0 miss, -1 exhausted), z backface.
        trace_static: fn(ro:vec3,rd:vec3,limit:float)->vec3 {
            let inv=self.reciprocal(rd)
            var result=vec3(limit,0.0,0.0)
            var node=0.0
            var steps=0.0
            while node<self.gi_scene.z && steps<self.gi_batch.w {
                steps=steps+1.0
                let a=self.node(node*2.0)
                let b=self.node(node*2.0+1.0)
                let v0=(a.xyz-ro)*inv
                let v1=(b.xyz-ro)*inv
                let low=min(v0,v1)
                let high=max(v0,v1)
                let near=max(max(low.x,low.y),max(low.z,0.0001))
                let far=min(min(high.x,high.y),min(high.z,result.x))*1.000001
                if near<=far {
                    if a.w<0.0 {
                        var k=0.0
                        while k<min(0.0-a.w,8.0) {
                            let index=b.w+k
                            let p=self.tri(index*5.0).xyz
                            let e1=self.tri(index*5.0+1.0).xyz-p
                            let e2=self.tri(index*5.0+2.0).xyz-p
                            let h=cross(rd,e2)
                            let det=dot(e1,h)
                            if abs(det)>0.00000001 {
                                let q=ro-p
                                let u=dot(q,h)/det
                                let r=cross(q,e1)
                                let v=dot(rd,r)/det
                                let t=dot(e2,r)/det
                                if u>=-0.000001 && v>=-0.000001 && u+v<=1.000001 && t>0.001 && t<result.x {
                                    result=vec3(t,index+1.0,step(det,0.0))
                                }
                            }
                            k=k+1.0
                        }
                    }
                    node=node+1.0
                } else {
                    if a.w<0.0 {node=node+1.0}else{node=b.w}
                }
            }
            if node<self.gi_scene.z {return vec3(0.0,-1.0,0.0)}
            return result
        }
    }
    mod.draw.DrawGiTrace = set_type_default() do #(DrawGiTrace::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..ProbeLayout,
        ..Geometry,
        alpha_blend: false
        color_format: @Rgba32F
        pixel: fn(){
            let index=floor(self.pos.y*self.gi_batch.y)+self.gi_batch.x
            let ray=min(floor(self.pos.x*self.gi_emitters.z),self.gi_emitters.z-1.0)
            if ray>=64.0 {
                let target=self.emitter_ray(index,ray-64.0)
                if target.w<=0.002 {return vec4(0.0,0.0,0.0,0.0)}
                // Endpoint is on the emitter. Exclude only that endpoint;
                // blockers and traversal exhaustion never become visible sky.
                let hit=self.trace_static(self.probe_pos(index),target.xyz,target.w-0.002)
                return vec4(hit,1.0)
            }
            let state=self.position_data(index,0.0).w
            if state<(-1.5) {return vec4(0.0,-1.0,0.0,1.0)}
            if state<0.0 {return vec4(0.0,0.0,1.0,1.0)}
            let hit=self.trace_static(self.probe_pos(index),self.ray_dir(ray).xyz,self.gi_batch.z)
            return vec4(hit,1.0)
        }
        fragment: fn(){self.fb0=self.pixel()}
    }
    let MoverData = {
        gi_movers: texture_2d(float)
        gi_mover_height: uniform(1.0)
        mover: fn(at:float)->vec4 {
            let y=floor(at/256.0)
            return self.gi_movers.sample_nearest(vec2((at-y*256.0+0.5)/256.0,(y+0.5)/self.gi_mover_height))
        }
    }
    mod.draw.DrawGiRelight = set_type_default() do #(DrawGiRelight::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..FastGiSampling,
        ..ProbeLayout,
        ..Geometry,
        ..mod.draw.ClusteredLighting,
        ..MoverData,
        alpha_blend: false
        color_format: @Rgba32F
        gi_hits: texture_2d(float)
        gi_sun_map: texture_2d(float)
        gi_sun_rows: texture_2d(float)
        gi_sun_dir: uniform(vec3(0.0,1.0,0.0))
        gi_sun_color: uniform(vec3(0.0,0.0,0.0))
        gi_sky: uniform(vec3(0.1,0.1,0.1))
        gi_ground: uniform(vec3(0.1,0.1,0.1))
        gi_counts: uniform(vec4(0.0,0.0,0.0,0.0))
        gi_light_ids0: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        gi_light_ids1: uniform(vec4(-1.0,-1.0,-1.0,-1.0))
        sun_row: fn(at:float)->vec4 {return self.gi_sun_rows.sample_nearest(vec2((at+0.5)/16.0,0.5))}
        sun_visibility: fn(p:vec3,n:vec3)->float {
            var i=0.0
            while i<self.gi_counts.z {
                let a=self.sun_row(i*4.0)
                let b=self.sun_row(i*4.0+1.0)
                let c=self.sun_row(i*4.0+2.0)
                let bias=self.sun_row(i*4.0+3.0)
                let wp=vec4(p+n*max(bias.y,0.01),1.0)
                let q=vec3(dot(a,wp),dot(b,wp),dot(c,wp))
                if abs(q.x)<0.98 && abs(q.y)<0.98 && q.z>0.0 && q.z<1.0 {
                    let uv=vec2((q.x*0.5+0.5+i)/3.0,0.5-q.y*0.5)
                    return step(q.z-bias.x*2.0,self.gi_sun_map.sample_nearest(uv).x)
                }
                i=i+1.0
            }
            // Outside known shadow coverage: no invented sunlight indoors.
            return 0.0
        }
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
        emitter_radiance: fn(index:float,target_index:float,hit:vec4)->vec4 {
            let target=self.emitter_ray(index,target_index)
            if target.w<=0.002 || hit.y!=0.0 || hit.w<0.5 {return vec4(0.0,0.0,0.0,0.0)}
            let ro=self.probe_pos(index)
            var i=0.0
            while i<self.gi_counts.x {
                let a=self.mover(i*6.0)
                let b=self.mover(i*6.0+1.0)
                let c=self.mover(i*6.0+2.0)
                let p=vec3(dot(a,vec4(ro,1.0)),dot(b,vec4(ro,1.0)),dot(c,vec4(ro,1.0)))
                // An approximate actor bound can contain a perfectly valid
                // air probe. Do not suppress all emitter samples from it.
                if max(max(abs(p.x),abs(p.y)),abs(p.z))>=0.999 || self.mover(i*6.0+3.0).w>0.5 {
                let d=vec3(dot(a.xyz,target.xyz),dot(b.xyz,target.xyz),dot(c.xyz,target.xyz))
                let inv=self.reciprocal(d)
                let t0=(vec3(-1.0,-1.0,-1.0)-p)*inv
                let t1=(vec3(1.0,1.0,1.0)-p)*inv
                let lo=min(t0,t1)
                let hi=max(t0,t1)
                let near=max(max(lo.x,lo.y),max(lo.z,0.001))
                let far=min(min(hi.x,hi.y),min(hi.z,target.w-0.002))
                if near<=far {return vec4(0.0,0.0,0.0,0.0)}
                }
                i=i+1.0
            }
            return vec4(self.position_data(index,1.0+floor(target_index/12.0)).xyz,1.0)
        }
        pixel: fn(){
            let index=floor(self.pos.y*self.gi_batch.y)+self.gi_batch.x
            let ray=min(floor(self.pos.x*self.gi_emitters.z),self.gi_emitters.z-1.0)
            let hit=self.gi_hits.sample_nearest(vec2((ray+0.5)/self.gi_emitters.z,(index+0.5)/self.gi_grid.w))
            if ray>=64.0 {return self.emitter_radiance(index,ray-64.0,hit)}
            let rd=self.ray_dir(ray).xyz
            let ro=self.probe_pos(index)
            if hit.y<0.0 {return vec4(0.0,0.0,0.0,-2.0)}
            if hit.z>0.5 {return vec4(0.0,0.0,0.0,-1.0)}
            var distance=hit.x
            var n=vec3(0.0,1.0,0.0)
            var albedo=vec3(0.0,0.0,0.0)
            var emission=vec3(0.0,0.0,0.0)
            var found=hit.y>0.5
            if found {
                let at=(hit.y-1.0)*5.0
                let p=self.tri(at).xyz
                n=normalize(cross(self.tri(at+1.0).xyz-p,self.tri(at+2.0).xyz-p))
                albedo=self.tri(at+3.0).xyz
                let source=self.tri(at+4.0)
                emission=source.xyz
                // Split ONLY emission, not reflected light. A zero per-probe
                // header means range/geometry fallback to the original rays.
                if self.emitter_sampled(index,source.w) {emission=vec3(0.0,0.0,0.0)}
            }
            // Small oriented proxies for movers; never rebuild static BVH
            // merely because a door/car moved. Only an exact solid box can
            // prove a probe is inside geometry; mesh bounds include air.
            var i=0.0
            while i<self.gi_counts.x {
                let a=self.mover(i*6.0)
                let b=self.mover(i*6.0+1.0)
                let c=self.mover(i*6.0+2.0)
                let p=vec3(dot(a,vec4(ro,1.0)),dot(b,vec4(ro,1.0)),dot(c,vec4(ro,1.0)))
                if max(max(abs(p.x),abs(p.y)),abs(p.z))<0.999 && self.mover(i*6.0+3.0).w>0.5 {return vec4(0.0,0.0,0.0,-1.0)}
                let d=vec3(dot(a.xyz,rd),dot(b.xyz,rd),dot(c.xyz,rd))
                let inv=self.reciprocal(d)
                let t0=(vec3(-1.0,-1.0,-1.0)-p)*inv
                let t1=(vec3(1.0,1.0,1.0)-p)*inv
                let lo=min(t0,t1)
                let hi=max(t0,t1)
                let near=max(max(lo.x,lo.y),lo.z)
                let far=min(min(hi.x,hi.y),hi.z)
                if near>0.001 && near<=far && near<distance {
                    distance=near
                    n=normalize(a.xyz)*(0.0-sign(d.x))
                    if lo.y>=lo.x && lo.y>=lo.z {n=normalize(b.xyz)*(0.0-sign(d.y))}
                    if lo.z>=lo.x && lo.z>=lo.y {n=normalize(c.xyz)*(0.0-sign(d.z))}
                    albedo=self.mover(i*6.0+3.0).xyz
                    emission=self.mover(i*6.0+4.0).xyz
                    found=true
                }
                i=i+1.0
            }
            if !found {return vec4(mix(self.gi_ground,self.gi_sky,clamp(rd.y*0.5+0.5,0.0,1.0)),self.gi_batch.z)}
            let p=ro+rd*distance
            var light=self.gi_sun_color*(max(dot(n,self.gi_sun_dir),0.0)*self.sun_visibility(p,n))
            light=light+self.local_bounce(p,n,self.gi_light_ids0.x)+self.local_bounce(p,n,self.gi_light_ids0.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids0.z)+self.local_bounce(p,n,self.gi_light_ids0.w)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.x)+self.local_bounce(p,n,self.gi_light_ids1.y)
            light=light+self.local_bounce(p,n,self.gi_light_ids1.z)+self.local_bounce(p,n,self.gi_light_ids1.w)
            // Reuse previous-frame irradiance at the HIT, not at the probe.
            // Subsequent updates propagate further diffuse bounces without
            // tracing recursive rays. Missing/invalid history adds no light.
            if self.gi_counts.w>0.0 {
                light=light+min(self.gi_ambient(p,n,vec3(0.0,0.0,0.0)),vec3(8.0,8.0,8.0))*self.gi_counts.w
            }
            // Legacy game light units already fold the Lambert normalization.
            return vec4(min(clamp(albedo,vec3(0.0,0.0,0.0),vec3(0.9,0.9,0.9))*light+emission,vec3(8.0,8.0,8.0)),distance)
        }
        fragment: fn(){self.fb0=self.pixel()}
    }
    mod.draw.DrawGiGather = set_type_default() do #(DrawGiGather::script_shader(vm)) {
        ..mod.draw.DrawQuad,
        ..FastGiSampling,
        ..ProbeLayout,
        ..MoverData,
        alpha_blend: false
        color_format: @Rgba32F
        gi_radiance: texture_2d(float)
        ray: fn(index:float,r:float)->vec4{return self.gi_radiance.sample_nearest(vec2((r+0.5)/self.gi_emitters.z,(index+0.5)/self.gi_grid.w))}
        distance_moments: fn(index:float,r:float)->vec2 {
            // Prefilter the existing rays, once per updated probe, rather
            // than treating each angular bin as a zero-variance point hit.
            // The latter projected octahedral wedges onto nearby walls.
            let center=self.ray_dir(r).xyz
            let cell=vec2(r-floor(r/8.0)*8.0,floor(r/8.0))
            var moments=vec2(0.0,0.0)
            var total=0.0
            var y=-1.0
            while y<=1.0 {
                var x=-1.0
                while x<=1.0 {
                    let b=self.gi_wrap_oct(cell+vec2(x,y))
                    let sample=b.x+b.y*8.0
                    let direction=self.ray_dir(sample)
                    let cosine=max(dot(center,direction.xyz),0.0)
                    let c2=cosine*cosine
                    let c4=c2*c2
                    let c8=c4*c4
                    let weight=c8*c8*direction.w
                    // Only local-cell visibility is queried. Distant sky
                    // hits must not inflate variance and leak through walls.
                    let raw_distance=self.ray(index,sample).w
                    // Invalid is NOT a zero-distance hit. Exclude its
                    // statistical weight as well as its radiance.
                    if raw_distance>=0.0 {
                        let distance=min(raw_distance,self.gi_origin.w*2.0)
                        moments=moments+vec2(distance,distance*distance)*weight
                        total=total+weight
                    }
                    x=x+1.0
                }
                y=y+1.0
            }
            // Explicit no-data marker, excluded during bilinear lookup too.
            if total<=0.000001 {return vec2(-1.0,0.0)}
            return moments/max(total,0.000001)
        }
        gi_keep_previous: uniform(0.0)
        gi_copy_previous: uniform(0.0)
        pixel: fn(){
            let height=max(self.gi_grid.w,64.0)
            let index=min(floor(self.pos.y*height*2.0),height*2.0-1.0)
            let lane=min(floor(self.pos.x*40.0),39.0)
            // The second bank holds the last complete world-space field.
            // First scroll frame copies bank zero; later frames retain bank
            // one. Scene shaders still bind only this single field texture.
            if index>=height {
                if self.gi_keep_previous<0.5 {return vec4(0.0,0.0,0.0,0.0)}
                var source=index
                if self.gi_copy_previous>0.5 {source=index-height}
                return self.gi_field.sample_nearest(vec2((lane+0.5)/40.0,(source+0.5)/(height*2.0)))
            }
            // Update blocker topology every frame, including untouched probe
            // rows. Geometry may move before a lighting sweep reaches a cell.
            if lane>=37.0 {
                // Zero rows are a non-blocking marker for approximate mesh
                // bounds. Preserve their actual rows in the relight bank.
                if index<self.gi_blocker_count && self.mover(index*6.0+3.0).w>0.5 {return self.mover(index*6.0+lane-37.0)}
                return vec4(0.0,0.0,0.0,0.0)
            }
            if lane>=36.0 {
                if index<self.gi_grid.w {return self.mover(384.0+index)}
                return vec4(-1.0,-1.0,-1.0,-1.0)
            }
            if index>=self.gi_grid.w {return vec4(0.0,0.0,0.0,0.0)}
            if index<self.gi_batch.x || index>=self.gi_batch.x+self.gi_batch.y {
                if self.gi_on<=0.0 {return vec4(0.0,0.0,0.0,0.0)}
                return self.gi_fetch(false,index,lane)
            }
            if lane>=4.0 {
                // Two filtered bins per texel: (E[d], E[d²]), NOT E[d]².
                let r=(lane-4.0)*2.0
                return vec4(self.distance_moments(index,r),self.distance_moments(index,r+1.0))
            }
            var coefficients=vec4(0.0,0.0,0.0,0.0)
            var weight=0.0
            var invalid=0.0
            var exhausted=0.0
            var r=0.0
            while r<64.0 {
                let sample=self.ray(index,r)
                let direction=self.ray_dir(r)
                var value=sample.x
                if lane>0.5 {value=sample.y}
                if lane>1.5 {value=sample.z}
                if sample.w<(-1.5) {exhausted=exhausted+1.0}
                if sample.w<0.0 {invalid=invalid+1.0} else {
                    coefficients=coefficients+vec4(1.0,direction.xyz*2.0)*(value*direction.w)
                    weight=weight+direction.w
                }
                r=r+1.0
            }
            if lane>2.5 {
                if exhausted>0.0 {return vec4(self.probe_pos(index),-2.0)}
                if invalid>6.0 {return vec4(self.probe_pos(index),-1.0)}
                return vec4(self.probe_pos(index),1.0)
            }
            var result=coefficients/max(weight,0.0001)
            // Analytic full-sphere DC/L1 weights are already normalized.
            // These targeted visibility rays must NOT enter distance moments,
            // general-ray normalization, or solid/backface classification.
            var target=0.0
            while target<self.gi_emitters.x*12.0 {
                let sample=self.ray(index,64.0+target)
                var value=sample.x
                if lane>0.5 {value=sample.y}
                if lane>1.5 {value=sample.z}
                result=result+self.emitter_sh(index,target)*value
                target=target+1.0
            }
            // Smooth small changes only. Geometry/large lighting changes
            // replace history immediately, rather than leaving glowing trails.
            if self.gi_on>0.0 && invalid<=6.0 && exhausted<0.5 {
                if self.gi_fetch(false,index,3.0).w>0.5 {
                    let old=self.gi_fetch(false,index,lane)
                    let d=abs(old-result)
                    let change=max(max(d.x,d.y),max(d.z,d.w))/max(max(abs(old.x),abs(result.x)),0.02)
                    return mix(result,old,0.35*(1.0-smoothstep(0.1,0.3,change)))
                }
            }
            return result
        }
        fragment: fn(){self.fb0=self.pixel()}
    }

    mod.draw.FastGiSampling = FastGiSampling
}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiTrace {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiRelight {#[deref] pub quad:DrawQuad}
#[derive(Script,ScriptHook)] #[repr(C)] pub struct DrawGiGather {#[deref] pub quad:DrawQuad}
