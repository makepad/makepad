import {WasmWebBrowser} from "./web.js";

export class WasmWebGL extends WasmWebBrowser {
    constructor(wasm, dispatch, canvas) {
        super (wasm, dispatch, canvas);
        if (wasm === undefined) {
            return
        }
        this.draw_shaders = [];
        this.array_buffers = [];
        this.index_buffers = [];
        this.vaos = [];
        this.textures = [];
        this.framebuffers = [];
        this.xr = undefined;
        this.init_webgl_context();
        
        this.load_deps();
        
    }
    
    
    // webGL API
    
    on_xr_animation_frame(time, frame) {
        function empty_transform() {
            return {
                orientation: {
                    a: 0,
                    b: 0,
                    c: 0,
                    d: 0,
                },
                position: {
                    x: 0,
                    y: 0,
                    z: 0,
                }
            }
        }
        
        function to_transform(pose_transform, tgt) {
            let po = pose_transform.inverse.orientation;
            let pp = pose_transform.position;
            let o = tgt.orientation;
            o.a = po.x
            o.b = po.y
            o.c = po.z
            o.d = po.w
            let p = tgt.position;
            p.x = pp.x
            p.y = pp.y
            p.z = pp.z
        }
        
        function get_matrices(layer, view, tgt){
            tgt.view = view;
            tgt.viewport = layer.getViewport(view);
            tgt.projection_matrix = view.projectionMatrix;
            tgt.transform_matrix = view.transform.inverse.matrix;
            tgt.invtransform_matrix = view.transform.matrix;
            tgt.camera_pos = view.transform.inverse.position;
        }
        
        if (this.xr == undefined) {
            return
        }
        
        let ref_space = this.xr.ref_space;
        let xr = this.xr;
        
        xr.session.requestAnimationFrame(this.xr.on_animation_frame);
        xr.pose = frame.getViewerPose(ref_space);
            
        let left_view = xr.pose.views[0];
        let right_view = xr.pose.views[1];
        
        get_matrices(xr.layer, xr.pose.views[0], xr.left_eye)
        get_matrices(xr.layer, xr.pose.views[1], xr.right_eye)

        if (xr.xr_update === undefined) {
            xr.xr_update = {
                time: 0,
                head_transform: empty_transform(),
                inputs: []
            }
        }
        
        let xr_update = xr.xr_update;
        xr_update.time = time / 1000.0;
        
        to_transform(this.xr.pose.transform, xr_update.head_transform);
        
        let inputs = xr_update.inputs;
        for (let i = 0; i < inputs.length; i ++) {
            inputs[i].active = false;
        }
        
        let input_sources = this.xr.session.inputSources;
        for (let i = 0; i < input_sources.length; i ++) {
            if (inputs[i] === undefined) {
                inputs[i] = {active: false, grip: empty_transform(), ray: empty_transform(), hand: 0, buttons: [], axes: []};
            }
            let input = inputs[i];
            let input_source = input_sources[i];
            
            let grip_pose = frame.getPose(input_source.gripSpace, ref_space);
            let ray_pose = frame.getPose(input_source.targetRaySpace, ref_space);
            
            if (grip_pose == null || ray_pose == null) {
                input.active = false;
                continue;
            }
            
            to_transform(grip_pose.transform, input.grip)
            to_transform(ray_pose.transform, input.ray)
            
            let buttons = input.buttons;
            let input_buttons = input_source.gamepad.buttons;
            for (let i = 0; i < input_buttons.length; i ++) {
                if (buttons[i] === undefined) {
                    buttons[i] = {pressed: 0, value: 0}
                };
                buttons[i].pressed = input_buttons[i].pressed? 1: 0;
                buttons[i].value = input_buttons[i].value
            }
            let axes = input.axes;
            let input_axes = input_source.gamepad.axes;
            for (let i = 0; i < input_axes.length; i ++) {
                axes[i] = input_axes[i];
            }
        }
        
        this.to_wasm.ToWasmXRUpdate(xr_update)
        this.to_wasm.ToWasmAnimationFrame({time: time / 1000.0});
        this.in_animation_frame = true;
        this.do_wasm_pump();
        this.in_animation_frame = false;
    }
    
