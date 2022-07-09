import {WasmApp} from "/makepad/platform/wasm_bridge/src/wasm_app.js"
import {
    is_fullscreen,
    can_fullscreen,
    pack_key_modifier,
    web_cursor_map,
    fetch_path,
    add_line_numbers_to_string
} from "./webgl_util.js"

export class WebGLWasmApp extends WasmApp {
    constructor(wasm, dispatch, canvas) {
        super (wasm, dispatch);
        
        this.wasm_app = this.wasm_create_app();
        this.dispatch = dispatch;
        this.canvas = canvas;
        this.handlers = {};
        
        this.timers = [];
        
        this.text_copy_response = "";
        
        this.draw_shaders = [];
        this.array_buffers = [];
        this.index_buffers = [];
        this.vaos = [];
        this.textures = [];
        this.framebuffers = [];
        
        this.init_detection();
        this.bind_screen_resize();
        this.init_webgl_context();
        
        this.to_wasm = this.new_to_wasm();
        
        // alright lets send the fucker an init
        this.to_wasm.ToWasmGetDeps({
            gpu_info: this.gpu_info,
            browser_info: {
                protocol: location.protocol + "",
                hostname: location.hostname + "",
                pathname: location.pathname + "",
                search: location.search + "",
                hash: location.hash + "",
            }
        });
        
        this.do_wasm_pump();
        
        this.load_deps_promise.then(
            results => {
                let deps = [];
                for (let result of results) {
                    deps.push({
                        path: result.path,
                        data: result.buffer
                    })
                }
                this.to_wasm.ToWasmInit({
                    window_info: this.window_info,
                    deps: deps
                });
                this.do_wasm_pump();

                this.bind_mouse_and_touch();
                this.bind_keyboard();
                
                this.to_wasm.ToWasmRedrawAll();
                this.do_wasm_pump();
                
                var loaders = document.getElementsByClassName('canvas_loader');
                for (var i = 0; i < loaders.length; i ++) {
                    loaders[i].parentNode.removeChild(loaders[i])
                }
            },
            error => {
                console.error("Error loading dep", error)
            }
        )
    }
    
    
    // from_wasm dispatch_on_app interface
    
    
    
    FromWasmLoadDeps(args) {
        let promises = [];
        for (let path of args.deps) {
            promises.push(fetch_path("/makepad/", path))
        }
        
        this.load_deps_promise = Promise.all(promises);
    }
    
    FromWasmStartTimer(args) {
        let timer_id = args.timer_id;
        
        for (let i = 0; i < this.timers.length; i ++) {
            if (this.timers[i].timer_id == timer_id) {
                console.log("Timer ID collision!")
                return
            }
        }
        
        var timer = {timer_id, repeats: args.repeats};
        if (args.repeats !== 0) {
            timer.sys_id = window.setInterval(e => {
                this.to_wasm.ToWasmTimerFired({timer_id});
                this.do_wasm_pump();
            }, args.interval * 1000.0);
        }
        else {
            timer.sys_id = window.setTimeout(e => {
                for (let i = 0; i < this.timers.length; i ++) {
                    let timer = this.timers[i];
                    if (timer.timer_id == timer_id) {
                        this.timers.splice(i, 1);
                        break;
                    }
                }
                this.to_wasm.ToWasmTimerFired({timer_id});
                this.do_wasm_pump();
            }, args.interval * 1000.0);
        }
        this.timers.push(timer)
    }
    
