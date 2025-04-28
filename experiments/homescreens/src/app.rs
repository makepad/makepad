use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;

    use makepad_experiments_homescreens::my_widget::MyWidget;
    use makepad_experiments_homescreens::iconbutton::IconButton;
    
    use makepad_experiments_homescreens::diffuse::DiffuseThing;
    use makepad_experiments_homescreens::particles::ParticleSystem;
    use makepad_experiments_homescreens::birds::BirdSystem;
    ContainerStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform shadowopacity: 0.5,
            uniform shadowx: 0.5,
            uniform shadowy: 0.5,
            uniform shadowcolor: #000,
            varying o0: vec2,
            varying oShadow: vec2,
            
            fn vertex(self) -> vec4 {                
                let dpi = self.dpi_factor;                               
                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                self.o0 = self.pos;
                self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi )*0.001;
                return pos;
            }

            fn pixel(self) -> vec4 {
                let shadow = sample2d(self.image, self.oShadow + vec2(cos(self.time*3.+self.o0.y*10.)*0.0013, cos(self.time+self.o0.x*100.)*0.0013));
                let main = sample2d(self.image, self.o0);
                let col =  (vec4(self.shadowcolor.xyz,self.shadowopacity)  * shadow.a ) * ( 1 - main.a) + main;
                return col;
            }
        }
    }

    ReflectorStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform shadowopacity:  0.2,
            uniform shadowx: 4.0,
            uniform shadowy: 4.0,
            uniform shadowcolor: vec3(0.01,0.01,0.02),
            varying o0: vec2,
            varying oShadow: vec2,
            
            fn vertex(self) -> vec4 {                
                let dpi = self.dpi_factor;                               
                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                self.o0 = self.pos;
                self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi )*0.001;
                return pos;
            }

            fn pixel(self) -> vec4 {
    
                let main = sample2d(self.image, self.o0);
                let uv = self.o0  - vec2(0.03,0.07);
                uv.y +=sin(uv.y*140.)*0.02+ (cos((uv.y + (self.time * 0.04)) * 45.0) * 0.0019) + (cos((uv.y + (self.time * 0.1)) * 10.0) * 0.002);
                uv.x += sin(uv.y*420.)*0.02+ (sin((uv.y + (self.time * 0.07)) * 15.0) * 0.0029) + (sin((uv.y + (self.time * 0.1)) * 15.0) * 0.002);
                let flect = sample2d(self.image, uv);
                let col =  vec4(flect.xyz*0.1, flect.w * 0.1) * ( 1 - main.a) + main;
                return col;
            }
        }
    }

    IconSet = <View> {
        width: Fill,
        height: Fill,
        flow: Down
        margin: {top: 25.} 
        spacing: 50
        padding: 5
        <View> {
            spacing: 5
            width: Fill,
            height: 200,
            align: {x:0., y:0.5}
            flow: Right,
            <IconButton>{button={text:"I am all"},width: Fill,image={source: dep("crate://self/resources/Icon1.png")}}
            <IconButton>{button={text:"Notificiations"},width: Fill,image={source: dep("crate://self/resources/Icon2.png")}}
            <IconButton>{button={text:"External Data"},width: Fill,image={source: dep("crate://self/resources/Icon3.png")}}
            <IconButton>{button={text:"Files"},width: Fill,image={source: dep("crate://self/resources/Icon4.png")}}
        }
        <View> {
            width: Fill,
            height: 200,
            flow: Right,
            
            <IconButton>{button={text:"Recycle Bin"},width: Fill,image={source: dep("crate://self/resources/Icon5.png")}}
            <IconButton>{button={text:"Utilities"},width: Fill,image={source: dep("crate://self/resources/Icon6.png")}}
            <IconButton>{button={text:"Food"},width: Fill,image={source: dep("crate://self/resources/Icon7.png")}}
            <IconButton>{button={text:"Zoo"},width: Fill,image={source: dep("crate://self/resources/Icon8.png")}}
                                
        }
        <View> { 
            width: Fill,
            height: 200,
            flow: Right,
         
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon9.png")}, button={text: "TheTube"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon11.png")}, button={text: "With Me"}}
            <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon12.png")}, button={text: "Passwords"}}
            <View> {}
        }
        // <View> {
        //     width: Fill,
        //     height: Fill,
        //     flow: Right,
        //     <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon13.png")}, button={ text: "Diwe"}}
        //     <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon14.png")}, button={ text: "Wubi"}}
        //     <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon15.png")}, button={text: "RideHyper"}}
        //     <IconButton>{width: Fill,image={source: dep("crate://self/resources/Icon16.png")}, button={text: "TrustyBank"}}
        // }
    
    }

    App = {{App}} {        
        ui: <Window>{
            
            window: {
                inner_size: vec2(640,1024)
            }
            show_bg: true
            width: Fill,
            height: Fill
            padding : 0,
            spacing : 0,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#7,#4,self.pos.y);
                }
            }

            body = <View>{
                width: Fill,
                height: Fill,
                flow: Down,
                padding: 0,
                spacing: 0,
                
                <View>{
                    width: Fill, height: 28, draw_bg:
                    {
                        fn pixel(self) ->  vec4{return #ff0;}
                    }
                }
                
                <Dock>{
                    width: Fill,
                    height: Fill,
                    padding: 0,
                    spacing: 0,

                    root = Tabs{tabs:[screen2tab, screen3tab, screen4tab, screen5tab, screen6tab], selected:4}

                    /*screen1tab = Tab{
                        name: "FloatTexture"
                        kind: screen1
                    }*/

                    screen2tab = Tab{
                        name: "Gradient"
                        kind: screen2
                    }

                    screen3tab = Tab{
                        name: "Wavy"
                        kind: screen3
                    }
                    screen4tab = Tab{
                        name: "Water"
                        kind: screen4
                    }
                    screen5tab = Tab{
                        name: "Fire"
                        kind: screen5
                    }
                    screen6tab = Tab{
                        name: "Drops"
                        kind: screen6
                    }
                    screen1 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                
                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                fn pixel(self) -> vec4 
                                {
                                
                                    let time = self.time * .015+23.0;
                                    let uv = self.pos*0.1;
                                    let p = mod(uv*6.283, 6.283)-250.0;
                                    let i = vec2(p);
                                    let c = 1.0;
                                    let inten = .005;
                                    let n = 0;
                                    for _n in 0..4 
                                    {
                                        let t = time * (1.0 - (3.5 / (float(n) +1.0)));
                                        i = p + vec2(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
                                        c += 1.0/length(vec2(p.x / (sin(i.x+t)/inten),p.y / (cos(i.y+t)/inten)));
                                        n = n + 1;
                                    }
                                    c /= float(5);
                                    c = 1.17-pow(c, 1.4);
                                    let colour = vec3(pow(abs(c), 8.0));
                                    colour = clamp(colour*.8 + vec3(0.70, 0.35, 0.5), 0.0, 1.0);
                                    return vec4(colour, 1.0);
                                }
                            }
                        }
                         <ContainerStage>{ 
                            flow: Overlay,  
                            <DiffuseThing>{width: Fill, height: Fill}
                            <IconSet> {}              
                         }
                
                        
                    }   
                    screen3 = <View> {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },

                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                fn pixel(self) -> vec4 {
                                
                                let fragColor = mix(#272001, #764423, self.pos.y);

                                    return fragColor;
                                }
                            }
                        }
                        <ReflectorStage>{   
                            <IconSet>{}
                        }
                    }
                    screen2 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                
                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            draw: {
                                // this example shader is ported from kishimisu's tutorial
                                fn pixel(self) -> vec4 {
                                

                                let time = self.time * .15+23.0;
                                let uv = self.pos;
                                
                                let p = mod(uv*6.283, 6.283)-250.0;
                                let i = vec2(p);
                                let c = 1.0;
                                let inten = .005;
                                let n = 0;
                                for _n in 0..4 
                                {
                                    let t = time * (1.0 - (3.5 / (float(n) +1.0)));
                                    i = p + vec2(cos(t - i.x) + sin(t + i.y), sin(t - i.y) + cos(t + i.x));
                                    c += 1.0/length(vec2(p.x / (sin(i.x+t)/inten),p.y / (cos(i.y+t)/inten)));
                                    n = n + 1;
                                }
                                c /= float(5);
                                c = 1.17-pow(c, 1.4);
                                let colour = vec3(pow(abs(c), 8.0));
                                colour = clamp(colour*.8 + vec3(0.0, 0.35, 0.5), 0.0, 1.0);

                                
                                let fragColor = vec4(colour, 1.0);



                                    //let finalColor = vec3(0.3+0.01*sin(uv.x*6.283*4));
                                    return fragColor;
                                }
                            }
                        }
                        <ContainerStage>{   
                            <IconSet> {}              
                         }
                
                        
                    }   

                
                    screen4 = <View> {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                        show_bg: true,
                        draw_bg: {
                            fn pixel(self) -> vec4{
                                return vec4(0.70,0.72,0.72,1)
                            }
                        }   

                        <Image>{
                            width: Fill;
                            height: Fill;	
                            source: dep("crate://self/resources/background.jpg")
                        }

                        <Image>{
                            width: Fill;
                            height: Fill;	
                            source: dep("crate://self/resources/water_mask.png")
                            draw_bg: {
                                
                                fn pixel(self) -> vec4{
                                    let col = sample2d(self.image, self.pos);
                                    //let s = sin(self.pos.y * 150.0 + sin(self.pos.x*100.)*10.0 ) *0.5 + 0.5;
                                    
                                    let q = 10.0;
                                    let r = 1000.0;

                                    let s2 = sin((self.pos.x * 100.0)/self.pos.y +self.time+ sin(self.time*0.3+self.pos.y*r + self.pos.x*0.2/self.pos.y)*q)*0.25 + 0.75;         
                                     let g = vec3(s2*0.9,s2*0.92,s2*0.95);
                                     let a = col.x * 0.3;

                                    return vec4(g*a, a);                                 
                                }
                            }
                        }

                        <ParticleSystem> {
                            width: Fill,
                            height: Fill,
                            maxparticles: 3000,
                            spawnrate: 30,
                            drop_width: 3,
                            drop_height: 60,
                            particletexture:{
                                 source: dep("crate://self/resources/drop.png")
                            }
                            
                        }       
                        <BirdSystem> {
                            width: Fill,
                            height: Fill,
                            max_birds: 100,
                            spawnrate: 20,
                            bird_width: 20,
                            bird_height: 20,
                            birdtexture:{
                                 source: dep("crate://self/resources/bird_combined.png")
                            }
                            
                        }        

                        <ContainerStage>{   
                            <IconSet> {}              
                         }  
                    }   
                    screen5 = <View>
                    {
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                
                        quad = <MyWidget> {
                            align:{x:0.,y:0.0}
                            width: Fill,
                            height: Fill,
                            
                            draw: {
                                
                                // ported from a creation by anatole duprat - XT95/2013
                                // License Creative Commons Attribution-NonCommercial-ShareAlike 3.0 Unported License.
                                // see https://www.shadertoy.com/view/MdX3zr
                                fn noise( p: vec3)-> float //Thx to Las^Mercury
                                {
                                    let  i = floor(p);
                                    let a=  dot(i, vec3(1., 57., 21.)) + vec4(0., 57., 21., 78.);
                                    let f = cos((p-i)*acos(-1.))*(-.5)+.5;
                                    a = mix(sin(cos(a)*a),sin(cos(1.+a)*(1.+a)), f.x);
                                    a.xy = mix(a.xz, a.yw, f.y);
                                    return mix(a.x, a.y, f.z);
                                }

                                fn sphere(p:vec3,  spr:vec4) -> float
                                {
                                    return length(spr.xyz-p) - spr.w;
                                }
                                
                                fn flame( p:vec3, time: float) ->float
                                {
                                    let d = sphere(p*vec3(1.,.5,1.), vec4(.0,-1.,.0,1.));
                                    return d + (noise(p+vec3(.0,time*2.,.0)) + noise(p*3.)*.5)*.25*(p.y) ;
                                }
                                
                                fn scene(p:vec3, time: float) -> float
                                {
                                    return min(100.-length(p) , abs(flame(p, time)) );
                                }
                                
                                fn raymarch( org:vec3,  dir:vec3, time: float) -> vec4
                                {
                                    let  d = 0.0;
                                    let  glow = 0.0;
                                    let eps = 0.02;
                                    let   p = org;
                                    let glowed = 0.0;
                                    let floati = 0.0;
                                    for i in 0..64
                                    {
                                        d = scene(p, time) + eps;
                                        p += d * dir;
                                        if( d>eps )
                                        {
                                            if(flame(p, time) < .0)
                                            {
                                                glowed=1.0;
                                            }
                                            if(glowed>0.0)
                                            {
                                                   glow = floati/64.;
                                            }
                                        }
                                        floati = floati+ 1.0;
                                    }
                                    return vec4(p,glow);
                                }
                                

                                fn pixel(self) -> vec4 {
                                
                                    let v= ((vec2(1.0,1.0)-self.pos) * 2.0 - vec2(1.0, 1.0));
                                    let org = vec3(0., -2., 4.);
                                    let dir = normalize(vec3(v.x*1.6, -v.y, -1.5));                                    
                                    let  p = raymarch(org, dir,self.time);
                                    let glow = p.w;                                    
                                    let col = mix(vec4(1.,.5,.1,1.), vec4(0.1,.5,1.,1.), p.y*.02+.4);
                                    let fragColor = mix(vec4(0.,0.,0.,1.), col, pow(glow*2.,4.));                        
                                    return fragColor;
                                }
                            }
                        }
                        <ContainerStage>{   
                            <IconSet> {}              
                         }
                
                
                        
                    }   

                
                    screen6 = <View>{
                        flow: Overlay,                
                        width: Fill,
                        height: Fill
                        spacing: 0,
                        padding: 0,
                        align: {
                            x: 0.5,
                            y: 0.5
                        },
                        show_bg: true,
                        draw_bg: {
                            fn pixel(self) -> vec4{
                                return vec4(0.70,0.72,0.72,1)
                            }
                        }   
                            width: Fill,
                            height: Fill,
                            <Image>{
                                width: Fill;
                                height: Fill;	
                                fit: Biggest;
                                source: dep("crate://self/resources/unsplash.jpg")
                            }
                            
                        // quad = <MyWidget> {
                        //     align:{x:0.,y:0.0}
                        //     width: Fill,
                        //     height: Fill,
                            
                        //     // draw: {
                        //     //     fn pixel(self) -> vec4 {
                        //     //      return vec4(0.0,self.pos.y*0.1,self.pos.y*0.1,1.0);   
                        //     //     } 
                        //     // }
                        //     draw: {
                        //         fn pixel(self) -> vec4 {
                                
                        //         let fragColor = mix(#A7A17C, #764423, self.pos.y);
                        //             return fragColor;
                        //         }
                        //     }
                        // }
                        <ContainerStage>{   
                            draw_bg: {
                                texture image: texture2d
                                uniform shadowopacity:  0.2,
                                uniform shadowx: 4.0,
                                uniform shadowy: 4.0,
                                uniform shadowcolor: vec3(0.01,0.01,0.02),
                                varying o0: vec2,
                                varying oShadow: vec2,
                                
                                fn vertex(self) -> vec4 {                
                                    let dpi = self.dpi_factor;                               
                                    let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                                    self.o0 = self.pos;
                                    self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi )*0.001;
                                    return pos;
                                }
                    
                                fn old_pixel(self) -> vec4 {
                    
                                    let shadow = sample2d(self.image, self.oShadow + vec2(cos(self.time*3.+self.o0.y*10.)*0.0013, cos(self.time+self.o0.x*100.)*0.0013));
                                    let main = sample2d(self.image, self.o0);
                                    let col =  (vec4(self.shadowcolor.xyz,self.shadowopacity)  * shadow.a ) * ( 1 - main.a) + main;
                                    return col;
                                }

                                 // ported from https://www.shadertoy.com/view/ltffzl
                                // Heartfelt - by Martijn Steinrucken aka BigWings - 2017

                                // Email:countfrolic@gmail.com Twitter:@The_ArtOfCode
                                // License Creative Commons Attribution-NonCommercial-ShareAlike 3.0 Unported License.


                                fn N13( p:float) -> vec3{
                                    //  from DAVE HOSKINS
                                   let p3 = fract(vec3(p) * vec3(.1031,.11369,.13787));
                                   p3 += dot(p3, p3.yzx + 19.19);
                                   return fract(vec3((p3.x + p3.y)*p3.z, (p3.x+p3.z)*p3.y, (p3.y+p3.z)*p3.x));
                                }
                                
                                fn N14( t:float)->vec4 {
                                    return fract(sin(t*vec4(123., 1024., 1456., 264.))*vec4(6547., 345., 8799., 1564.));
                                }
                                 fn N( t:float)-> float {
                                    return fract(sin(t*12345.564)*7658.76);
                                }
                                
                                fn S( a:float, b:float, t: float) ->float {
                                    return smoothstep(a, b, t);
                                }

                                fn Saw( b:float,  t: float) ->float {
                                    return S(0., b, t)*S(1., b, t);
                                }

                                
                                fn DropLayer2( uv: vec2, t:float) -> vec2{
                                    let UV = uv;
                                    
                                    uv.y += t*0.75;
                                    let a = vec2(6., 1.);
                                    let grid = a*2.;
                                    let id = floor(uv*grid);
                                    
                                    let colShift = N(id.x); 
                                    uv.y += colShift;
                                    
                                    id = floor(uv*grid);
                                    let n = N13(id.x*35.2+id.y*2376.1);
                                    let st = fract(uv*grid)-vec2(.5, 0);
                                    
                                    let x = n.x-.5;
                                    
                                    let y = UV.y*20.;
                                    let wiggle = sin(y+sin(y));
                                    x += wiggle*(.5-abs(x))*(n.z-.5);
                                    x *= .7;
                                    let ti = fract(t+n.z);
                                    y = (Saw(.85, ti)-.5)*.9+.5;
                                    let p = vec2(x, y);
                                    
                                    let d = length((st-p)*a.yx);
                                    
                                    let mainDrop = S(.4, .0, d);
                                    
                                    let r = sqrt(S(1., y, st.y));
                                    let cd = abs(st.x-x);
                                    let trail = S(.23*r, .15*r*r, cd);
                                    let trailFront = S(-.02, .02, st.y-y);
                                    trail *= trailFront*r*r;
                                    
                                    y = UV.y;
                                    let trail2 = S(.2*r, .0, cd);
                                    let droplets = max(0., (sin(y*(1.-y)*120.)-st.y))*trail2*trailFront*n.z;
                                    y = fract(y*10.)+(st.y-.5);
                                    let dd = length(st-vec2(x, y));
                                    droplets = S(.3, 0., dd);
                                    let m = mainDrop+droplets*r*trailFront;
                                    
                                    //m += st.x>a.y*.45 || st.y>a.x*.165 ? 1.2 : 0.;
                                    return vec2(m, trail);
                                }

                                fn StaticDrops( uv: vec2,  t: float) -> float{
                                    uv *= 40.;
                                    
                                    let id = floor(uv);
                                    uv = fract(uv)-.5;
                                    let n = N13(id.x*107.45+id.y*3543.654);
                                    let p = (n.xy-.5)*.7;
                                    let d = length(uv-p);
                                    
                                    let fade = Saw(.025, fract(t+n.z));
                                    let c = S(.3, 0., d)*fract(n.z*10.)*fade;
                                    return c;
                                }

                                fn Drops( uv:vec2,  t:float,  l0:float,  l1:float,  l2:float) -> vec2 {
                                    let s = StaticDrops(uv, t)*l0; 
                                    let m1 = DropLayer2(uv, t)*l1;
                                    let m2 = DropLayer2(uv*1.85, t)*l2;
                                    
                                    let c = s+m1.x+m2.x;
                                    c = S(.3, 1., c);
                                    
                                    return vec2(c, max(m1.y*l0, m2.y*l1));
                                }


                                fn pixel(self) -> vec4 {
                                
                                    let aspect = self.rect_size.x/self.rect_size.y;
                                    
                                    let ipos =self.pos;
                                    ipos.x = (ipos.x-0.5)*aspect + 0.5;                                    
                                    ipos.y = (1. - ipos.y)*0.2;
                                    ipos.x = (1. - ipos.x)*0.2;
                                    let uv = ipos * 2.0 - vec2(1.0);
                                    let UV = ipos;
                                    let col = vec4(StaticDrops(ipos, self.time),DropLayer2(ipos, self.time).x,0.,1.);
                                    let T = self.time;
                                    let t = T*.2;
                                    let rainAmount =  sin(T*.05)*.3+.7;
                                    let maxBlur = mix(3., 6., rainAmount);
                                    let minBlur = 2.;
                                    let story = 0.;
                                    let heart = 0.;
                                    let zoom = -0.8;
                                    uv *= .7+zoom*.3;
                                    UV = (UV-.5)*(.9+zoom*.1)+.5;
                                    let staticDrops = S(-.5, 1., rainAmount)*2.;
                                    let layer1 = S(.25, .75, rainAmount);
                                    let layer2 = S(.0, .5, rainAmount);
                                    let c = Drops(uv, t, staticDrops, layer1, layer2);
                                    let e = vec2(.001, 0.);
                                    let cx = Drops(uv+e, t, staticDrops, layer1, layer2).x;
                                    let cy = Drops(uv+e.yx, t, staticDrops, layer1, layer2).x;
                                    let  n = vec2(cx-c.x, cy-c.x);		// expensive normals
                                    let focus = mix(maxBlur-c.y, minBlur, S(.1, .2, c.x));
                                    let L = self.pos + n*0.52 ;

                                    let col = sample2d(self.image, L) + length(n*0.4)*vec4(0.,0.7,1.,1.);;

                                    //let col = vec4(sin(L.x*100.), cos(L.y*100.), sin(L.x*100.0),1.0);
                                    
                                    let fragColor =col;
                                    return fragColor;
                                }
                                
                            }
                            <IconSet> {}              
                         }
                
                    }
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::my_widget::live_design(cx);
        crate::iconbutton::live_design(cx);
        crate::diffuse::live_design(cx);
        crate::particles::live_design(cx);
        crate::birds::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}