    FromWasmXrStartPresenting(args) {
        if (this.xr !== undefined) {
            console.log("XR already presenting")
            return
        }
        // alright lets fire up the xr stuff
        navigator.xr.requestSession('immersive-vr', {requiredFeatures: ['local-floor']}).then(session => {
            let layer = new XRWebGLLayer(session, this.gl, {
                antialias: false,
                depth: true,
                stencil: false,
                ignoreDepthValues: false,
                framebufferScaleFactor: 1.5
            });
            session.updateRenderState({baseLayer: layer});
            session.requestReferenceSpace("local-floor").then(ref_space => {
                window.localStorage.setItem("xr_presenting", "true");
                this.xr = {
                    left_eye: {},
                    right_eye: {},
                    layer,
                    ref_space,
                    session,
                    on_animation_frame: (t, f) => this.on_xr_animation_frame(t, f)
                }
                session.requestAnimationFrame(this.xr.on_animation_frame);
                session.addEventListener("end", () => {
                    window.localStorage.setItem("xr_presenting", "false");
                    this.xr = undefined;
                    this.FromWasmRequestAnimationFrame();
                })
            })
        })
    }
    
    FromWasmXrStopPresenting() {
        
    }
    
    
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
       //console.log(args.pixel)
        let texture_locs = [];
        for (let i = 0; i < args.textures.length; i ++) {
            texture_locs.push({
                name: args.textures[i].name,
                ty: args.textures[i].ty,
                loc: gl.getUniformLocation(program, "ds_" + args.textures[i].name),
            });
        }
        