    FromWasmStopTimer(args) {
        for (let i = 0; i < this.timers.length; i ++) {
            let timer = this.timers[i];
            if (timer.timer_id == args.timer_id) {
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
    }
    
    FromWasmFullScreen() {
        if (document.body.requestFullscreen) {
            document.body.requestFullscreen();
            return
        }
        if (document.body.webkitRequestFullscreen) {
            document.body.webkitRequestFullscreen();
            return
        }
        if (document.body.mozRequestFullscreen) {
            document.body.mozRequestFullscreen();
            return
        }
    }
    
    FromWasmNormalScreen() {
        if (this.canvas.exitFullscreen) {
            this.canvas.exitFullscreen();
            return
        }
        if (this.canvas.webkitExitFullscreen) {
            this.canvas.webkitExitFullscreen();
            return
        }
        if (this.canvas.mozExitFullscreen) {
            this.canvas.mozExitFullscreen();
            return
        }
    }

    FromWasmRequestAnimationFrame() {
        if (this.window_info.xr_is_presenting || this.req_anim_frame_id) {
            return;
        }
        this.req_anim_frame_id = window.requestAnimationFrame(time => {
            this.req_anim_frame_id = 0;
            if (this.xr_is_presenting) {
                return
            }
            this.to_wasm.ToWasmAnimationFrame({time: time / 1000.0});
            this.in_animation_frame = true;
            this.do_wasm_pump();
            this.in_animation_frame = false;
        })
    }
    
    FromWasmSetDocumentTitle(args) {
        document.title = args.title
    }

    FromWasmSetMouseCursor(args) {
        //console.log(args);
        document.body.style.cursor = web_cursor_map[args.web_cursor] || 'default'
    }

    FromWasmTextCopyResponse(args) {
        this.text_copy_response = args.response
    }

    FromWasmShowTextIME(args) {
        self.text_area_pos = args;
        this.update_text_area_pos();
    }
    
    FromWasmHideTextIME(){
        console.log("IMPLEMENTR!")
    }
    
    
    // webGL API
    
    
    FromWasmCompileWebGLShader(args) {
        function get_attrib_locations(gl, program, base, slots) {
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
        
        var gl = this.gl
        var vsh = gl.createShader(gl.VERTEX_SHADER)
        
        gl.shaderSource(vsh, args.vertex)
        gl.compileShader(vsh)
        if (!gl.getShaderParameter(vsh, gl.COMPILE_STATUS)) {
            return console.log(
                gl.getShaderInfoLog(vsh),
                add_line_numbers_to_string(args.vertex)
            )
        }
        
        // compile pixelshader
        var fsh = gl.createShader(gl.FRAGMENT_SHADER)
        gl.shaderSource(fsh, args.pixel)
        gl.compileShader(fsh)
        if (!gl.getShaderParameter(fsh, gl.COMPILE_STATUS)) {
            return console.log(
                gl.getShaderInfoLog(fsh),
                add_line_numbers_to_string(args.pixel)
            )
        }
        
        var program = gl.createProgram()
        gl.attachShader(program, vsh)
        gl.attachShader(program, fsh)
        gl.linkProgram(program)
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
            return console.log(
                gl.getProgramInfoLog(program),
                add_line_numbers_to_string(args.vertex),
                add_line_numbers_to_string(args.fragment)
            )
        }

        let texture_locs = [];
        for (let i = 0; i < args.textures.length; i ++) {
            texture_locs.push({
                name: args.textures[i].name,
                ty: args.textures[i].ty,
                loc: gl.getUniformLocation(program, "ds_"+args.textures[i].name),
            });
        }
        
        // fetch all attribs and uniforms
        this.draw_shaders[args.shader_id] = {
            vertex:args.vertex,
            pixel:args.pixel,
            geom_attribs: get_attrib_locations(gl, program, "packed_geometry_", args.geometry_slots),
            inst_attribs: get_attrib_locations(gl, program, "packed_instance_", args.instance_slots),
            pass_uniform: gl.getUniformLocation(program, "pass_table"),
            view_uniform: gl.getUniformLocation(program, "view_table"),
            draw_uniform: gl.getUniformLocation(program, "draw_table"),
            user_uniform: gl.getUniformLocation(program, "user_table"),
            live_uniform: gl.getUniformLocation(program, "live_table"),
            const_uniform: gl.getUniformLocation(program, "const_table"),
            texture_locs: texture_locs,
            geometry_slots: args.geometry_slots,
            instance_slots: args.instance_slots,
            program: program,
        };
    }
    
    FromWasmAllocIndexBuffer(args) {
        var gl = this.gl;
        
        let buf = this.index_buffers[args.buffer_id];
        if (buf === undefined) {
            buf = this.index_buffers[args.buffer_id] = {
                gl_buf: gl.createBuffer(),
            };
        }
        let array = new Uint32Array(this.memory.buffer, args.data.ptr, args.data.len);
        buf.length = array.length;
        
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, buf.gl_buf);
        gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, array, gl.STATIC_DRAW);
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, null);
    }
    
    FromWasmAllocArrayBuffer(args) {
        var gl = this.gl;
        
        let buf = this.array_buffers[args.buffer_id];
        if (buf === undefined) {
            buf = this.array_buffers[args.buffer_id] = {
                gl_buf: gl.createBuffer(),
            };
        }
        
        let array = new Float32Array(this.memory.buffer, args.data.ptr, args.data.len);
        buf.length = array.length;
        
        gl.bindBuffer(gl.ARRAY_BUFFER, buf.gl_buf);
        gl.bufferData(gl.ARRAY_BUFFER, array, gl.STATIC_DRAW);
        gl.bindBuffer(gl.ARRAY_BUFFER, null);
    }
    
    FromWasmAllocVao(args) {
        let gl = this.gl;
        let old_vao = this.vaos[args.vao_id];
        if (old_vao) {
            this.OES_vertex_array_object.deleteVertexArrayOES(old_vao.gl);
        }
        let gl_vao = this.OES_vertex_array_object.createVertexArrayOES();
        let vao = this.vaos[args.vao_id] = {
            gl_vao: gl_vao,
            geom_ib_id: args.geom_ib_id,
            geom_vb_id: args.geom_vb_id,
            inst_vb_id: args.inst_vb_id
        };
        
        this.OES_vertex_array_object.bindVertexArrayOES(vao.gl_vao)
        gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[args.geom_vb_id].gl_buf);
        
        let shader = this.draw_shaders[args.shader_id];
        
        for (let i = 0; i < shader.geom_attribs.length; i ++) {
            let attr = shader.geom_attribs[i];
            if (attr.loc < 0) {
                continue;
            }
            gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
            gl.enableVertexAttribArray(attr.loc);
            this.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 0);
        }
        
        gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[args.inst_vb_id].gl_buf);
        
        for (let i = 0; i < shader.inst_attribs.length; i ++) {
            let attr = shader.inst_attribs[i];
            if (attr.loc < 0) {
                continue;
            }
            gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
            gl.enableVertexAttribArray(attr.loc);
            this.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 1);
        }
        
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.index_buffers[args.geom_ib_id].gl_buf);
        this.OES_vertex_array_object.bindVertexArrayOES(null);
    }
    
    
    
    
    FromWasmDrawCall(args) {
        var gl = this.gl;

        let shader = this.draw_shaders[args.shader_id];
        
        gl.useProgram(shader.program);
        
        let vao = this.vaos[args.vao_id];
        
        this.OES_vertex_array_object.bindVertexArrayOES(vao.gl_vao);
        
        let index_buffer = this.index_buffers[vao.geom_ib_id];
        let instance_buffer = this.array_buffers[vao.inst_vb_id];
        // if vr_presenting
        // TODO CACHE buffers
        if(args.const_table.ptr != 0) gl.uniform1fv(shader.const_uniform, new Float32Array(this.memory.buffer, args.const_table.ptr, args.const_table.len));
        if(args.pass_uniforms.ptr != 0) gl.uniform1fv(shader.pass_uniform, new Float32Array(this.memory.buffer, args.pass_uniforms.ptr, args.pass_uniforms.len));
        if(args.view_uniforms.ptr != 0) gl.uniform1fv(shader.view_uniform, new Float32Array(this.memory.buffer, args.view_uniforms.ptr, args.view_uniforms.len));
        if(args.draw_uniforms.ptr != 0) gl.uniform1fv(shader.draw_uniform, new Float32Array(this.memory.buffer, args.draw_uniforms.ptr, args.draw_uniforms.len));
        if(args.user_uniforms.ptr != 0) gl.uniform1fv(shader.user_uniform, new Float32Array(this.memory.buffer, args.user_uniforms.ptr, args.user_uniforms.len));
        if(args.live_uniforms.ptr != 0) gl.uniform1fv(shader.live_uniform, new Float32Array(this.memory.buffer, args.live_uniforms.ptr, args.live_uniforms.len));
        
        let texture_slots = shader.texture_locs.length;
        for (let i = 0; i < texture_slots; i ++) {
            let tex_loc = shader.texture_locs[i];
            let texture_id = args.textures[i]
            if (texture_id !== undefined) {
                let tex_obj = this.textures[texture_id];
                gl.activeTexture(gl.TEXTURE0 + i);
                gl.bindTexture(gl.TEXTURE_2D, tex_obj);
                gl.uniform1i(tex_loc.loc, i);
            }
        }
        
        let indices = index_buffer.length;
        let instances = instance_buffer.length / shader.instance_slots;

        this.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
        this.OES_vertex_array_object.bindVertexArrayOES(null);
    }
    
    
    FromWasmAllocTextureImage2D(args){
        var gl = this.gl;
        var gl_tex = this.textures[texture_id] || gl.createTexture()
        
        gl.bindTexture(gl.TEXTURE_2D, gl_tex)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
        
        let data_array = new Uint8Array(this.memory.buffer, args.data.ptr, args.width * args.height * 4);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, args.width, args.height, 0, gl.RGBA, gl.UNSIGNED_BYTE, data_array);

        this.textures[texture_id] = gl_tex;
    }
    
    FromWasmBeginRenderTexture(args){
        let gl = this.gl

        var gl_framebuffer = this.framebuffers[args.pass_id] || (this.framebuffers[args.pass_id] = gl.createFramebuffer());
        gl.bindFramebuffer(gl.FRAMEBUFFER, gl_framebuffer);
        
        let clear_flags = 0;
        let clear_depth = 0.0;
        let clear_color;
        
        for(let i = 0; i < args.color_targets.length; i++){
            let tgt = args.color_targets[i];
            
            var gl_tex = this.textures[tgt.texture_id] || (this.textures[tgt.texture_id] = gl.createTexture());
            // resize or create texture
            if (gl_tex.mp_width != args.width || gl_tex.mp_height != this.args.height) {
                gl.bindTexture(gl.TEXTURE_2D, gl_tex)
                
                clear_flags |= gl.COLOR_BUFFER_BIT;
                clear_color = tgt.clear_color;
                
                gl_tex.mp_width = args.width
                gl_tex.mp_height = args.height
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
                
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl_tex.mp_width, gl_tex.mp_height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
            }
            else if (!tgt.init_only) {
                clear_flags |= gl.COLOR_BUFFER_BIT;
            }
            
            gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, gl_tex, 0)
        }
        // TODO implement depth target
        gl.viewport(0, 0, this.target_width, this.target_height);
        
        if (clear_flags !== 0) {
            gl.clearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
            gl.clearDepth(clear_depth);
            gl.clear(clear_flags);
        }
    }
    
    FromWasmBeginRenderCanvas(args) {
        let gl = this.gl
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.viewport(0, 0, this.canvas.width, this.canvas.height);
        let c = args.clear_color;
        gl.clearColor(c.r, c.g, c.b, c.a);
        gl.clearDepth(args.depth);
        gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    }

    FromWasmSetDefaultDepthAndBlendMode() {
        let gl = this.gl
        gl.disable(gl.DEPTH_TEST);
        gl.depthFunc(gl.LEQUAL);
        gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
        gl.blendFuncSeparate(gl.ONE, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
        gl.enable(gl.BLEND);
    }
    
    
    
    // calling into wasm
    
    
    wasm_create_app() {
        let new_ptr = this.exports.wasm_create_app();
        this.update_array_buffer_refs();
        return new_ptr
    }
    
    wasm_process_msg(to_wasm) {
        let ret_ptr = this.exports.wasm_process_msg(to_wasm.release_ownership(), this.wasm_app)
        this.update_array_buffer_refs();
        return this.new_from_wasm(ret_ptr);
    }
    
    do_wasm_pump() {
        let to_wasm = this.to_wasm;
        this.to_wasm = this.new_to_wasm();
        let from_wasm = this.wasm_process_msg(to_wasm);
        from_wasm.dispatch_on_app();
        from_wasm.free();
    }
    
    
    // init and setup
    
    
    init_detection() {
        this.detect = {
            user_agent: window.navigator.userAgent,
            is_mobile_safari: window.navigator.platform.match(/iPhone|iPad/i),
            is_touch_device: ('ontouchstart' in window || navigator.maxTouchPoints),
            is_firefox: navigator.userAgent.toLowerCase().indexOf('firefox') > -1,
            use_touch_scroll_overlay: window.ontouchstart === null
        }
        this.detect.is_android = this.detect.user_agent.match(/Android/i)
        this.detect.is_add_to_homescreen_safari = this.is_mobile_safari && navigator.standalone
    }
    
    init_webgl_context() {
        let mqString = '(resolution: ' + window.devicePixelRatio + 'dppx)'
        let mq = matchMedia(mqString);
        if (mq && mq.addEventListener) {
            mq.addEventListener("change", this.handlers.on_screen_resize);
        }
        else { // poll for it. yes. its terrible
            window.setInterval(_ => {
                if (window.devicePixelRation != this.dpi_factor) {
                    this.handlers.on_screen_resize();
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
            preferLowPowerToHighPerformance: true,
            //xrCompatible: true
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
        
        this.OES_standard_derivatives = gl.getExtension('OES_standard_derivatives')
        this.OES_vertex_array_object = gl.getExtension('OES_vertex_array_object')
        this.OES_element_index_uint = gl.getExtension("OES_element_index_uint")
        this.ANGLE_instanced_arrays = gl.getExtension('ANGLE_instanced_arrays')
        
        // check uniform count
        var max_vertex_uniforms = gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS);
        var max_fragment_uniforms = gl.getParameter(gl.MAX_FRAGMENT_UNIFORM_VECTORS);
        
        this.gpu_info = {
            min_uniforms: Math.min(max_vertex_uniforms, max_fragment_uniforms),
            vendor: "unknown",
            renderer: "unknown"
        }
        let debug_info = gl.getExtension('WEBGL_debug_renderer_info');
        
        if (debug_info) {
            this.gpu_info.vendor = gl.getParameter(debug_info.UNMASKED_VENDOR_WEBGL);
            this.gpu_info.renderer = gl.getParameter(debug_info.UNMASKED_RENDERER_WEBGL);
        }
        
        //gl.EXT_blend_minmax = gl.getExtension('EXT_blend_minmax')
        //gl.OES_texture_half_float_linear = gl.getExtension('OES_texture_half_float_linear')
        //gl.OES_texture_float_linear = gl.getExtension('OES_texture_float_linear')
        //gl.OES_texture_half_float = gl.getExtension('OES_texture_half_float')
        //gl.OES_texture_float = gl.getExtension('OES_texture_float')
        //gl.WEBGL_depth_texture = gl.getExtension("WEBGL_depth_texture") || gl.getExtension("WEBKIT_WEBGL_depth_texture")
        this.handlers.on_screen_resize()
    }
    
    bind_screen_resize() {
        this.window_info = {};
        
        this.handlers.on_screen_resize = () => {
            var dpi_factor = window.devicePixelRatio;
            var w;
            var h;
            var canvas = this.canvas;
            
            if (this.window_info.xr_is_presenting) {
                let xr_webgllayer = this.xr_session.renderState.baseLayer;
                this.dpi_factor = 3.0;
                this.width = 2560.0 / this.dpi_factor;
                this.height = 2000.0 / this.dpi_factor;
            }
            else {
                if (canvas.getAttribute("fullpage")) {
                    if (this.detect.is_add_to_homescreen_safari) { // extremely ugly. but whatever.
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
                
                this.window_info.dpi_factor = dpi_factor;
                this.window_info.inner_width = canvas.offsetWidth;
                this.window_info.inner_height = canvas.offsetHeight;
                // send the wasm a screenresize event
            }
            this.window_info.is_fullscreen = is_fullscreen();
            this.window_info.can_fullscreen = can_fullscreen();
            
            if (this.to_wasm !== undefined) {
                this.to_wasm.ToWasmResizeWindow({window_info:this.window_info});
                this.FromWasmRequestAnimationFrame();
            }
        }
        
        // TODO! BIND THESE SOMEWHERE USEFUL
        this.handlers.on_app_got_focus = () => {
            this.to_wasm.ToWasmAppGotFocus();
            this.do_wasm_pump();
        }
        
        this.handlers.on_app_lost_focus = () => {
            this.to_wasm.ToWasmAppGotFocus();
            this.do_wasm_pump();
        }
        
        window.addEventListener('resize', _ => this.handlers.on_screen_resize())
        window.addEventListener('orientationchange', _ => this.handlers.on_screen_resize())
    }
    
    bind_mouse_and_touch() {
        
        var canvas = this.canvas
        
        
        let last_mouse_finger;
        if (this.detect.use_touch_scroll_overlay) {
            var ts = this.touch_scroll_overlay = document.createElement('div')
            ts.className = "makepad_webgl_scroll_overlay"
            var ts_inner = document.createElement('div')
            var style = document.createElement('style')
            style.innerHTML = "\n"
                + "div.makepad_webgl_scroll_overlay {\n"
                + "z-index: 10000;\n"
                + "margin:0;\n"
                + "overflow:scroll;\n"
                + "top:0;\n"
                + "left:0;\n"
                + "width:100%;\n"
                + "height:100%;\n"
                + "position:fixed;\n"
                + "background-color:transparent\n"
                + "}\n"
                + "div.cx_webgl_scroll_overlay div{\n"
                + "margin:0;\n"
                + "width:400000px;\n"
                + "height:400000px;\n"
                + "background-color:transparent\n"
                + "}\n"
            
            document.body.appendChild(style)
            ts.appendChild(ts_inner);
            document.body.appendChild(ts);
            canvas = ts;
            
            ts.scrollTop = 200000;
            ts.scrollLeft = 200000;
            let last_scroll_top = ts.scrollTop;
            let last_scroll_left = ts.scrollLeft;
            let scroll_timeout = null;
            
            this.handlers.on_overlay_scroll = e => {
                let new_scroll_top = ts.scrollTop;
                let new_scroll_left = ts.scrollLeft;
                let dx = new_scroll_left - last_scroll_left;
                let dy = new_scroll_top - last_scroll_top;
                last_scroll_top = new_scroll_top;
                last_scroll_left = new_scroll_left;
                
                window.clearTimeout(scroll_timeout);
                
                scroll_timeout = window.setTimeout(_ => {
                    ts.scrollTop = 200000;
                    ts.scrollLeft = 200000;
                    last_scroll_top = ts.scrollTop;
                    last_scroll_left = ts.scrollLeft;
                }, 200);
                
                let finger = last_mouse_finger;
                if (finger) {
                    finger.is_touch = false;
                    this.to_wasm.ToWasmFingerScroll({
                        finger: finger,
                        scroll_x: dx,
                        scroll_y: dy
                    });
                    this.do_wasm_pump();
                }
            }
            
            ts.addEventListener('scroll', e => this.handlers.on_overlay_scroll(e))
        }
        
        var mouse_fingers = [];
        function mouse_to_finger(e) {
            let mf = mouse_fingers[e.button] || (mouse_fingers[e.button] = {});
            mf.x = e.pageX;
            mf.y = e.pageY;
            mf.digit = e.button;
            mf.time = e.timeStamp / 1000.0;
            mf.modifiers = pack_key_modifier(e);
            mf.touch = false;
            return mf
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
        
        this.handlers.on_mouse_down = e => {
            e.preventDefault();
            this.focus_keyboard_input();
            mouse_buttons_down[e.button] = true;
            this.to_wasm.ToWasmFingerDown({finger: mouse_to_finger(e)});
            this.do_wasm_pump();
        }
        
        this.handlers.on_mouse_up = e => {
            e.preventDefault();
            mouse_buttons_down[e.button] = false;
            this.to_wasm.ToWasmFingerUp({finger: mouse_to_finger(e)});
            this.do_wasm_pump();
        }
        
        this.handlers.on_mouse_move = e => {
            document.body.scrollTop = 0;
            document.body.scrollLeft = 0;
            
            for (var i = 0; i < mouse_buttons_down.length; i ++) {
                if (mouse_buttons_down[i]) {
                    let mf = mouse_to_finger(e);
                    mf.digit = i;
                    this.to_wasm.ToWasmFingerMove({finger: mouse_to_finger(e)});
                }
            }
            last_mouse_finger = mouse_to_finger(e);
            this.to_wasm.ToWasmFingerHover({finger: last_mouse_finger});
            this.do_wasm_pump();
            //console.log("Redraw cycle "+(end-begin)+" ms");
        }
        
        this.handlers.on_mouse_out = e => {
            this.to_wasm.ToWasmFingerOut({finger: mouse_to_finger(e)});
            this.do_wasm_pump();
        }
        
        canvas.addEventListener('mousedown', e => this.handlers.on_mouse_down(e))
        window.addEventListener('mouseup', e => this.handlers.on_mouse_up(e))
        window.addEventListener('mousemove', e => this.handlers.on_mouse_move(e));
        window.addEventListener('mouseout', e => this.handlers.on_mouse_out(e));
        
        this.handlers.on_contextmenu = e => {
            e.preventDefault()
            return false
        }
        
        canvas.addEventListener('contextmenu', e => this.handlers.on_contextmenu(e))
        
        this.handlers.on_touchstart = e => {
            e.preventDefault()
            
            let fingers = touch_to_finger_alloc(e);
            for (let i = 0; i < fingers.length; i ++) {
                this.to_wasm.ToWasmFingerDown({finger: fingers[i]});
            }
            this.do_wasm_pump();
            return false
        }
        
        this.handlers.on_touchmove = e => {
            //e.preventDefault();
            var fingers = touch_to_finger_lookup(e);
            for (let i = 0; i < fingers.length; i ++) {
                this.to_wasm.ToWasmFingerMove({finger: fingers[i]});
            }
            this.do_wasm_pump();
            return false
        }
        
        this.handlers.on_touch_end_cancel_leave = e => {
            e.preventDefault();
            var fingers = touch_to_finger_free(e);
            for (let i = 0; i < fingers.length; i ++) {
                this.to_wasm.ToWasmFingerUp({finger: fingers[i]});
            }
            this.do_wasm_pump();
            return false
        }
        
        canvas.addEventListener('touchstart', e => this.handlers.on_touchstart(e))
        canvas.addEventListener('touchmove', e => this.handlers.on_touchmove(e), {passive: false})
        canvas.addEventListener('touchend', e => this.handlers.on_touch_end_cancel_leave(e));
        canvas.addEventListener('touchcancel', e => this.handlers.on_touch_end_cancel_leave(e));
        canvas.addEventListener('touchleave', e => this.handlers.on_touch_end_cancel_leave(e));
        
        var last_wheel_time;
        var last_was_wheel;
        this.handlers.on_mouse_wheel = e => {
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
            
            finger.is_touch = !last_was_wheel;
            this.to_wasm.ToWasmFingerScroll({
                finger: finger,
                scroll_x: e.deltaX * fac,
                scroll_y: e.deltaY * fac
            });
            this.do_wasm_pump();
        };
        canvas.addEventListener('wheel', e => this.handlers.on_mouse_wheel(e))
    }
    
    bind_keyboard() {
        if (this.detect.is_mobile_safari || this.detect.is_android) { // mobile keyboards are unusable on a UI like this. Not happening.
            return
        }
        
        var ta = this.text_area = document.createElement('textarea')
        ta.className = "cx_webgl_textinput"
        ta.setAttribute('autocomplete', 'off')
        ta.setAttribute('autocorrect', 'off')
        ta.setAttribute('autocapitalize', 'off')
        ta.setAttribute('spellcheck', 'false')
        var style = document.createElement('style')
        
        style.innerHTML = "\n"
            + "textarea.cx_webgl_textinput {\n"
            + "z-index: 1000;\n"
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
            + "textarea: focus.cx_webgl_textinput {\n"
            + "outline: 0px !important;\n"
            + "-webkit-appearance: none;\n"
            + "}"
        
        document.body.appendChild(style)
        ta.style.left = -100 + 'px'
        ta.style.top = -100 + 'px'
        ta.style.height = 1
        ta.style.width = 1
        
        //document.addEventListener('focusout', this.onFocusOut.bind(this))
        var was_paste = false;
        this.neutralize_ime = false;
        var last_len = 0;
        
        this.handlers.on_cut = e => {
            setTimeout(_ => {
                ta.value = "";
                last_len = 0;
            }, 0)
        }
        
        ta.addEventListener('cut', e => this.handlers.on_cut(e));
        
        this.handlers.on_copy = e => {
            setTimeout(_ => {
                ta.value = "";
                last_len = 0;
            }, 0)
        }
        
        ta.addEventListener('copy', e => this.handlers.on_copy(e));
        
        this.handlers.on_paste = e => {
            was_paste = true;
        }
        
        ta.addEventListener('paste', e => this.handlers.on_paste(e));
        
        this.handlers.on_select = e => {}
        
        ta.addEventListener('select', e => this.handlers.on_select(e))
        
        this.handlers.on_input = e => {
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
                        this.to_wasm.ToWasmTextInput({
                            was_paste: false,
                            input: text_value,
                            replace_last: replace_last,
                        });
                    }
                }
                this.do_wasm_pump();
            }
            last_len = ta.value.length;
        };
        ta.addEventListener('input', e => this.handlers.on_input(e));
        
        ta.addEventListener('mousedown', e => this.handlers.on_mouse_down(e));
        ta.addEventListener('mouseup', e => this.handlers.on_mouse_up(e));
        ta.addEventListener('wheel', e => this.handlers.on_mouse_wheel(e));
        
        ta.addEventListener('contextmenu', e => this.handlers.on_contextmenu(e));
        
        ta.addEventListener('blur', e => {
            this.focus_keyboard_input();
        })
        
        var ugly_ime_hack = false;
        
        this.handlers.on_keydown = e => {
            let code = e.keyCode;
            
            //if (code == 91) {firefox_logo_key = true; e.preventDefault();}
            if (code == 18 || code == 17 || code == 16) e.preventDefault(); // alt
            if (code === 8 || code === 9) e.preventDefault() // backspace/tab
            if ((code === 88 || code == 67) && (e.metaKey || e.ctrlKey)) { // copy or cut
                // we need to request the clipboard
                this.to_wasm.ToWasmTextCopy();
                this.do_wasm_pump();
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
            this.to_wasm.ToWasmKeyDown({key: {
                key_code: key_code,
                char_code: e.charCode,
                is_repeat: e.repeat,
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e)
            }})
            
            this.do_wasm_pump();
        };
        
        ta.addEventListener('keydown', e => this.handlers.on_keydown(e));
        
        this.handlers.on_keyup = e => {
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
            this.to_wasm.ToWasmKeyUp({key: {
                key_code: e.keyCode,
                char_code: e.charCode,
                is_repeat: e.repeat,
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e)
            }})
            this.do_wasm_pump();
        };
        ta.addEventListener('keyup', e => this.handlers.on_keyup(e));
        document.body.appendChild(ta);
        ta.focus();
    }
    
    
    // internal helper api
    
    
    update_text_area_pos() {
        var pos = this.text_area_pos;
        var ta = this.text_area;
        if (ta && pos) {
            ta.style.left = (Math.round(pos.x) - 4) + "px";
            ta.style.top = Math.round(pos.y) + "px"
        }
    }
    
    
    focus_keyboard_input() {
        this.text_area.focus();
    }
    
}

