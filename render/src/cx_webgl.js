(function(root) {
    var user_agent = window.navigator.userAgent;
    var is_mobile_safari = window.navigator.platform.match(/iPhone|iPad/i);
    var is_android = user_agent.match(/Android/i);
    var is_add_to_homescreen_safari = is_mobile_safari && navigator.standalone;
    var is_touch_device = ('ontouchstart' in window || navigator.maxTouchPoints);
    var is_firefox = navigator.userAgent.toLowerCase().indexOf('firefox') > -1;
    
    // message we can send to wasm
    class ToWasm {
        constructor(wasm_app) {
            this.wasm_app = wasm_app;
            this.exports = wasm_app.exports;
            this.slots = 512;
            this.used = 2; // skip 8 byte header
            // lets write
            this.pointer = this.exports.alloc_wasm_message(this.slots * 4);
            this.update_refs();
        }
        
        update_refs() {
            this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, this.slots);
            this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, this.slots);
            this.mf64 = new Float64Array(this.exports.memory.buffer, this.pointer, this.slots >> 1);
        }
        
        fit(slots) {
            this.update_refs(); // its possible our mf32/mu32 refs are dead because of realloccing of wasm heap inbetween calls
            if (this.used + slots > this.slots) {
                let new_slots = Math.max(this.used + slots, this.slots * 2) // exp alloc algo
                if (new_slots & 1)new_slots ++; // float64 align it
                let new_bytes = new_slots * 4;
                this.pointer = this.exports.realloc_wasm_message(this.pointer, new_bytes); // by
                this.slots = new_slots
                this.update_refs()
            }
            let pos = this.used;
            this.used += slots;
            return pos;
        }
        
        fetch_deps() {
            let pos = this.fit(1);
            this.mu32[pos ++] = 1;
        }
        
        send_string(str) {
            let pos = this.fit(str.length + 1)
            this.mu32[pos ++] = str.length
            for (let i = 0; i < str.length; i ++) {
                this.mu32[pos ++] = str.charCodeAt(i)
            }
        }
        
        send_f64(value) {
            if (this.used & 1) { // float64 align, need to fit another
                var pos = this.fit(3);
                pos ++;
                this.mf64[pos >> 1] = value;
            }
            else {
                var pos = this.fit(2);
                this.mf64[pos >> 1] = value;
            }
        }
        
        deps_loaded(deps) {
            let pos = this.fit(2);
            this.mu32[pos ++] = 2
            this.mu32[pos ++] = deps.length
            for (let i = 0; i < deps.length; i ++) {
                let dep = deps[i];
                this.send_string(dep.name);
                pos = this.fit(2);
                this.mu32[pos ++] = dep.vec_ptr
                this.mu32[pos ++] = dep.vec_len
            }
        }
        
        init(info) {
            let pos = this.fit(5);
            this.mu32[pos ++] = 3;
            this.mf32[pos ++] = info.width;
            this.mf32[pos ++] = info.height;
            this.mf32[pos ++] = info.dpi_factor;
            this.mu32[pos ++] = info.vr_can_present? 1: 0;
        }
        
        resize(info) {
            let pos = this.fit(6);
            this.mu32[pos ++] = 4;
            this.mf32[pos ++] = info.width;
            this.mf32[pos ++] = info.height;
            this.mf32[pos ++] = info.dpi_factor;
            this.mu32[pos ++] = info.vr_is_presenting? 1: 0;
            this.mu32[pos ++] = info.vr_can_present? 1: 0;
        }
        
        animation_frame(time) {
            let pos = this.fit(1); // float64 uses 2 slots
            this.mu32[pos ++] = 5;
            this.send_f64(time);
        }
        
        finger_down(finger) {
            let pos = this.fit(6);
            this.mu32[pos ++] = 6;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mu32[pos ++] = finger.digit
            this.mu32[pos ++] = finger.touch? 1: 0
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        finger_up(finger) {
            let pos = this.fit(6);
            this.mu32[pos ++] = 7;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mu32[pos ++] = finger.digit
            this.mu32[pos ++] = finger.touch? 1: 0
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        finger_move(finger) {
            let pos = this.fit(6);
            this.mu32[pos ++] = 8;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mu32[pos ++] = finger.digit
            this.mu32[pos ++] = finger.touch? 1: 0
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        finger_hover(finger) {
            let pos = this.fit(4);
            this.mu32[pos ++] = 9;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        finger_scroll(finger) {
            let pos = this.fit(7);
            this.mu32[pos ++] = 10;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mf32[pos ++] = finger.scroll_x
            this.mf32[pos ++] = finger.scroll_y
            this.mu32[pos ++] = finger.is_wheel? 1: 0
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        finger_out(finger) {
            let pos = this.fit(4);
            this.mu32[pos ++] = 11;
            this.mf32[pos ++] = finger.x
            this.mf32[pos ++] = finger.y
            this.mu32[pos ++] = finger.modifiers
            this.send_f64(finger.time);
        }
        
        key_down(key) {
            let pos = this.fit(4);
            this.mu32[pos ++] = 12;
            this.mu32[pos ++] = key.key_code;
            this.mu32[pos ++] = key.is_repeat? 1: 0;
            this.mu32[pos ++] = key.modifiers;
            this.send_f64(key.time);
        }
        
        key_up(key) {
            let pos = this.fit(4);
            this.mu32[pos ++] = 13;
            this.mu32[pos ++] = key.key_code;
            this.mu32[pos ++] = key.is_repeat? 1: 0;
            this.mu32[pos ++] = key.modifiers;
            this.send_f64(key.time);
        }
        
        text_input(data) {
            let pos = this.fit(3);
            this.mu32[pos ++] = 14;
            this.mu32[pos ++] = data.was_paste? 1: 0,
            this.mu32[pos ++] = data.replace_last? 1: 0,
            this.send_string(data.input);
        }
        
        read_file_data(id, buf_ptr, buf_len) {
            let pos = this.fit(4);
            this.mu32[pos ++] = 15;
            this.mu32[pos ++] = id;
            this.mu32[pos ++] = buf_ptr;
            this.mu32[pos ++] = buf_len;
        }
        
        read_file_error(id) {
            let pos = this.fit(2);
            this.mu32[pos ++] = 16;
            this.mu32[pos ++] = id;
        }
        
        
        text_copy() {
            let pos = this.fit(1);
            this.mu32[pos ++] = 17;
        }
        
        timer(id) {
            let pos = this.fit(1);
            this.mu32[pos ++] = 18;
            this.send_f64(id);
        }
        
        window_focus(is_focus) { // TODO CALL THIS
            let pos = this.fit(2);
            this.mu32[pos ++] = 19;
            this.mu32[pos ++] = is_focus? 1: 0;
        }
        
        vr_frame(time, frame_data) {
            let pos = this.fit(1);
            this.mu32[pos ++] = 20;
            this.send_f64(time);
        }
        
        paint_dirty(time, frame_data) {
            let pos = this.fit(1);
            this.mu32[pos ++] = 21;
        }
        
        end() {
            let pos = this.fit(1);
            this.mu32[pos] = 0;
        }
        
    }
    
    class WasmApp {
        constructor(canvas, webasm) {
            this.canvas = canvas;
            this.webasm = webasm;
            this.exports = webasm.instance.exports;
            this.memory = webasm.instance.exports.memory
            
            // local webgl resources
            this.shaders = [];
            this.vr_mode = false;
            this.index_buffers = [];
            this.array_buffers = [];
            this.timers = [];
            this.vaos = [];
            this.textures = [];
            this.framebuffers = [];
            this.resources = [];
            this.req_anim_frame_id = 0;
            this.text_copy_response = "";
            this.init_webgl_context();
            this.init_webvr_bindings();
            this.bind_mouse_and_touch();
            this.bind_keyboard();
            
            window.addEventListener('focus', _ => {
                this.to_wasm.window_focus(true);
                this.do_wasm_io();
            })
            
            window.addEventListener('blur', _ => {
                this.to_wasm.window_focus(false);
                this.do_wasm_io();
            })
            // lets create the wasm app and cx
            this.app = this.exports.create_wasm_app();
            
            // create initial to_wasm
            this.to_wasm = new ToWasm(this);
            
            // fetch dependencies
            this.to_wasm.fetch_deps();
            
            this.do_wasm_io();
            
            this.do_wasm_block = true;
            
            // ok now, we wait for our resources to load.
            Promise.all(this.resources).then(results => {
                let deps = []
                // copy our reslts into wasm pointers
                for (let i = 0; i < results.length; i ++) {
                    var result = results[i]
                    // allocate pointer, do +8 because of the u64 length at the head of the buffer
                    let vec_len = result.buffer.byteLength;
                    let vec_ptr = this.exports.alloc_wasm_vec(vec_len);
                    this.copy_to_wasm(result.buffer, vec_ptr);
                    deps.push({
                        name: result.name,
                        vec_ptr: vec_ptr,
                        vec_len: vec_len
                    });
                }
                // pass wasm the deps
                this.to_wasm.deps_loaded(deps);
                // initialize the application
                this.to_wasm.init({
                    width: this.width,
                    height: this.height,
                    dpi_factor: this.dpi_factor,
                    vr_can_present: this.vr_can_present,
                    vr_is_presenting: false
                })
                this.do_wasm_block = false;
                this.do_wasm_io();
                
                var loaders = document.getElementsByClassName('cx_webgl_loader');
                for (var i = 0; i < loaders.length; i ++) {
                    loaders[i].parentNode.removeChild(loaders[i])
                }
                
            })
        }
        
        do_wasm_io() {
            
            if (this.do_wasm_block) {
                return
            }
            
            //if(this.dpi_factor != window.devicePixelRatio){
            //    this.on_screen_resize();
            //}
            
            this.to_wasm.end();
            let from_wasm_ptr = this.exports.process_to_wasm(this.app, this.to_wasm.pointer)
            // get a clean to_wasm set up immediately
            this.to_wasm = new ToWasm(this);
            
            // set up local shortcuts to the from_wasm memory chunk for faster parsing
            this.parse = 2; // skip the 8 byte header
            this.mf32 = new Float32Array(this.memory.buffer, from_wasm_ptr);
            this.mu32 = new Uint32Array(this.memory.buffer, from_wasm_ptr);
            this.mf64 = new Float64Array(this.memory.buffer, from_wasm_ptr);
            this.basef32 = new Float32Array(this.memory.buffer);
            this.baseu32 = new Uint32Array(this.memory.buffer);
            this.basef64 = new Float64Array(this.memory.buffer);
            
            // process all messages
            var send_fn_table = this.send_fn_table;
            while (1) {
                let msg_type = this.mu32[this.parse ++];
                if (send_fn_table[msg_type](this)) {
                    break;
                }
            }
            // destroy from_wasm_ptr object
            this.exports.dealloc_wasm_message(from_wasm_ptr);
        }
        
        
        request_animation_frame() {
            if (this.vr_is_presenting || this.req_anim_frame_id) {
                return;
            }
            this.req_anim_frame_id = window.requestAnimationFrame(time => {
                this.req_anim_frame_id = 0;
                if (this.vr_is_presenting) {
                    return
                }
                this.to_wasm.animation_frame(time / 1000.0);
                this.in_animation_frame = true;
                this.do_wasm_io();
                this.in_animation_frame = false;
            })
        }
        
        // i forgot how to do memcpy with typed arrays. so, we'll do this.
        copy_to_wasm(input_buffer, output_ptr) {
            let u8len = input_buffer.byteLength;
            
            if ((u8len & 3) != 0 || (output_ptr & 3) != 0) { // not u32 aligned, do a byte copy
                var u8out = new Uint8Array(this.memory.buffer, output_ptr, u8len)
                var u8in = new Uint8Array(input_buffer)
                for (let i = 0; i < u8len; i ++) {
                    u8out[i] = u8in[i];
                }
            }
            else { // not f64 aligned, do a u32 copy
                let u32len = u8len >> 2; //4 bytes at a time.
                var u32out = new Uint32Array(this.memory.buffer, output_ptr, u32len)
                var u32in = new Uint32Array(input_buffer)
                for (let i = 0; i < u32len; i ++) {
                    u32out[i] = u32in[i];
                }
            }
        }
        
        on_screen_resize() {
            var dpi_factor = window.devicePixelRatio;
            var w,
            h;
            var canvas = this.canvas;
            
            if (this.vr_is_presenting) {
                let vr_display = this.vr_display;
                var left_eye = vr_display.getEyeParameters("left");
                var right_eye = vr_display.getEyeParameters("right");
                canvas.width = Math.max(left_eye.renderWidth, right_eye.renderWidth) * 2;
                canvas.height = Math.max(left_eye.renderHeight, right_eye.renderHeight);
                this.dpi_factor = 1.0;
                this.width = canvas.width >> 1;
                this.height = canvas.height;
            }
            else {
                if (canvas.getAttribute("fullpage")) {
                    if (is_add_to_homescreen_safari) { // extremely ugly. but whatever.
                        if (window.orientation == 90 || window.orientation == -90) {
                            h = screen.width;
                            w = screen.height - 90;
                        }
                        else {
                            w = screen.width;
                            h = screen.height - 80;
                        }
                    }
                    else {
                        w = window.innerWidth;
                        h = window.innerHeight;
                    }
                }
                else {
                    w = canvas.offsetWidth;
                    h = canvas.offsetHeight;
                }
                var sw = canvas.width = w * dpi_factor;
                var sh = canvas.height = h * dpi_factor;
                
                this.gl.viewport(0, 0, sw, sh);
                
                this.dpi_factor = dpi_factor;
                this.width = canvas.offsetWidth;
                this.height = canvas.offsetHeight;
                // send the wasm a screenresize event
            }
            
            if (this.to_wasm) {
                // initialize the application
                this.to_wasm.resize({
                    width: this.width,
                    height: this.height,
                    dpi_factor: this.dpi_factor,
                    vr_can_present: this.vr_can_present,
                    vr_is_presenting: this.vr_is_presenting
                })
                this.request_animation_frame()
            }
        }
        
        load_deps(deps) {
            for (var i = 0; i < deps.length; i ++) {
                let file_path = deps[i];
                this.resources.push(fetch_path(file_path))
            }
        }
        
        parse_string() {
            var str = "";
            var len = this.mu32[this.parse ++];
            for (let i = 0; i < len; i ++) {
                var c = this.mu32[this.parse ++];
                if (c != 0) str += String.fromCharCode(c);
            }
            return str
        }
        
        parse_f64() {
            if (this.parse & 1) {
                this.parse ++;
            }
            var ret = this.mf64[this.parse >> 1];
            this.parse += 2;
            return ret
        }
        
        parse_shvarvec() {
            var len = this.mu32[this.parse ++];
            var vars = []
            for (let i = 0; i < len; i ++) {
                vars.push({ty: this.parse_string(), name: this.parse_string()})
            }
            return vars
        }
        
        
        // The UA may kick us out of VR present mode for any reason, so to
        // ensure we always know when we begin/end presenting we need to
        // listen for vrdisplaypresentchange events.
        on_vr_display_present_change() {
            this.vr_is_presenting = this.vr_display.isPresenting;
            if (this.vr_is_presenting) { // we need to start the continuous repaintloop
                let vr_on_request_animation_frame = time => {
                    if (!this.vr_is_presenting) {
                        return
                    }
                    this.vr_display.requestAnimationFrame(vr_on_request_animation_frame);
                    
                    
                    // compute the view matrices taking into account the persons movement
                    this.to_wasm.paint_dirty();
                    this.to_wasm.vr_frame(time / 1000.0, this.vr_frame_data);
                    this.in_animation_frame = true;
                    this.do_wasm_io();
                    this.in_animation_frame = false;
                    this.vr_display.submitFrame();
                };
                this.vr_display.requestAnimationFrame(vr_on_request_animation_frame);
            }
            else { // lets return to normal
                this.to_wasm.paint_dirty();
                this.request_animation_frame();
            }
            this.on_screen_resize();
        }
        
        on_vr_display_activate() {
            var attributes = {depth: true, antialias: true, multiview: false};
            this.vr_display.requestPresent([{source: this.canvas, attributes: attributes}]).then(_ => {
            }, error => {
                console.log("requestPresent failed", error)
            });
        }
        
        on_vr_display_deactivate() {
            if (!this.vr_display.isPresenting) {
                return;
            }
            this.vr_display.exitPresent().then(_ => {
            }, error => {
                console.log("exitPresent failed", error)
            });
        }
        
        init_webvr_bindings() {
            this.vr_can_present = false;
            this.vr_is_presenting = false;
            // ok this changes a bunch in how the renderflow works.
            // first thing we are going to do is get the vr displays.
            if (navigator.getVRDisplays) {
                this.vr_frame_data = new VRFrameData();
                navigator.getVRDisplays().then(displays => {
                    if (displays.length > 0) {
                        this.vr_display = displays[0];
                        // It's heighly reccommended that you set the near and far planes to
                        // something appropriate for your scene so the projection matricies
                        // WebVR produces have a well scaled depth buffer.
                        this.vr_display.depthNear = 0.1;
                        this.vr_display.depthFar = 250.0;
                        // vrDisplay.resetPose();
                        // Generally, you want to wait until VR support is confirmed and
                        // you know the user has a VRDisplay capable of presenting connected
                        // before adding UI that advertises VR features.
                        if (this.vr_display.capabilities.canPresent) {
                            this.vr_can_present = true;
                            console.log("webVR available");
                            window.addEventListener("vrdisplaypresentchange", this.on_vr_display_present_change.bind(this), false);
                            window.addEventListener("vrdisplayactivate", this.on_vr_display_activate.bind(this), false);
                            window.addEventListener("vrdisplaydeactivate", this.on_vr_display_deactivate.bind(this), false);
                        }
                    }
                    else {
                        console.log("No VR displays found")
                    }
                })
            }
            else {
                console.log("No webVR support found")
            }
        }
        
        vr_start_presenting() {
            if (this.vr_can_present) {
                console.log("Starting webVR")
                this.on_vr_display_activate();
            }
        }
        
        vr_stop_presenting() {
            
        }
        
        init_webgl_context() {
            
            window.addEventListener('resize', _ => {
                this.on_screen_resize()
            })
            
            window.addEventListener('orientationchange', _ => {
                this.on_screen_resize()
            })
            
            let mqString = '(resolution: ' + window.devicePixelRatio + 'dppx)'
            let mq = matchMedia(mqString);
            if (mq && mq.addEventListener) {
                mq.addEventListener("change", _ => {
                    this.on_screen_resize()
                });
            }
            else { // poll for it. yes. its terrible
                window.setInterval(_ => {
                    if (window.devicePixelRation != this.dpi_factor) {
                        this.on_screen_resize()
                    }
                }, 1000);
            }
            
            var canvas = this.canvas
            var options = {
                alpha: canvas.getAttribute("noalpha")? false: true,
                depth: canvas.getAttribute("nodepth")? false: true,
                stencil: canvas.getAttribute("nostencil")? false: true,
                antialias: canvas.getAttribute("noantialias")? false: true,
                premultipliedAlpha: canvas.getAttribute("premultipliedAlpha")? true: false,
                preserveDrawingBuffer: canvas.getAttribute("preserveDrawingBuffer")? true: false,
                preferLowPowerToHighPerformance: true
            }
            
            var gl = this.gl = canvas.getContext('webgl', options)
                || canvas.getContext('webgl-experimental', options)
                || canvas.getContext('experimental-webgl', options)
            
            if (!gl) {
                var span = document.createElement('span')
                span.style.color = 'white'
                canvas.parentNode.replaceChild(span, canvas)
                span.innerHTML = "Sorry, makepad needs browser support for WebGL to run<br/>Please update your browser to a more modern one<br/>Update to atleast iOS 10, Safari 10, latest Chrome, Edge or Firefox<br/>Go and update and come back, your browser will be better, faster and more secure!<br/>If you are using chrome on OSX on a 2011/2012 mac please enable your GPU at: Override software rendering list:Enable (the top item) in: <a href='about://flags'>about://flags</a>. Or switch to Firefox or Safari."
                return
            }
            gl.OES_standard_derivatives = gl.getExtension('OES_standard_derivatives')
            gl.OES_vertex_array_object = gl.getExtension('OES_vertex_array_object')
            gl.OES_element_index_uint = gl.getExtension("OES_element_index_uint")
            gl.ANGLE_instanced_arrays = gl.getExtension('ANGLE_instanced_arrays')
            //gl.EXT_blend_minmax = gl.getExtension('EXT_blend_minmax')
            //gl.OES_texture_half_float_linear = gl.getExtension('OES_texture_half_float_linear')
            //gl.OES_texture_float_linear = gl.getExtension('OES_texture_float_linear')
            //gl.OES_texture_half_float = gl.getExtension('OES_texture_half_float')
            //gl.OES_texture_float = gl.getExtension('OES_texture_float')
            //gl.WEBGL_depth_texture = gl.getExtension("WEBGL_depth_texture") || gl.getExtension("WEBKIT_WEBGL_depth_texture") 
            this.on_screen_resize()
        }
        
        // new shader helpers
        get_attrib_locations(program, base, slots) {
            var gl = this.gl;
            let attrib_locs = [];
            let attribs = slots >> 2;
            let stride = slots * 4;
            if ((slots & 3) != 0) attribs ++;
            for (let i = 0; i < attribs; i ++) {
                let size = (slots - i * 4);
                if (size > 4) size = 4;
                attrib_locs.push({
                    loc: gl.getAttribLocation(program, base + i),
                    offset: i * 16,
                    size: size,
                    stride: slots * 4
                });
            }
            return attrib_locs
        }
        
        get_uniform_locations(program, uniforms) {
            var gl = this.gl;
            let uniform_locs = [];
            let offset = 0;
            for (let i = 0; i < uniforms.length; i ++) {
                let uniform = uniforms[i];
                // lets align the uniform
                let slots = this.uniform_size_table[uniform.ty];
                if ((offset & 3) != 0 && (offset & 3) + slots > 4) { // goes over the boundary
                    offset += 4 - (offset & 3); // make jump to new slot
                }
                uniform_locs.push({
                    name: uniform.name,
                    offset: offset << 2,
                    ty: uniform.ty,
                    loc: gl.getUniformLocation(program, uniform.name),
                    fn: this.uniform_fn_table[uniform.ty]
                });
                offset += slots
            }
            return uniform_locs;
        }
        
        compile_webgl_shader(ash) {
            var gl = this.gl
            var vsh = gl.createShader(gl.VERTEX_SHADER)
            
            gl.shaderSource(vsh, ash.vertex)
            gl.compileShader(vsh)
            if (!gl.getShaderParameter(vsh, gl.COMPILE_STATUS)) {
                return console.log(
                    gl.getShaderInfoLog(vsh),
                    add_line_numbers_to_string(ash.vertex)
                )
            }
            
            // compile pixelshader
            var fsh = gl.createShader(gl.FRAGMENT_SHADER)
            gl.shaderSource(fsh, ash.fragment)
            gl.compileShader(fsh)
            if (!gl.getShaderParameter(fsh, gl.COMPILE_STATUS)) {
                return console.log(
                    gl.getShaderInfoLog(fsh),
                    add_line_numbers_to_string(ash.fragment)
                )
            }
            
            var program = gl.createProgram()
            gl.attachShader(program, vsh)
            gl.attachShader(program, fsh)
            gl.linkProgram(program)
            if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
                return console.log(
                    gl.getProgramInfoLog(program),
                    add_line_numbers_to_string(ash.vertex),
                    add_line_numbers_to_string(ash.fragment)
                )
            }
            
            // fetch all attribs and uniforms
            this.shaders[ash.shader_id] = {
                geom_attribs: this.get_attrib_locations(program, "geomattr", ash.geometry_slots),
                inst_attribs: this.get_attrib_locations(program, "instattr", ash.instance_slots),
                uniforms_cx: this.get_uniform_locations(program, ash.uniforms_cx),
                uniforms_dr: this.get_uniform_locations(program, ash.uniforms_dr),
                uniforms_dl: this.get_uniform_locations(program, ash.uniforms_dl),
                texture_slots: this.get_uniform_locations(program, ash.texture_slots),
                instance_slots: ash.instance_slots,
                program: program,
                ash: ash
            };
        }
        
        set_document_title(title) {
            document.title = title
        }
        
        bind_mouse_and_touch() {
            
            this.cursor_map = [
                "none", //Hidden=>0
                "default", //Default=>1,
                "crosshair", //CrossHair=>2,
                "pointer", //Hand=>3,
                "default", //Arrow=>4,
                "move", //Move=>5,
                "text", //Text=>6,
                "wait", //Wait=>7,
                "help", //Help=>8,
                "not-allowed", //NotAllowed=>9,
                "n-resize", // NResize=>10,
                "ne-resize", // NeResize=>11,
                "e-resize", // EResize=>12,
                "se-resize", // SeResize=>13,
                "s-resize", // SResize=>14,
                "sw-resize", // SwResize=>15,
                "w-resize", // WResize=>16,
                "nw-resize", // NwResize=>17,
                "ns-resize", //NsResize=>18,
                "nesw-resize", //NeswResize=>19,
                "ew-resize", //EwResize=>20,
                "nwse-resize", //NwseResize=>21,
                "col-resize", //ColResize=>22,
                "row-resize", //RowResize=>23,
            ]
            
            var canvas = this.canvas
            function mouse_to_finger(e) {
                return {
                    x: e.pageX,
                    y: e.pageY,
                    digit: e.button,
                    time: e.timeStamp / 1000.0,
                    modifiers: pack_key_modifier(e),
                    touch: false
                }
            }
            
            var digit_map = {}
            var digit_alloc = 0;
            
            function touch_to_finger_alloc(e) {
                var f = []
                for (let i = 0; i < e.changedTouches.length; i ++) {
                    var t = e.changedTouches[i]
                    // find an unused digit
                    var digit = undefined;
                    for (digit in digit_map) {
                        if (!digit_map[digit]) break
                    }
                    // we need to alloc a new one
                    if (digit === undefined || digit_map[digit]) digit = digit_alloc ++;
                    // store it
                    digit_map[digit] = {identifier: t.identifier};
                    // return allocated digit
                    digit = parseInt(digit);
                    
                    f.push({
                        x: t.pageX,
                        y: t.pageY,
                        digit: digit,
                        time: e.timeStamp / 1000.0,
                        modifiers: 0,
                        touch: true,
                    })
                }
                return f
            }
            
            function lookup_digit(identifier) {
                for (let digit in digit_map) {
                    var digit_id = digit_map[digit]
                    if (!digit_id) continue
                    if (digit_id.identifier == identifier) {
                        return digit
                    }
                }
            }
            
            function touch_to_finger_lookup(e) {
                var f = []
                for (let i = 0; i < e.changedTouches.length; i ++) {
                    var t = e.changedTouches[i]
                    f.push({
                        x: t.pageX,
                        y: t.pageY,
                        digit: lookup_digit(t.identifier),
                        time: e.timeStamp / 1000.0,
                        modifiers: {},
                        touch: true,
                    })
                }
                return f
            }
            
            function touch_to_finger_free(e) {
                var f = []
                for (let i = 0; i < e.changedTouches.length; i ++) {
                    var t = e.changedTouches[i]
                    var digit = lookup_digit(t.identifier)
                    if (!digit) {
                        console.log("Undefined state in free_digit");
                        digit = 0
                    }
                    else {
                        digit_map[digit] = undefined
                    }
                    
                    f.push({
                        x: t.pageX,
                        y: t.pageY,
                        time: e.timeStamp / 1000.0,
                        digit: digit,
                        modifiers: 0,
                        touch: true,
                    })
                }
                return f
            }
            
            var mouse_buttons_down = [];
            this.mouse_down_handler = e => {
                e.preventDefault();
                this.focus_keyboard_input();
                mouse_buttons_down[e.button] = true;
                this.to_wasm.finger_down(mouse_to_finger(e))
                this.do_wasm_io();
            }

            canvas.addEventListener('mousedown', this.mouse_down_handler)
            
            this.mouse_up_handler = e => {
                e.preventDefault();
                mouse_buttons_down[e.button] = false;
                this.to_wasm.finger_up(mouse_to_finger(e))
                this.do_wasm_io();
            }

            window.addEventListener('mouseup', this.mouse_up_handler)
            
            let mouse_move = e => {
                document.body.scrollTop = 0;
                document.body.scrollLeft = 0;
                
                for (var i = 0; i < mouse_buttons_down.length; i ++) {
                    if (mouse_buttons_down[i]) {
                        this.to_wasm.finger_move({
                            x: e.pageX,
                            y: e.pageY,
                            time: e.timeStamp / 1000.0,
                            modifiers: 0,
                            digit: i
                        })
                    }
                }
                this.to_wasm.finger_hover(mouse_to_finger(e));
                var begin = performance.now();
                
                this.do_wasm_io();
                var end = performance.now();
                //console.log("Redraw cycle "+(end-begin)+" ms");
            }
            window.addEventListener('mousemove', mouse_move);
            
            window.addEventListener('mouseout', e => {
                this.to_wasm.finger_out(mouse_to_finger(e)) //e.pageX, e.pageY, pa;
                this.do_wasm_io();
            });
            canvas.addEventListener('contextmenu', e => {
                e.preventDefault()
                return false
            })
            canvas.addEventListener('touchstart', e => {
                e.preventDefault()
                let fingers = touch_to_finger_alloc(e);
                for (let i = 0; i < fingers.length; i ++) {
                    this.to_wasm.finger_down(fingers[i])
                }
                this.do_wasm_io();
                return false
            })
            canvas.addEventListener('touchmove', e => {
                //e.preventDefault();
                var fingers = touch_to_finger_lookup(e);
                for (let i = 0; i < fingers.length; i ++) {
                    this.to_wasm.finger_move(fingers[i])
                }
                this.do_wasm_io();
                return false
            }, {passive: false})
            var end_cancel_leave = e => {
                e.preventDefault();
                var fingers = touch_to_finger_free(e);
                for (let i = 0; i < fingers.length; i ++) {
                    this.to_wasm.finger_up(fingers[i])
                }
                this.do_wasm_io();
                return false
            }
            canvas.addEventListener('touchend', end_cancel_leave);
            canvas.addEventListener('touchcancel', end_cancel_leave);
            canvas.addEventListener('touchleave', end_cancel_leave);
            
            var last_wheel_time;
            var last_was_wheel;
            this.mouse_wheel_handler =  e => {
                var finger = mouse_to_finger(e)
                e.preventDefault()
                let delta = e.timeStamp - last_wheel_time;
                last_wheel_time = e.timeStamp;
                // typical web bullshit. this reliably detects mousewheel or touchpad on mac in safari
                if (is_firefox) {
                    last_was_wheel = e.deltaMode == 1
                }
                else { // detect it
                    if (Math.abs(Math.abs((e.deltaY / e.wheelDeltaY)) - (1. / 3.)) < 0.00001 || !last_was_wheel && delta < 250) {
                        last_was_wheel = false;
                    }
                    else {
                        last_was_wheel = true;
                    }
                }
                //console.log(e.deltaY / e.wheelDeltaY);
                //last_delta = delta;
                var fac = 1
                if (e.deltaMode === 1) fac = 40
                else if (e.deltaMode === 2) fac = window.offsetHeight
                finger.scroll_x = e.deltaX * fac
                finger.scroll_y = e.deltaY * fac
                finger.is_wheel = last_was_wheel;
                this.to_wasm.finger_scroll(finger);
                this.do_wasm_io();
            };
            canvas.addEventListener('wheel', this.mouse_wheel_handler)
            //window.addEventListener('webkitmouseforcewillbegin', this.onCheckMacForce.bind(this), false)
            //window.addEventListener('webkitmouseforcechanged', this.onCheckMacForce.bind(this), false)
        }
        
        bind_keyboard() {
            if (is_mobile_safari || is_android) { // mobile keyboards are unusable on a UI like this. Not happening.
                return
            }
            var ta = this.text_area = document.createElement('textarea')
            ta.className = "makepad"
            ta.setAttribute('autocomplete', 'off')
            ta.setAttribute('autocorrect', 'off')
            ta.setAttribute('autocapitalize', 'off')
            ta.setAttribute('spellcheck', 'false')
            var style = document.createElement('style')
            style.innerHTML = "\n"
                + "textarea.makepad {\n"
                + "z-index: 100000;\n"
                + "position: absolute;\n"
                + "opacity: 0;\n"
                + "border-radius: 4px;\n"
                + "color:white;\n"
                + "font-size: 6;\n"
                + "background: gray;\n"
                + "-moz-appearance: none;\n"
                + "appearance:none;\n"
                + "border:none;\n"
                + "resize: none;\n"
                + "outline: none;\n"
                + "overflow: hidden;\n"
                + "text-indent: 0px;\n"
                + "padding: 0 0px;\n"
                + "margin: 0 -1px;\n"
                + "text-indent: 0px;\n"
                + "-ms-user-select: text;\n"
                + "-moz-user-select: text;\n"
                + "-webkit-user-select: text;\n"
                + "user-select: text;\n"
                + "white-space: pre!important;\n"
                + "}\n"
                + "textarea: focus.makepad {\n"
                + "outline: 0px !important;\n"
                + "-webkit-appearance: none;\n"
                + "}"
            document.body.appendChild(style)
            ta.style.left = -100 + 'px'
            ta.style.top = -100 + 'px'
            ta.style.height = 1
            ta.style.width = 1
            
            // make the IME not show up:
            //ta.setAttribute('readonly','false')
            
            //document.addEventListener('focusout', this.onFocusOut.bind(this))
            var was_paste = false;
            this.neutralize_ime = false;
            var last_len = 0;
            ta.addEventListener('cut', e => {
                setTimeout(_ => {
                    ta.value = "";
                    last_len = 0;
                }, 0)
            })
            ta.addEventListener('copy', e => {
                setTimeout(_ => {
                    ta.value = "";
                    last_len = 0;
                }, 0)
            })
            ta.addEventListener('paste', e => {
                was_paste = true;
            })
            ta.addEventListener('select', e => {
                
            })
            
            ta.addEventListener('input', e => {
                if (ta.value.length > 0) {
                    if (was_paste) {
                        was_paste = false;
                        
                        this.to_wasm.text_input({
                            was_paste: true,
                            input: ta.value.substring(last_len),
                            replace_last: false,
                        })
                        ta.value = "";
                    }
                    else {
                        var replace_last = false;
                        var text_value = ta.value;
                        if (ta.value.length >= 2) { // we want the second char
                            text_value = ta.value.substring(1, 2);
                            ta.value = text_value;
                        }
                        else if (ta.value.length == 1 && last_len == ta.value.length) { // its an IME replace
                            replace_last = true;
                        }
                        // we should send a replace last
                        if (replace_last || text_value != '\n') {
                            this.to_wasm.text_input({
                                was_paste: false,
                                input: text_value,
                                replace_last: replace_last,
                            })
                        }
                    }
                    this.do_wasm_io();
                }
                last_len = ta.value.length;
            })
            
            ta.addEventListener('mousedown', this.mouse_down_handler);
            ta.addEventListener('mouseup', this.mouse_up_handler);
            ta.addEventListener('wheel', this.mouse_wheel_handler);
            ta.addEventListener('contextmenu', e=>{
                e.preventDefault()
            });
            //ta.addEventListener('touchmove', e => {
            //})
            
            ta.addEventListener('blur', e => {
                this.focus_keyboard_input();
            })
            
            var ugly_ime_hack = false;
            
            ta.addEventListener('keydown', e => {
                let code = e.keyCode;
                
                //if (code == 91) {firefox_logo_key = true; e.preventDefault();}
                if (code == 18 || code == 17 || code == 16) e.preventDefault(); // alt
                if (code === 8 || code === 9) e.preventDefault() // backspace/tab
                if ((code === 88 || code == 67) && (e.metaKey || e.ctrlKey)) { // copy or cut
                    // we need to request the clipboard
                    this.to_wasm.text_copy();
                    this.do_wasm_io();
                    ta.value = this.text_copy_response;
                    ta.selectionStart = 0;
                    ta.selectionEnd = ta.value.length;
                }
                //    this.keyboardCut = true // x cut
                //if(code === 65 && (e.metaKey || e.ctrlKey)) this.keyboardSelectAll = true     // all (select all)
                if (code === 89 && (e.metaKey || e.ctrlKey)) e.preventDefault() // all (select all)
                if (code === 83 && (e.metaKey || e.ctrlKey)) e.preventDefault() // ctrl s
                if (code === 90 && (e.metaKey || e.ctrlKey)) {
                    this.update_text_area_pos();
                    ta.value = "";
                    ugly_ime_hack = true;
                    ta.readOnly = true;
                    e.preventDefault()
                }
                // if we are using arrow keys, home or end
                let key_code = e.keyCode;
                
                if (key_code >= 33 && key_code <= 40) {
                    ta.value = "";
                    last_len = ta.value.length;
                }
                //if(key_code
                this.to_wasm.key_down({
                    key_code: key_code,
                    char_code: e.charCode,
                    is_repeat: e.repeat,
                    time: e.timeStamp / 1000.0,
                    modifiers: pack_key_modifier(e)
                })
                
                this.do_wasm_io();
            })
            ta.addEventListener('keyup', e => {
                let code = e.keyCode;
                
                if (code == 18 || code == 17 || code == 16) e.preventDefault(); // alt
                if (code == 91) {e.preventDefault();}
                var ta = this.text_area;
                if (ugly_ime_hack) {
                    ugly_ime_hack = false;
                    document.body.removeChild(ta);
                    this.bind_keyboard();
                    this.update_text_area_pos();
                }
                this.to_wasm.key_up({
                    key_code: e.keyCode,
                    char_code: e.charCode,
                    is_repeat: e.repeat,
                    time: e.timeStamp / 1000.0,
                    modifiers: pack_key_modifier(e)
                })
                this.do_wasm_io();
            })
            document.body.appendChild(ta);
            ta.focus();
        }
        
        focus_keyboard_input() {
            this.text_area.focus();
        }
        
        update_text_area_pos() {
            var pos = this.text_area_pos;
            var ta = this.text_area;
            if (ta) {
                ta.style.left = (Math.round(pos.x) - 4) + "px";
                ta.style.top = Math.round(pos.y) + "px"
            }
        }
        
        show_text_ime(x, y) {
            this.text_area_pos = {x: x, y: y}
            this.update_text_area_pos();
        }
        
        hide_text_ime() {
        }
        
        alloc_array_buffer(array_buffer_id, array) {
            if (this.multipass_updated_buffers) {
                return
            }
            var gl = this.gl;
            let gl_buf = this.array_buffers[array_buffer_id] || gl.createBuffer()
            gl_buf.length = array.length;
            gl.bindBuffer(gl.ARRAY_BUFFER, gl_buf);
            gl.bufferData(gl.ARRAY_BUFFER, array, gl.STATIC_DRAW);
            this.array_buffers[array_buffer_id] = gl_buf;
        }
        
        alloc_index_buffer(index_buffer_id, array) {
            if (this.multipass_updated_buffers) {
                return
            }
            var gl = this.gl;
            let gl_buf = this.index_buffers[index_buffer_id] || gl.createBuffer();
            gl_buf.length = array.length;
            gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, gl_buf);
            gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, array, gl.STATIC_DRAW);
            this.index_buffers[index_buffer_id] = gl_buf;
        }
        
        alloc_texture(texture_id, width, height, data_ptr) {
            if (this.multipass_updated_buffers) {
                return
            }
            var gl = this.gl;
            var gl_tex = this.textures[texture_id] || gl.createTexture()
            
            gl.bindTexture(gl.TEXTURE_2D, gl_tex)
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
            
            let data = new Uint8Array(this.memory.buffer, data_ptr, width * height * 4);
            gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
            //gl.bindTexture(gl.TEXTURE_2D,0);
            this.textures[texture_id] = gl_tex;
        }
        
        alloc_vao(shader_id, vao_id, geom_ib_id, geom_vb_id, inst_vb_id) {
            if (this.multipass_updated_buffers) {
                return
            }
            let gl = this.gl;
            
            let shader = this.shaders[shader_id];
            
            let old_vao = this.vaos[vao_id];
            if (old_vao) {
                gl.OES_vertex_array_object.deleteVertexArrayOES(old_vao);
            }
            let vao = gl.OES_vertex_array_object.createVertexArrayOES();
            this.vaos[vao_id] = vao;
            
            vao.geom_ib_id = geom_ib_id;
            vao.geom_vb_id = geom_vb_id;
            vao.inst_vb_id = inst_vb_id;
            
            gl.OES_vertex_array_object.bindVertexArrayOES(vao)
            
            gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[geom_vb_id]);
            
            for (let i = 0; i < shader.geom_attribs.length; i ++) {
                let attr = shader.geom_attribs[i];
                gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
                gl.enableVertexAttribArray(attr.loc);
                gl.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 0);
            }
            
            gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[inst_vb_id]);
            for (let i = 0; i < shader.inst_attribs.length; i ++) {
                let attr = shader.inst_attribs[i];
                gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
                gl.enableVertexAttribArray(attr.loc);
                gl.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 1);
            }
            
            gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.index_buffers[geom_ib_id]);
            //gl.OES_vertex_array_object.bindVertexArrayOES(0);
            
        }
        
        draw_call(shader_id, vao_id, uniforms_cx_ptr, uni_cx_update, uniforms_dl_ptr, uni_dl_update, uniforms_dr_ptr, uni_dr_update, textures_ptr) {
            var gl = this.gl;
            
            let shader = this.shaders[shader_id];
            gl.useProgram(shader.program);
            
            let vao = this.vaos[vao_id];
            gl.OES_vertex_array_object.bindVertexArrayOES(vao);
            
            let index_buffer = this.index_buffers[vao.geom_ib_id];
            let instance_buffer = this.array_buffers[vao.inst_vb_id];
            // set up uniforms TODO do this a bit more incremental based on uniform layer
            // also possibly use webGL2 uniform buffers. For now this will suffice for webGL 1 compat
            let uniforms_cx = shader.uniforms_cx;
            // if vr_presenting
            if (this.is_main_canvas && this.vr_is_presenting) {
                // the first 2 matrices are project and view
                if (this.multipass_updated_buffers) {
                    gl.uniformMatrix4fv(uniforms_cx[0].loc, false, this.vr_frame_data.rightProjectionMatrix)
                    gl.uniformMatrix4fv(uniforms_cx[1].loc, false, this.vr_right_view_matrix)
                }
                else {
                    gl.uniformMatrix4fv(uniforms_cx[0].loc, false, this.vr_frame_data.leftProjectionMatrix)
                    gl.uniformMatrix4fv(uniforms_cx[1].loc, false, this.vr_left_view_matrix)
                }
                for (let i = 2; i < uniforms_cx.length; i ++) {
                    let uni = uniforms_cx[i];
                    uni.fn(this, uni.loc, uni.offset + uniforms_cx_ptr);
                }
            }
            else {
                for (let i = 0; i < uniforms_cx.length; i ++) {
                    let uni = uniforms_cx[i];
                    uni.fn(this, uni.loc, uni.offset + uniforms_cx_ptr);
                }
            }
            let uniforms_dl = shader.uniforms_dl;
            for (let i = 0; i < uniforms_dl.length; i ++) {
                let uni = uniforms_dl[i];
                uni.fn(this, uni.loc, uni.offset + uniforms_dl_ptr);
            }
            let uniforms_dr = shader.uniforms_dr;
            for (let i = 0; i < uniforms_dr.length; i ++) {
                let uni = uniforms_dr[i];
                uni.fn(this, uni.loc, uni.offset + uniforms_dr_ptr);
            }
            let texture_slots = shader.texture_slots;
            for (let i = 0; i < texture_slots.length; i ++) {
                let tex_slot = texture_slots[i];
                let tex_id = this.baseu32[(textures_ptr >> 2) + i];
                let tex_obj = this.textures[tex_id];
                gl.activeTexture(gl.TEXTURE0 + i);
                gl.bindTexture(gl.TEXTURE_2D, tex_obj);
                gl.uniform1i(tex_slot.loc, i);
            }
            let indices = index_buffer.length;
            let instances = instance_buffer.length / shader.instance_slots;
            // lets do a drawcall!
            gl.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
        }
        
        set_default_depth_and_blend_mode() {
            let gl = this.gl
            gl.enable(gl.DEPTH_TEST);
            gl.depthFunc(gl.LEQUAL);
            gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
            gl.blendFuncSeparate(gl.ONE, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
            gl.enable(gl.BLEND);
        }
        
        begin_main_canvas(r, g, b, a, depth) {
            let gl = this.gl
            this.is_main_canvas = true;
            gl.bindFramebuffer(gl.FRAMEBUFFER, null);
            gl.clearColor(r, g, b, a);
            gl.clearDepth(depth);
            gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
            
            if (this.vr_is_presenting) { // set up the left eye
                // set the viewport to the whole thing
                gl.viewport(0, 0, this.canvas.width * 0.5, this.canvas.height);
                
                this.vr_display.getFrameData(this.vr_frame_data);
                
                let inv = new Float32Array(16);
                this.vr_left_view_matrix = new Float32Array(16);
                this.vr_right_view_matrix = new Float32Array(16);
                if (this.vr_display.stageParameters) {
                    mat4_invert(inv, this.vr_display.stageParameters.sittingToStandingTransform);
                }
                else {
                    mat4_translation(inv, [0, 1.65, 0]);
                    mat4_invert(inv, inv);
                }
                mat4_multiply(this.vr_left_view_matrix, this.vr_frame_data.leftViewMatrix, inv);
                mat4_multiply(this.vr_right_view_matrix, this.vr_frame_data.rightViewMatrix, inv);
            }
            else{
                gl.viewport(0, 0, this.canvas.width, this.canvas.height);
            }
        }
        
        
        begin_render_targets(pass_id, width, height) {
            let gl = this.gl
            this.target_width = width;
            this.target_height = height;
            this.color_targets = 0;
            this.clear_flags = 0;
            this.is_main_canvas = false;
            var gl_framebuffer = this.framebuffers[pass_id] || (this.framebuffers[pass_id] = gl.createFramebuffer());
            gl.bindFramebuffer(gl.FRAMEBUFFER, gl_framebuffer);
        }
        
        add_color_target(texture_id, init_only, r, g, b, a) {
            // if use_default
            this.clear_r = r;
            this.clear_g = g;
            this.clear_b = b;
            this.clear_a = a;
            var gl = this.gl;
            
            var gl_tex = this.textures[texture_id] || (this.textures[texture_id] = gl.createTexture());
            
            // resize or create texture
            if (gl_tex.mp_width != this.target_width || gl_tex.mp_height != this.target_height) {
                gl.bindTexture(gl.TEXTURE_2D, gl_tex)
                this.clear_flags = gl.COLOR_BUFFER_BIT;
                
                gl_tex.mp_width = this.target_width
                gl_tex.mp_height = this.target_height
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
                
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl_tex.mp_width, gl_tex.mp_height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
            }
            else if (!init_only) {
                this.clear_flags = gl.COLOR_BUFFER_BIT;
            }
            
            gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, gl_tex, 0)
            this.color_targets += 1;
        }
        
        set_depth_target(texture_id, init_only, depth) {
            this.clear_depth = depth;
            console.log("IMPLEMENT DEPTH TEXTURE TARGETS ON WEBGL")
        }
        
        end_render_targets() {
            var gl = this.gl;
            
            // process the actual 'clear'
            gl.viewport(0, 0, this.target_width, this.target_height);
            
            // check if we need to clear color, and depth
            // clear it
            if (this.clear_flags) {
                gl.clearColor(this.clear_r, this.clear_g, this.clear_b, this.clear_a);
                gl.clearDepth(this.clear_depth);
                gl.clear(this.clear_flags);
            }
        }
        
        mark_vr_draw_eye() {
            this.multipass_updated_buffers = false;
            // mark parse position for VR multiple eyes
            this.vr_begin_parse = this.parse;
        }
        
        loop_vr_draw_eye() {
            // mark parse end position
            if (!this.multipass_updated_buffers) {
                this.multipass_updated_buffers = true;
                // set up the right eye
                this.gl.viewport(this.canvas.width * 0.5, 0, this.canvas.width * 0.5, this.canvas.height);
                // jump the parser back to begin_frame
                this.parse = this.vr_begin_parse;
            }
        }
        
        set_mouse_cursor(id) {
            document.body.style.cursor = this.cursor_map[id] || 'default'
        }
        
        read_file(id, file_path) {
            
            fetch_path(file_path).then(result => {
                let byte_len = result.buffer.byteLength
                let output_ptr = this.exports.alloc_wasm_vec(byte_len);
                this.copy_to_wasm(result.buffer, output_ptr);
                this.to_wasm.read_file_data(id, output_ptr, byte_len)
                this.do_wasm_io();
            }, err => {
                this.to_wasm.read_file_error(id)
                this.do_wasm_io();
            })
        }
        
        start_timer(id, interval, repeats) {
            for (let i = 0; i < this.timers.length; i ++) {
                if (this.timers[i].id == id) {
                    console.log("Timer ID collision!")
                    return
                }
            }
            var obj = {id: id, repeats: repeats};
            if (repeats !== 0) {
                obj.sys_id = window.setInterval(e => {
                    this.to_wasm.timer(id);
                    this.do_wasm_io();
                }, interval * 1000.0);
            }
            else {
                obj.sys_id = window.setTimeout(e => {
                    for (let i = 0; i < this.timers.length; i ++) {
                        let timer = this.timers[i];
                        if (timer.id == id) {
                            this.timers.splice(i, 1);
                            break;
                        }
                    }
                    this.to_wasm.timer(id);
                    this.do_wasm_io();
                }, interval * 1000.0);
            }
            this.timers.push(obj)
        }
        
        stop_timer(id) {
            for (let i = 0; i < this.timers.length; i ++) {
                let timer = this.timers[i];
                if (timer.id == id) {
                    if (timer.repeats) {
                        window.clearInterval(timer.sys_id);
                    }
                    else {
                        window.clearTimeout(timer.sys_id);
                    }
                    this.timers.splice(i, 1);
                    return
                }
            }
            //console.log("Timer ID not found!")
        }
    }
    
    // array of function id's wasm can call on us, self is pointer to WasmApp
    WasmApp.prototype.send_fn_table = [
        function end_0(self) {
            return true;
        },
        function log_1(self) {
            console.log(self.parse_string());
        },
        function compile_webgl_shader_2(self) {
            let ash = {
                shader_id: self.mu32[self.parse ++],
                fragment: self.parse_string(),
                vertex: self.parse_string(),
                geometry_slots: self.mu32[self.parse ++],
                instance_slots: self.mu32[self.parse ++],
                uniforms_cx: self.parse_shvarvec(),
                uniforms_dl: self.parse_shvarvec(),
                uniforms_dr: self.parse_shvarvec(),
                texture_slots: self.parse_shvarvec()
            }
            self.compile_webgl_shader(ash);
        },
        function alloc_array_buffer_3(self) {
            let array_buffer_id = self.mu32[self.parse ++];
            let len = self.mu32[self.parse ++];
            let pointer = self.mu32[self.parse ++];
            let array = new Float32Array(self.memory.buffer, pointer, len);
            self.alloc_array_buffer(array_buffer_id, array);
        },
        function alloc_index_buffer_4(self) {
            let index_buffer_id = self.mu32[self.parse ++];
            let len = self.mu32[self.parse ++];
            let pointer = self.mu32[self.parse ++];
            let array = new Uint32Array(self.memory.buffer, pointer, len);
            self.alloc_index_buffer(index_buffer_id, array);
        },
        function alloc_vao_5(self) {
            let shader_id = self.mu32[self.parse ++];
            let vao_id = self.mu32[self.parse ++];
            let geom_ib_id = self.mu32[self.parse ++];
            let geom_vb_id = self.mu32[self.parse ++];
            let inst_vb_id = self.mu32[self.parse ++];
            self.alloc_vao(shader_id, vao_id, geom_ib_id, geom_vb_id, inst_vb_id)
        },
        function draw_call_6(self) {
            let shader_id = self.mu32[self.parse ++];
            let vao_id = self.mu32[self.parse ++];
            let uniforms_cx_ptr = self.mu32[self.parse ++];
            let uni_cx_update = self.mu32[self.parse ++];
            let uniforms_dl_ptr = self.mu32[self.parse ++];
            let uni_dl_update = self.mu32[self.parse ++];
            let uniforms_dr_ptr = self.mu32[self.parse ++];
            let uni_dr_update = self.mu32[self.parse ++];
            let textures = self.mu32[self.parse ++];
            self.draw_call(
                shader_id,
                vao_id,
                uniforms_cx_ptr,
                uni_cx_update,
                uniforms_dl_ptr,
                uni_dl_update,
                uniforms_dr_ptr,
                uni_dr_update,
                textures
            );
        },
        function clear_7(self) {
            let r = self.mf32[self.parse ++];
            let g = self.mf32[self.parse ++];
            let b = self.mf32[self.parse ++];
            let a = self.mf32[self.parse ++];
            self.clear(r, g, b, a);
        },
        function load_deps_8(self) {
            let deps = []
            let num_deps = self.mu32[self.parse ++];
            for (let i = 0; i < num_deps; i ++) {
                deps.push(self.parse_string());
            }
            self.load_deps(deps);
        },
        function alloc_texture_9(self) {
            let texture_id = self.mu32[self.parse ++];
            let width = self.mu32[self.parse ++];
            let height = self.mu32[self.parse ++];
            let data_ptr = self.mu32[self.parse ++];
            self.alloc_texture(texture_id, width, height, data_ptr);
        },
        function request_animation_frame_10(self) {
            self.request_animation_frame()
        },
        function set_document_title_11(self) {
            self.set_document_title(self.parse_string())
        },
        function set_mouse_cursor_12(self) {
            self.set_mouse_cursor(self.mu32[self.parse ++]);
        },
        function read_file_13(self) {
            self.read_file(self.mu32[self.parse ++], self.parse_string());
        },
        function show_text_ime_14(self) {
            self.show_text_ime(self.mf32[self.parse ++], self.mf32[self.parse ++])
        },
        function hide_text_ime_15(self) {
            self.hide_text_ime();
        },
        function text_copy_response_16(self) {
            self.text_copy_response = self.parse_string();
        },
        function start_timer_17(self) {
            var repeats = self.mu32[self.parse ++]
            var id = self.parse_f64();
            var interval = self.parse_f64();
            self.start_timer(id, interval, repeats);
        },
        function stop_timer_18(self) {
            var id = self.parse_f64();
            self.stop_timer(id);
        },
        function vr_start_presenting_19(self) {
            self.vr_start_presenting();
        },
        function vr_stop_presenting_20(self) {
            self.vr_stop_presenting();
        },
        function mark_vr_draw_eye_21(self) {
            self.mark_vr_draw_eye();
        },
        function loop_vr_draw_eye_22(self) {
            self.loop_vr_draw_eye();
        },
        function begin_render_targets_23(self) {
            let pass_id = self.mu32[self.parse++];
            let width = self.mu32[self.parse ++];
            let height = self.mu32[self.parse ++];
            self.begin_render_targets(pass_id, width, height);
        },
        function add_color_target_24(self) {
            let texture_id = self.mu32[self.parse ++];
            let init_only = self.mu32[self.parse ++];
            let r = self.mf32[self.parse ++];
            let g = self.mf32[self.parse ++];
            let b = self.mf32[self.parse ++];
            let a = self.mf32[self.parse ++];
            self.add_color_target(texture_id, init_only, r, g, b, a)
        },
        function set_depth_target_25(self) {
            let texture_id = self.mu32[self.parse ++];
            let init_only = self.mu32[self.parse ++];
            let depth = self.mf32[self.parse ++];
            self.set_depth_target(texture_id, init_only, depth);
        },
        function end_render_targets_26(self) {
            self.end_render_targets();
        },
        function set_default_depth_and_blend_mode_27(self) {
            self.set_default_depth_and_blend_mode();
        },
        function begin_main_canvas_28(self){
            let r = self.mf32[self.parse ++];
            let g = self.mf32[self.parse ++];
            let b = self.mf32[self.parse ++];
            let a = self.mf32[self.parse ++];
            let depth = self.mf32[self.parse ++];
            self.begin_main_canvas(r, g, b, a, depth);
        }
    ]
    
    WasmApp.prototype.uniform_fn_table = {
        "float": function set_float(self, loc, off) {
            let slot = off >> 2;
            self.gl.uniform1f(loc, self.basef32[slot])
        },
        "vec2": function set_vec2(self, loc, off) {
            let slot = off >> 2;
            let basef32 = self.basef32;
            self.gl.uniform2f(loc, basef32[slot], basef32[slot + 1])
        },
        "vec3": function set_vec3(self, loc, off) {
            let slot = off >> 2;
            let basef32 = self.basef32;
            self.gl.uniform3f(loc, basef32[slot], basef32[slot + 1], basef32[slot + 2])
        },
        "vec4": function set_vec4(self, loc, off) {
            let slot = off >> 2;
            let basef32 = self.basef32;
            self.gl.uniform4f(loc, basef32[slot], basef32[slot + 1], basef32[slot + 2], basef32[slot + 3])
        },
        "mat2": function set_mat2(self, loc, off) {
            self.gl.uniformMatrix2fv(loc, false, new Float32Array(self.memory.buffer, off, 4))
        },
        "mat3": function set_mat3(self, loc, off) {
            self.gl.uniformMatrix3fv(loc, false, new Float32Array(self.memory.buffer, off, 9))
        },
        "mat4": function set_mat4(self, loc, off) {
            let mat4 = new Float32Array(self.memory.buffer, off, 16);
            self.gl.uniformMatrix4fv(loc, false, mat4)
        },
    };
    
    WasmApp.prototype.uniform_size_table = {
        "float": 1,
        "vec2": 2,
        "vec3": 3,
        "vec4": 4,
        "mat2": 4,
        "mat3": 9,
        "mat4": 16
    }
    
    function add_line_numbers_to_string(code) {
        var lines = code.split('\n')
        var out = ''
        for (let i = 0; i < lines.length; i ++) {
            out += (i + 1) + ': ' + lines[i] + '\n'
        }
        return out
    }
    
    //var firefox_logo_key = false;
    function pack_key_modifier(e) {
        return (e.shiftKey? 1: 0) | (e.ctrlKey? 2: 0) | (e.altKey? 4: 0) | (e.metaKey? 8: 0)
    }
    
    function mat4_invert(out, a) {
        let a00 = a[0]
        let a01 = a[1]
        let a02 = a[2]
        let a03 = a[3]
        let a10 = a[4]
        let a11 = a[5]
        let a12 = a[6]
        let a13 = a[7]
        let a20 = a[8]
        let a21 = a[9]
        let a22 = a[10]
        let a23 = a[11]
        let a30 = a[12]
        let a31 = a[13]
        let a32 = a[14]
        let a33 = a[15]
        
        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;
        
        // Calculate the determinant
        let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
        
        if (!det) {
            return null;
        }
        det = 1.0 / det;
        
        out[0] = (a11 * b11 - a12 * b10 + a13 * b09) * det;
        out[1] = (a02 * b10 - a01 * b11 - a03 * b09) * det;
        out[2] = (a31 * b05 - a32 * b04 + a33 * b03) * det;
        out[3] = (a22 * b04 - a21 * b05 - a23 * b03) * det;
        out[4] = (a12 * b08 - a10 * b11 - a13 * b07) * det;
        out[5] = (a00 * b11 - a02 * b08 + a03 * b07) * det;
        out[6] = (a32 * b02 - a30 * b05 - a33 * b01) * det;
        out[7] = (a20 * b05 - a22 * b02 + a23 * b01) * det;
        out[8] = (a10 * b10 - a11 * b08 + a13 * b06) * det;
        out[9] = (a01 * b08 - a00 * b10 - a03 * b06) * det;
        out[10] = (a30 * b04 - a31 * b02 + a33 * b00) * det;
        out[11] = (a21 * b02 - a20 * b04 - a23 * b00) * det;
        out[12] = (a11 * b07 - a10 * b09 - a12 * b06) * det;
        out[13] = (a00 * b09 - a01 * b07 + a02 * b06) * det;
        out[14] = (a31 * b01 - a30 * b03 - a32 * b00) * det;
        out[15] = (a20 * b03 - a21 * b01 + a22 * b00) * det;
        
        return out;
    }
    
    function mat4_multiply(out, a, b) {
        let a00 = a[0]
        let a01 = a[1]
        let a02 = a[2]
        let a03 = a[3]
        let a10 = a[4]
        let a11 = a[5]
        let a12 = a[6]
        let a13 = a[7]
        let a20 = a[8]
        let a21 = a[9]
        let a22 = a[10]
        let a23 = a[11]
        let a30 = a[12]
        let a31 = a[13]
        let a32 = a[14]
        let a33 = a[15]
        
        // Cache only the current line of the second matrix
        let b0 = b[0]
        let b1 = b[1]
        let b2 = b[2]
        let b3 = b[3]
        out[0] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[1] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[2] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[3] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
        
        b0 = b[4];
        b1 = b[5];
        b2 = b[6];
        b3 = b[7];
        out[4] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[5] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[6] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[7] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
        
        b0 = b[8];
        b1 = b[9];
        b2 = b[10];
        b3 = b[11];
        out[8] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[9] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[10] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[11] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
        
        b0 = b[12];
        b1 = b[13];
        b2 = b[14];
        b3 = b[15];
        out[12] = b0 * a00 + b1 * a10 + b2 * a20 + b3 * a30;
        out[13] = b0 * a01 + b1 * a11 + b2 * a21 + b3 * a31;
        out[14] = b0 * a02 + b1 * a12 + b2 * a22 + b3 * a32;
        out[15] = b0 * a03 + b1 * a13 + b2 * a23 + b3 * a33;
        return out;
    }
    
    function mat4_translation(out, v) {
        out[0] = 1;
        out[1] = 0;
        out[2] = 0;
        out[3] = 0;
        out[4] = 0;
        out[5] = 1;
        out[6] = 0;
        out[7] = 0;
        out[8] = 0;
        out[9] = 0;
        out[10] = 1;
        out[11] = 0;
        out[12] = v[0];
        out[13] = v[1];
        out[14] = v[2];
        out[15] = 1;
        return out;
    }
    
    var wasm_instances = [];
    
    function init() {
        console.log("NOTICE! When profiling in chrome check 'Disable JavaScript Samples' under the gear icon. It slows the readings by a factor of 6-8x")
        for (let i = 0; i < canvasses.length; i ++) {
            // we found a canvas. instance the referenced wasm file
            var canvas = canvasses[i]
            let wasmfile = canvas.getAttribute("wasm");
            if (!wasmfile) continue
            fetch(wasmfile)
                .then(response => response.arrayBuffer())
                .then(bytes => WebAssembly.instantiate(bytes, {}))
                .then(results => {
                wasm_instances.push(
                    new WasmApp(canvas, results)
                );
            }, errors => {
                console.log("Error compiling wasm file");
            });
            // load this wasm file
        }
    }
    
    var canvasses = document.getElementsByClassName('cx_webgl')
    document.addEventListener('DOMContentLoaded', init)
    
    function fetch_path(file_path) {
        return new Promise(function(resolve, reject) {
            var req = new XMLHttpRequest()
            req.addEventListener("error", function() {
                reject(resource)
            })
            req.responseType = 'arraybuffer'
            req.addEventListener("load", function() {
                if (req.status !== 200) {
                    return reject(req.status)
                }
                resolve({
                    name: file_path,
                    buffer: req.response
                })
            })
            req.open("GET", file_path)
            req.send()
        })
    }
    
    function watchFileChange() {
        var req = new XMLHttpRequest()
        req.timeout = 60000
        req.addEventListener("error", function() {
            
            setTimeout(function() {
                location.href = location.href
            }, 500)
        })
        req.responseType = 'text'
        req.addEventListener("load", function() {
            if (req.status === 201) return watchFileChange();
            if (req.status === 200) {
                var msg = JSON.parse(req.response);
                if(msg.type == "file_change"){
                    location.href = location.href
                }
                if(msg.type == "build_start"){  
                    let note = "Rebuilding application..."
                    if(document.title != note){
                        document.title = note;
                        console.log(note);
                    }
                    watchFileChange();
                }
            }
        })
        req.open("GET", "/$watch?" + ('' + Math.random()).slice(2))
        req.send()
    }
    watchFileChange()
})({})