        // fetch all attribs and uniforms
        this.draw_shaders[args.shader_id] = {
            vertex: args.vertex,
            pixel: args.pixel,
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
        gl.uniform1fv(shader.view_uniform, new Float32Array(this.memory.buffer, args.view_uniforms.ptr, args.view_uniforms.len));
        gl.uniform1fv(shader.draw_uniform, new Float32Array(this.memory.buffer, args.draw_uniforms.ptr, args.draw_uniforms.len));
        
        if (args.user_uniforms.ptr != 0) gl.uniform1fv(shader.user_uniform, new Float32Array(this.memory.buffer, args.user_uniforms.ptr, args.user_uniforms.len));
        if (args.live_uniforms.ptr != 0) gl.uniform1fv(shader.live_uniform, new Float32Array(this.memory.buffer, args.live_uniforms.ptr, args.live_uniforms.len));
        if (args.const_table.ptr != 0) gl.uniform1fv(shader.const_uniform, new Float32Array(this.memory.buffer, args.const_table.ptr, args.const_table.len));

        let indices = index_buffer.length;
        let instances = instance_buffer.length / shader.instance_slots;
        
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
        
        let xr = this.xr;
        if (xr !== undefined && xr.in_xr_pass) {
            let pass_uniforms = new Float32Array(this.memory.buffer, args.pass_uniforms.ptr, args.pass_uniforms.len);

            let left = xr.left_eye;
            let lvp = left.viewport;
            gl.viewport(lvp.x, lvp.y, lvp.width, lvp.height);
            let mlp = left.projection_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i] = mlp[i];
            let mlt = left.transform_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i + 16] = mlt[i];
            let mli = left.invtransform_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i + 32] = mli[i];
            gl.uniform1fv(shader.pass_uniform, pass_uniforms);
            this.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
            
            let right = xr.right_eye;
            let rvp = right.viewport;
            gl.viewport(rvp.x, rvp.y, rvp.width, rvp.height);
            let mrp = right.projection_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i] = mrp[i];
            let mrt = right.transform_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i + 16] = mrt[i];
            let mri = right.invtransform_matrix;
            for(let i = 0; i < 16; i++) pass_uniforms[i + 32] = mri[i];
            gl.uniform1fv(shader.pass_uniform, pass_uniforms);
            this.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
        }
        else {
            gl.uniform1fv(shader.pass_uniform, new Float32Array(this.memory.buffer, args.pass_uniforms.ptr, args.pass_uniforms.len));
            this.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
        }
        
        this.OES_vertex_array_object.bindVertexArrayOES(null);
    }
    
    
    FromWasmAllocTextureImage2D(args) {
        var gl = this.gl;
        var gl_tex = this.textures[args.texture_id] || gl.createTexture()
        
        gl.bindTexture(gl.TEXTURE_2D, gl_tex)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
        //gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
        let data_array = new Uint8Array(this.memory.buffer, args.data.ptr, args.width * args.height * 4);
        //agdconsole.log(args.width, args.height);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, args.width, args.height, 0, gl.RGBA, gl.UNSIGNED_BYTE, data_array);
        this.textures[args.texture_id] = gl_tex;
    }
    
    FromWasmBeginRenderTexture(args) {
        if(this.xr !== undefined){
            this.xr.in_xr_pass = false;
        }

        let gl = this.gl
        var gl_framebuffer = this.framebuffers[args.pass_id] || (this.framebuffers[args.pass_id] = gl.createFramebuffer());
        gl.bindFramebuffer(gl.FRAMEBUFFER, gl_framebuffer);
        
        let clear_flags = 0;
        let clear_depth = 0.0;
        let clear_color;
        
        for (let i = 0; i < args.color_targets.length; i ++) {
            let tgt = args.color_targets[i];
            
            var gl_tex = this.textures[tgt.texture_id] || (this.textures[tgt.texture_id] = gl.createTexture());
            // resize or create texture
            clear_color = tgt.clear_color;
            if (gl_tex._width != args.width || gl_tex._height != args.height) {
                gl.bindTexture(gl.TEXTURE_2D, gl_tex)
                
                clear_flags |= gl.COLOR_BUFFER_BIT;
                
                gl_tex._width = args.width
                gl_tex._height = args.height
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
                gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl_tex._width, gl_tex._height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
            }
            else if (!tgt.init_only) {
                clear_flags |= gl.COLOR_BUFFER_BIT;
            }
            
            gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, gl_tex, 0)
        }
        // TODO implement depth target
        gl.viewport(0, 0, args.width, args.height);
        
        if (clear_flags !== 0) {
            gl.clearColor(clear_color.r, clear_color.g, clear_color.b, clear_color.a);
            gl.clearDepth(clear_depth);
            gl.clear(clear_flags);
        }
    }
    
    FromWasmBeginRenderCanvas(args) {
        let gl = this.gl
        let xr = this.xr;
        
        if(xr !== undefined){
            xr.in_xr_pass = true;
            gl.bindFramebuffer(gl.FRAMEBUFFER, xr.layer.framebuffer);
            gl.viewport(0, 0, xr.layer.framebufferWidth, xr.layer.framebufferHeight);            
        }
        else{
            gl.bindFramebuffer(gl.FRAMEBUFFER, null);
            gl.viewport(0, 0, this.canvas.width, this.canvas.height);
        }
        let c = args.clear_color;
        gl.clearColor(c.r, c.g, c.b, c.a);
        gl.clearDepth(args.depth);
        gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    }
    
    FromWasmSetDefaultDepthAndBlendMode() {
        let gl = this.gl
        gl.disable(gl.DEPTH_TEST);
        gl.depthFunc(gl.GEQUAL);
        gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
        gl.blendFuncSeparate(gl.ONE, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
        gl.enable(gl.BLEND);
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
    }
    
}

function add_line_numbers_to_string(code) {
    var lines = code.split('\n')
    var out = ''
    for (let i = 0; i < lines.length; i ++) {
        out += (i + 1) + ': ' + lines[i] + '\n'
    }
    return out
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
