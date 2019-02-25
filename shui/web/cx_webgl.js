(function(root){

	// message we can send to wasm
	class WasmRecv{
		constructor(wasm_app){
			this.wasm_app = wasm_app;
			this.exports = wasm_app.exports;
			this.slots = 512;
			this.used = 1;
			// lets write 
			this.pointer = this.exports.wasm_alloc(this.slots);
			this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, this.slots);
			this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, this.slots);
		}

		fit(new_slots){
			if(this.used + new_slots > this.slots){
				let new_slots = Math.max(this.used+new_slots, this.slots * 2)
				this.pointer = this.exports.wasm_realloc(this.pointer, new_slots);
				this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, new_slots);
				this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, new_slots);
   				this.slots = new_slots
			}
			let pos = this.used;
			this.used += new_slots;
			return pos;
		}

		init(wasm_init){
			let pos = this.fit(4);
			this.mu32[pos++] = 1;
			this.mf32[pos++] = wasm_init.width;
			this.mf32[pos++] = wasm_init.height;
			this.mf32[pos++] = wasm_init.dpi_factor;
		}

		end(){
			let pos = this.fit(1);
			this.mu32[pos] = 0;
		}

	}

	class WasmApp{
		constructor(canvas, webasm){

			this.canvas = canvas;
			this.webasm = webasm;
			this.exports = webasm.instance.exports;
			this.memory = webasm.instance.exports.memory
			// lets get the webgl context
			this.init_webgl_context();
			this.shaders = [];

			// lets start
			this.app = this.exports.wasm_init();
			
			var to_wasm = new WasmRecv(this)
			to_wasm.init({
				width:this.width,
				height:this.height,
				dpi_factor:this.dpi_factor
			});
			to_wasm.end();

			this.next_recv = new WasmRecv(this);
			this.new_send_pointer( this.exports.wasm_recv(this.app, to_wasm.pointer))

			// object storage
			this.shaders = [];
			this.index_buffers = [];
			this.array_buffers = [];
			this.vaos = [];

			this.parse_send()
		}

		on_screen_resize(){
			var dpi_factor = window.devicePixelRatio;
			var w, h;
			var canvas = this.canvas;
	  		if(canvas.getAttribute("fullpage")){
				w = document.body.offsetWidth; 
				h = document.body.offsetHeight;
			}
			else{
				w = canvas.offsetWidth;
				h = canvas.offsetHeight;
			}
			var sw = canvas.width = w * dpi_factor;
			var sh = canvas.height = h * dpi_factor;
			canvas.style.width = w + 'px';
			canvas.style.height = h + 'px';
			this.gl.viewport(0,0,sw,sh);
	
			this.dpi_factor = dpi_factor;
			this.width = canvas.offsetWidth;
			this.height = canvas.offsetHeight;
			// send the wasm a screenresize event
		}

		new_send_pointer(send_pointer){
			this.send_pointer = send_pointer;
			this.parse = 1;
			this.mf32 = new Float32Array(this.memory.buffer, send_pointer);
			this.mu32 = new Uint32Array(this.memory.buffer, send_pointer);
		}

		parse_send(){
			this.basef32 = new Float32Array(this.memory.buffer);
			var send_fn_table = this.send_fn_table;
			while(1){
				let type = this.mu32[this.parse++];
				if(send_fn_table[type](this)){
					break;
				}
			}
		}
		
		parse_string(){
			var str = "";
			var len = this.mu32[this.parse++];
			for(let i = 0; i < len ; i++){
				var c = this.mu32[this.parse++];
				if(c != 0) str += String.fromCharCode(c);
			}
			return str
		}

		parse_shvarvec(){
			var len = this.mu32[this.parse++];
			var vars = []
			for(let i = 0; i < len; i++){
				vars.push({ty:this.parse_string(), name:this.parse_string()})
			}
			return vars
		}
		
		// new shader helpers
		get_attrib_locations(program, base, slots){
			var gl = this.gl;
			let attrib_locs = [];
			let attribs = slots >> 2;
			let stride = slots * 4;
			if((slots&3) != 0) attribs++;
			for(let i = 0; i < attribs; i++){
				let size = (slots - i*4);
				if(size > 4) size = 4;
				attrib_locs.push({
					loc:gl.getAttribLocation(program, base+i),
					offset:i * 16,
					size: size,
					stride:slots * 4
				});
			}
			return attrib_locs
		}

		get_uniform_locations(program, uniforms){
			var gl = this.gl;
			let uniform_locs = [];
			let offset = 0;
			for(let i = 0; i < uniforms.length; i++){
				let uniform = uniforms[i];
				uniform_locs.push({
					name:uniform.name,
					offset:offset,
					ty:uniform.ty,
					loc:gl.getUniformLocation(program, uniform.name),
					fn:this.uniform_fn_table[uniform.ty]
				});
				offset += this.uniform_size_table[uniform.ty]
			}
			return uniform_locs;
		}

		compile_webgl_shader(ash){
			var gl = this.gl
			var vsh = gl.createShader(gl.VERTEX_SHADER)
			gl.shaderSource(vsh, ash.vertex)
			gl.compileShader(vsh)
			if (!gl.getShaderParameter(vsh, gl.COMPILE_STATUS)){
				return console.log(
					gl.getShaderInfoLog(vsh), 
					add_line_numbers_to_string(ash.vertex)
				)
			}
			
			// compile pixelshader
			var fsh = gl.createShader(gl.FRAGMENT_SHADER)
			gl.shaderSource(fsh, ash.fragment)
			gl.compileShader(fsh)
			if (!gl.getShaderParameter(fsh, gl.COMPILE_STATUS)){
				return console.log(
					gl.getShaderInfoLog(fsh), 
					add_line_numbers_to_string(ash.fragment)
				)
			}
	
			var program = gl.createProgram()
			gl.attachShader(program, vsh)
			gl.attachShader(program, fsh)
			gl.linkProgram(program)
			if(!gl.getProgramParameter(program, gl.LINK_STATUS)){
				return console.log(
					gl.getProgramInfoLog(program),
					add_line_numbers_to_string(ash.vertex), 
					add_line_numbers_to_string(ash.fragment)
				)
			}
			// lets fetch all uniforms
			
 			this.shaders[ash.shader_id] = {
				geom_attribs:this.get_attrib_locations(program, "geomattr", ash.geometry_slots),
            	inst_attribs:this.get_attrib_locations(program, "instattr", ash.instance_slots),
				uniforms_cx:this.get_uniform_locations(program, ash.uniforms_cx),
				uniforms_dr:this.get_uniform_locations(program, ash.uniforms_dr),
				uniforms_dl:this.get_uniform_locations(program, ash.uniforms_dl),
				texture_slots:this.get_uniform_locations(program, ash.texture_slots),
				instance_slots:ash.instance_slots,
				program:program,
				ash:ash
			};
		}

		init_webgl_context(){
			window.addEventListener('resize', function(){
				this.on_screen_resize()
			}.bind(this))
			var canvas = this.canvas
			var options = {
				alpha: canvas.getAttribute("alpha")?true:false,
				depth: canvas.getAttribute("nodepth")?false:true,
				stencil: canvas.getAttribute("nostencil")?false:true,
				antialias: canvas.getAttribute("antialias")?true:false,
				premultipliedAlpha: canvas.getAttribute("premultipliedAlpha")?true:false,
				preserveDrawingBuffer: canvas.getAttribute("preserveDrawingBuffer")?true:false,
				preferLowPowerToHighPerformance: true
			}
	
			var gl = this.gl =  canvas.getContext('webgl', options) ||
					 canvas.getContext('webgl-experimental', options) ||
					 canvas.getContext('experimental-webgl', options)
	
			if(!gl){
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

		alloc_array_buffer(array_buffer_id, array){
			var gl = this.gl;
			let gl_buf = this.array_buffers[array_buffer_id] || gl.createBuffer()
			gl_buf.length = array.length;
			gl.bindBuffer(gl.ARRAY_BUFFER, gl_buf);
			gl.bufferData(gl.ARRAY_BUFFER, array, gl.STATIC_DRAW);
			this.array_buffers[array_buffer_id] = gl_buf;
		}

		alloc_index_buffer(index_buffer_id, array){
			var gl = this.gl;
			let gl_buf = this.index_buffers[index_buffer_id] || gl.createBuffer();
			gl_buf.length = array.length;
			gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, gl_buf);
			gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, array, gl.STATIC_DRAW);
			this.index_buffers[index_buffer_id] = gl_buf;
		}
		
		alloc_vao(shader_id, vao_id, geom_ib_id, geom_vb_id, inst_vb_id){
			let gl = this.gl;

			let shader = this.shaders[shader_id];

			let vao = gl.OES_vertex_array_object.createVertexArrayOES();
			this.vaos[vao_id] = vao

			vao.geom_ib_id = geom_ib_id;
			vao.geom_vb_id = geom_vb_id;
			vao.inst_vb_id = inst_vb_id;
			
			gl.OES_vertex_array_object.bindVertexArrayOES(vao)

			gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[geom_vb_id]);

			for(let i = 0; i < shader.geom_attribs.length; i++){
				let attr = shader.geom_attribs[i];
				gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
				gl.enableVertexAttribArray(attr.loc);
				gl.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 0);
			}

			gl.bindBuffer(gl.ARRAY_BUFFER, this.array_buffers[inst_vb_id]);
			for(let i = 0; i < shader.inst_attribs.length; i++){
				let attr = shader.inst_attribs[i];
				gl.vertexAttribPointer(attr.loc, attr.size, gl.FLOAT, false, attr.stride, attr.offset);
				gl.enableVertexAttribArray(attr.loc);
				gl.ANGLE_instanced_arrays.vertexAttribDivisorANGLE(attr.loc, 1);
			}
			
			gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.index_buffers[geom_ib_id]);
			//gl.OES_vertex_array_object.bindVertexArrayOES(0);

		}

		draw_call(shader_id, vao_id, uniforms_cx_ptr, uni_cx_update, uniforms_dl_ptr, uni_dl_update,
			uniforms_dr_ptr, uni_dr_update, textures){
			var gl = this.gl;

			let shader = this.shaders[shader_id];
			gl.useProgram(shader.program);
			
			let vao = this.vaos[vao_id];
			gl.OES_vertex_array_object.bindVertexArrayOES(vao);
			
			let index_buffer = this.index_buffers[vao.geom_ib_id];
			let instance_buffer = this.array_buffers[vao.inst_vb_id];
			// set up uniforms TODO do this a bit more incremental based on uniform layer
			let uniforms_cx = shader.uniforms_cx;
			for(let i = 0; i < uniforms_cx.length; i++){
				let uni = uniforms_cx[i];
				uni.fn(this, uni.loc, uni.offset + uniforms_cx_ptr);
			}
			let uniforms_dl = shader.uniforms_dl;
			for(let i = 0; i < uniforms_dl.length; i++){
				let uni = uniforms_dl[i];
				uni.fn(this, uni.loc, uni.offset + uniforms_dl_ptr);
			}
			let uniforms_dr = shader.uniforms_dr;
			for(let i = 0; i < uniforms_dr.length; i++){
				let uni = uniforms_dr[i];
				uni.fn(this, uni.loc, uni.offset + uniforms_dr_ptr);
			}
			let indices = index_buffer.length;
			let instances = instance_buffer.length / shader.instance_slots;
			// lets do a drawcall!
			gl.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
		}
		// now i have to do the drawcall

		clear(r,g,b,a){
			var gl = this.gl;
			gl.enable(gl.DEPTH_TEST);
			gl.depthFunc(gl.LEQUAL);
			gl.blendEquationSeparate(gl.FUNC_ADD, gl.FUNC_ADD);
			gl.blendFuncSeparate(gl.ONE, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
			gl.enable(gl.BLEND);
			gl.clearColor(r,g,b,a);
			gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);			
		}
	}

	WasmApp.prototype.send_fn_table = [
		function end_0(self){
			return true;
		},
		function log_1(self){
			console.log(self.parse_string());
		},
		function compile_webgl_shader_2(self){
			let ash = {
				shader_id: self.mu32[self.parse++],
				fragment: self.parse_string(),
				vertex: self.parse_string(),
				geometry_slots: self.mu32[self.parse++],
				instance_slots: self.mu32[self.parse++],
				uniforms_cx: self.parse_shvarvec(),
				uniforms_dl: self.parse_shvarvec(),
				uniforms_dr: self.parse_shvarvec(),
				texture_slots: self.parse_shvarvec()
			}
			self.compile_webgl_shader(ash);
		},
		function alloc_array_buffer_3(self){
			let array_buffer_id = self.mu32[self.parse++];
			let len = self.mu32[self.parse++];
			let pointer = self.mu32[self.parse++];
			let array = new Float32Array(self.memory.buffer, pointer, len);
			self.alloc_array_buffer(array_buffer_id, array);
		},
		function alloc_index_buffer_4(self){
			let index_buffer_id = self.mu32[self.parse++];
			let len = self.mu32[self.parse++];
			let pointer = self.mu32[self.parse++];
			let array = new Uint32Array(self.memory.buffer, pointer, len);
			self.alloc_index_buffer(index_buffer_id, array);
		},
		function alloc_vao_5(self){
			let shader_id = self.mu32[self.parse++];
			let vao_id = self.mu32[self.parse++];
			let geom_ib_id = self.mu32[self.parse++];
			let geom_vb_id = self.mu32[self.parse++];
			let inst_vb_id = self.mu32[self.parse++];
			self.alloc_vao(shader_id, vao_id, geom_ib_id, geom_vb_id, inst_vb_id)
		},
		function draw_call_6(self){
			let shader_id = self.mu32[self.parse++];
			let vao_id = self.mu32[self.parse++];
			let uniforms_cx_ptr = self.mu32[self.parse++];
			let uni_cx_update = self.mu32[self.parse++];
			let uniforms_dl_ptr = self.mu32[self.parse++];
			let uni_dl_update = self.mu32[self.parse++];
			let uniforms_dr_ptr = self.mu32[self.parse++];
			let uni_dr_update = self.mu32[self.parse++];
			let textures = self.mu32[self.parse++];
			self.draw_call(
				shader_id, vao_id, uniforms_cx_ptr, uni_cx_update, uniforms_dl_ptr, uni_dl_update,
				uniforms_dr_ptr, uni_dr_update, textures
			);
		},
		function clear_7(self){
			let r = self.mf32[self.parse++];
			let g = self.mf32[self.parse++];
			let b = self.mf32[self.parse++];
			let a = self.mf32[self.parse++];
			self.clear(r,g,b,a);
		}
	]
	
	WasmApp.prototype.uniform_fn_table = {
		"float":function set_float(self, loc, off){
			let slot = off>>2;
			self.gl.uniform1f(loc, self.basef32[slot])
		},
		"vec2":function set_vec2(self, loc, off){
			let slot = off>>2;
			let basef32 = self.basef32;
			self.gl.uniform2f(loc, basef32[slot], basef32[slot+1])
		},
		"vec3":function set_vec3(self, loc, off){
			let slot = off>>2;
			let basef32 = self.basef32;
			self.gl.uniform3f(loc, basef32[slot], basef32[slot+1], basef32[slot+2])
		},
		"vec4":function set_vec4(self, loc, off){
			let slot = off>>2;
			let basef32 = self.basef32;
			self.gl.uniform4f(loc, basef32[slot], basef32[slot+1], basef32[slot+2], basef32[slot+3])
		},
		"mat2":function set_mat2(self, loc, off){
			self.gl.uniformMatrix2fv(loc, false, new Float32Array(self.memory.buffer, off))
		},
		"mat3":function set_mat3(self, loc, off){
			self.gl.uniformMatrix3fv(loc, false, new Float32Array(self.memory.buffer, off))
		},
		"mat4":function set_mat4(self, loc, off){
			let mat4 = new Float32Array(self.memory.buffer, off, 16)
			self.gl.uniformMatrix4fv(loc, false, mat4)
		},
	};

	WasmApp.prototype.uniform_size_table = {
		"float":1,
		"vec2":2,
		"vec3":3,
		"vec4":4,
		"mat2":4,
		"mat3":9,
		"mat4":16
	}

	function add_line_numbers_to_string(code){
		var lines = code.split('\n')
		var out = ''
		for(let i = 0; i < lines.length; i++){
			out += (i+1)+': '+lines[i]+'\n'
		}
		return out	
	}

	var wasm_instances = [];

	function init(){
		for(let i = 0; i < canvasses.length; i++){
			// we found a canvas. instance the referenced wasm file
			var canvas = canvasses[i]
			let wasmfile = canvas.getAttribute("wasm");
			if(!wasmfile) continue
			fetch(wasmfile)
				.then(response => response.arrayBuffer())
				.then(bytes => WebAssembly.instantiate(bytes, {}))
				.then(results => {
					wasm_instances.push(
						new WasmApp(canvas, results)
					);
				});
			// load this wasm file
		}
	}

	root.isWindows = typeof navigator !== 'undefined' && navigator.appVersion.indexOf("Win") > -1
	root.isIPad = navigator.userAgent.match(/iPad/)
	root.isIOSDevice = navigator.userAgent.match(/(iPod|iPhone|iPad)/) && navigator.userAgent.match(/AppleWebKit/)
	root.isTouchDevice = ('ontouchstart' in window || navigator.maxTouchPoints)
	root.locationSearch = location.search

	var canvasses =	document.getElementsByClassName('cx_webgl')
	document.addEventListener('DOMContentLoaded', init)

	function watchFileChange(){
		var req = new XMLHttpRequest()
		req.timeout = 60000
		req.addEventListener("error", function(){

			setTimeout(function(){
				location.href = location.href
			}, 500)
		})
		req.responseType = 'text'
		req.addEventListener("load", function(){
			if(req.response === '{continue:true}') return watchFileChange()
			if(req.status === 200){
			// do something with data, or not
				location.href = location.href
			}
		})
		req.open("GET", "/$watch?"+(''+Math.random()).slice(2))
		req.send()
	}
	watchFileChange()
})({})