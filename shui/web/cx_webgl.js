(function(root){
	var user_agent = window.navigator.userAgent;
	var is_mobile_safari = user_agent.match(/Mobile\/\w+ Safari/i);

	// message we can send to wasm
	class ToWasm{
		constructor(wasm_app){
			this.wasm_app = wasm_app;
			this.exports = wasm_app.exports;
			this.slots = 512;
			this.used = 2; // skip 8 byte header
			// lets write 
			this.pointer = this.exports.alloc_wasm_buffer(this.slots * 4);
			this.update_refs();
		}

		update_refs(){
			this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, this.slots);
			this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, this.slots);
			this.mf64 = new Float64Array(this.exports.memory.buffer, this.pointer, this.slots>>1);
		}

		fit(slots){
			this.update_refs(); // its possible our mf32/mu32 refs are dead because of realloccing of wasm heap inbetween calls
			if(this.used + slots > this.slots){
				let new_slots = Math.max(this.used + slots, this.slots * 2) // exp alloc algo
				if(new_slots&1)new_slots++; // float64 align it
				let new_bytes = new_slots * 4;
				this.pointer = this.exports.realloc_wasm_buffer(this.pointer, new_bytes); // by
				this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, new_slots);
				this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, new_slots);
				this.mf64 =  new Float64Array(this.exports.memory.buffer, this.pointer, new_slots>>1);
   				this.slots = new_slots
			}
			let pos = this.used;
			this.used += slots;
			return pos;
		}

		fetch_deps(){
			let pos = this.fit(1);
			this.mu32[pos++] = 1;
		}

		send_string(str){
			let pos = this.fit(str.length+1)
			this.mu32[pos++] = str.length
			for(let i = 0; i < str.length; i++){
				this.mu32[pos++] = str.charCodeAt(i)
			}
		}

		deps_loaded(deps){
			let pos = this.fit(2);
			this.mu32[pos++] = 2
			this.mu32[pos++] = deps.length
			for(let i = 0; i < deps.length; i++){
				let dep = deps[i];
				this.send_string(dep.name);
				pos = this.fit(1);
				this.mu32[pos++] = dep.ptr
			}
		}

		init(info){
			let pos = this.fit(4);
			this.mu32[pos++] = 3;
			this.mf32[pos++] = info.width;
			this.mf32[pos++] = info.height;
			this.mf32[pos++] = info.dpi_factor;
		}

		resize(info){
			let pos = this.fit(4);
			this.mu32[pos++] = 4;
			this.mf32[pos++] = info.width;
			this.mf32[pos++] = info.height;
			this.mf32[pos++] = info.dpi_factor;
		}

		animation_frame(time){
			let pos = this.fit(3); // float64 uses 2 slots
			this.mu32[pos++] = 5;
			if(pos&1){ // float64 align, need to fit another
				pos++;
				this.fit(1);
			}
			this.mf64[pos>>1] = time;
		}

		on_finger_down(finger){
			let pos = this.fit(5);
			this.mu32[pos++] = 6;
			this.mf32[pos++] = finger.x
			this.mf32[pos++] = finger.y
			this.mu32[pos++] = finger.digit
			this.mu32[pos++] = finger.touch?1:0
		}

		on_finger_up(finger){
			let pos = this.fit(5);
			this.mu32[pos++] = 7;
			this.mf32[pos++] = finger.x
			this.mf32[pos++] = finger.y
			this.mu32[pos++] = finger.digit
			this.mu32[pos++] = finger.touch?1:0
		}

		on_finger_move(finger){
			let pos = this.fit(5);
			this.mu32[pos++] = 8;
			this.mf32[pos++] = finger.x
			this.mf32[pos++] = finger.y
			this.mu32[pos++] = finger.digit
			this.mu32[pos++] = finger.touch?1:0
		}

		on_finger_hover(finger){
			let pos = this.fit(3);
			this.mu32[pos++] = 9;
			this.mf32[pos++] = finger.x
			this.mf32[pos++] = finger.y
		}
		
		on_finger_wheel(finger){
			let pos = this.fit(5);
			this.mu32[pos++] = 10;
			this.mf32[pos++] = finger.x
			this.mf32[pos++] = finger.y
			this.mf32[pos++] = finger.x_wheel
			this.mf32[pos++] = finger.y_wheel
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
			
			// local webgl resources
			this.shaders = [];
			this.index_buffers = [];
			this.array_buffers = [];
			this.vaos = [];
			this.textures = [];
			this.resources = [];
			this.req_anim_frame_id = 0;

			this.init_webgl_context();
			this.bind_mouse_and_touch();

			// lets create the wasm app and cx
			this.app = this.exports.create_wasm_app();
			
			// create initial to_wasm
			this.to_wasm = new ToWasm(this);

			// fetch dependencies
			this.to_wasm.fetch_deps();

			this.do_wasm_io();

			// ok now, we wait for our resources to load.
			Promise.all(this.resources).then(results=>{
				let deps = []

				// copy our reslts into wasm pointers
				for(let i = 0; i < results.length; i++){
					var result = results[i]
					// allocate pointer, do +8 because of the u64 length at the head of the buffer
					let output_ptr = this.exports.alloc_wasm_buffer(result.buffer.byteLength+8);
					let array = new Uint32Array(this.memory.buffer, output_ptr);
					this.copy_to_wasm(result.buffer, output_ptr);
					deps.push({
						name:result.name,
						ptr:output_ptr
					});
				}
				// pass wasm the deps
				this.to_wasm.deps_loaded(deps);
				// initialize the application
				this.to_wasm.init({
					width:this.width,
					height:this.height,
					dpi_factor:this.dpi_factor
				})
				
				this.do_wasm_io();
			})
		}

		do_wasm_io(){
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
			while(1){
				let msg_type = this.mu32[this.parse++];
				if(send_fn_table[msg_type](this)){
					break;
				}
			}
			// destroy from_wasm_ptr object
			this.exports.dealloc_wasm_buffer(from_wasm_ptr);
		}

		request_animation_frame(){
			if (this.req_anim_frame_id){
				return;
			}
			this.req_anim_frame_id = window.requestAnimationFrame(time=>{
				this.req_anim_frame_id = 0;
				this.to_wasm.animation_frame(time / 1000.0);
				this.do_wasm_io();
			})
			this.requ
		}

		// i forgot how to do memcpy with typed arrays. so, we'll do this.
		copy_to_wasm(input_buffer, output_ptr){
			let len = input_buffer.byteLength;
			let f64len = len>>3; //8 bytes at a time. 
			var f64out = new Float64Array(this.memory.buffer, output_ptr + 8, f64len)
			var f64in = new Float64Array(input_buffer);
			for(let i = 0; i < f64len; i++){
				f64out[i] = f64in[i];
			}
			let u8len = len&7;
			if(u8len){ // copy remainder
				let u8off = len - u8len;
				var u8out = new Uint8Array(this.memory.buffer, output_ptr + 8 + u8off, len)
				var u8in = new Uint8Array(input_buffer, u8off);
				for(let i = 0; i < u8len; i++){
					u8out[i] = u8in[i];
				}
			}
		}

		on_screen_resize(){
			var dpi_factor = window.devicePixelRatio;
			var w, h;
			var canvas = this.canvas;

			if(canvas.getAttribute("fullpage")){
				w = window.innerWidth;
				h = window.innerHeight;
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
			if(this.to_wasm){
				// initialize the application
				this.to_wasm.resize({
					width:this.width,
					height:this.height,
					dpi_factor:this.dpi_factor
				})
				this.request_animation_frame()
			}
		}
		
		load_deps(deps){
			for(var i = 0; i < deps.length; i++){
				let file_path = deps[i];
				this.resources.push(new Promise(function(resolve, reject){
					var req = new XMLHttpRequest()
					req.addEventListener("error", function(){
						reject(resource)
					})
					req.responseType = 'arraybuffer'
					req.addEventListener("load", function(){
						if(req.status !== 200){
							return reject(req.status)
						}
						resolve({
							name:file_path,
							buffer:req.response
						})
					})
					req.open("GET", file_path)
					req.send()
				}))
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
					offset:offset<<2,
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

			// fetch all attribs and uniforms
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
			window.addEventListener('resize', _=>{
				this.on_screen_resize()
			})

			window.addEventListener('orientationchange', _=>{
				console.log("HERE");
				this.on_screen_resize()
			})
			
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

		set_document_title(title){
			document.title = title
		}

		on_finger_down(fingers){
			for(let i = 0; i < fingers.length; i++){
				var finger = fingers[i]	
					this.to_wasm.on_finger_down(finger)
			}

			this.do_wasm_io();
		}

		on_finger_up(fingers){
			for(let i = 0; i < fingers.length; i++){
				var finger = fingers[i]
				this.to_wasm.on_finger_up(finger)
			}

			this.do_wasm_io();
		}

		on_finger_hover(fingers){
			for(let i = 0; i < fingers.length; i++){
				var finger = fingers[i]
				this.to_wasm.on_finger_hover(finger);
			}			

			this.do_wasm_io();
		}

		on_finger_move(fingers){
			for(let i = 0; i < fingers.length; i++){
				var finger = fingers[i]
				this.to_wasm.on_finger_move(finger);
			}
			this.do_wasm_io();
		}

		on_finger_wheel(fingers){
			for(let i = 0; i < fingers.length; i++){
				var finger = fingers[i]
				this.to_wasm.on_finger_wheel(finger)
			}
			this.do_wasm_io();
		}
		
		bind_mouse_and_touch(){
			var canvas = this.canvas
			function mouse_to_finger(e){
				return [{
					x:e.pageX,
					y:e.pageY,
					digit: e.button,
					touch: false
				}]
			}

			var digit_map = {}
			var digit_alloc = 0;

			function touch_to_finger_alloc(e){
				var f = []
				for(let i = 0; i < e.changedTouches.length; i++){
					var t = e.changedTouches[i]

					// find an unused digit
					var digit = undefined;
					for(digit in digit_map){
						if(!digit_map[digit]) break
					}
					// we need to alloc a new one
					if(digit === undefined || digit_map[digit]) digit = digit_alloc++;
					// store it					
					digit_map[digit] = {x:t.pageX, y:t.pageY};
					// return allocated digit
					digit = parseInt(digit);

					f.push({
						x:t.pageX,
						y:t.pageY,
						digit:digit,
						touch: true,
					})
				}
				return f
			}

			function lookup_digit(x, y){
				var near_dist = Infinity, near_digit = -1
				for(let digit in digit_map){
					var digit_pos = digit_map[digit]
					if(!digit_pos) continue
					var dx = x - digit_pos.x, dy = y - digit_pos.y
					var len = Math.sqrt(dx*dx+dy*dy)
					if(len < near_dist) near_dist = len, near_digit = digit
				}
				return near_digit;
			}

			function touch_to_finger_lookup(e){
				var f = []
				for(let i = 0; i < e.changedTouches.length; i++){
					var t = e.changedTouches[i]
					f.push({
						x:t.pageX,
						y:t.pageY,
						digit:lookup_digit(t.pageX, t.pageY),
						touch: true,
					})
				}
				return f
			}

			function touch_to_finger_free(e){
				var f = []
				for(let i = 0; i < e.changedTouches.length; i++){
					var t = e.changedTouches[i]
					var digit = lookup_digit(t.pageX, t.pageY)
					if(!digit){
						console.log("Undefined state in free_digit");
						digit = 0
					}
					else{
						digit_map[digit] = undefined
					}
					
					f.push({
						x:t.pageX,
						y:t.pageY,
						digit:digit,
						touch: true,
					})
				}
				return f
			}
		
			var mouse_buttons_down = [];
			canvas.addEventListener('mousedown',e=>{
				e.preventDefault();
				mouse_buttons_down[e.button] = true;
				this.on_finger_down(mouse_to_finger(e))
			})
			window.addEventListener('mouseup',e=>{
				e.preventDefault();
				mouse_buttons_down[e.button] = false;
				this.on_finger_up(mouse_to_finger(e))
			})
			let mouse_move = e=>{
				let move_fingers = [];
				for(var i = 0; i < mouse_buttons_down.length; i++){
					if(mouse_buttons_down[i]){
						move_fingers.push({
							x:e.pageX,
							y:e.pageY,
							digit:i
						})
					}
				}
				this.on_finger_move(move_fingers);
				this.on_finger_hover(mouse_to_finger(e));
			}
			window.addEventListener('mousemove',mouse_move);
			window.addEventListener('mouseout',mouse_move);
			canvas.addEventListener('contextmenu',e=>{
				e.preventDefault()
				return false
			})
			window.addEventListener('touchstart', e=>{
				e.preventDefault()
				this.on_finger_down(touch_to_finger_alloc(e))
				return false
			})
			window.addEventListener('touchmove',e=>{
				e.preventDefault();
				this.on_finger_move(touch_to_finger_lookup(e))
				return false
			},{passive:false})
			window.addEventListener('touchend', e=>{
				e.preventDefault();
				this.on_finger_up(touch_to_finger_free(e))
				return false
			})
			canvas.addEventListener('touchcancel', e=>{
				e.preventDefault();
				this.on_finger_up(touch_to_finger_free(e))
				return false
			})
			canvas.addEventListener('touchleave', e=>{
				e.preventDefault();
				this.on_finger_up(touch_to_finger_free(e))
				return false
			})
			canvas.addEventListener('wheel', e=>{
				var fingers = mouse_to_finger(e)
				e.preventDefault()
				var fac = 1
				if(e.deltaMode === 1) fac = 40
				else if(e.deltaMode === 2) fac = window.offsetHeight
		
				fingers[0].x_wheel = e.deltaX * fac
				fingers[0].y_wheel = e.deltaY * fac
				this.on_finger_wheel(fingers)
			})
			//window.addEventListener('webkitmouseforcewillbegin', this.onCheckMacForce.bind(this), false)
			//window.addEventListener('webkitmouseforcechanged', this.onCheckMacForce.bind(this), false)
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

		alloc_texture(texture_id, width, height, data_ptr){
			var gl = this.gl;
			var gl_tex = this.textures[texture_id] || gl.createTexture()

			gl.bindTexture(gl.TEXTURE_2D, gl_tex)
			gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR)
			gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR)
			gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
			gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
			
			let data = new Uint8Array(this.memory.buffer, data_ptr, width*height*4);
			gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
			//gl.bindTexture(gl.TEXTURE_2D,0);
			this.textures[texture_id] = gl_tex;
		}
		
		alloc_vao(shader_id, vao_id, geom_ib_id, geom_vb_id, inst_vb_id){
			let gl = this.gl;

			let shader = this.shaders[shader_id];

			let old_vao =this.vaos[vao_id];
			if(old_vao){
				gl.OES_vertex_array_object.deleteVertexArrayOES(old_vao);
			}
			let vao = gl.OES_vertex_array_object.createVertexArrayOES();
			this.vaos[vao_id] = vao;

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
			uniforms_dr_ptr, uni_dr_update, textures_ptr){
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
			let texture_slots = shader.texture_slots;
			for(let i = 0; i < texture_slots.length; i++){
				let tex_slot = texture_slots[i];
				let tex_id = this.baseu32[(textures_ptr>>2)+i];
				let tex_obj = this.textures[tex_id];
				gl.activeTexture(gl.TEXTURE0+i);
				gl.bindTexture(gl.TEXTURE_2D, tex_obj);
				gl.uniform1i(tex_slot.loc, i);
			}

			let indices = index_buffer.length;
			let instances = instance_buffer.length / shader.instance_slots;
			// lets do a drawcall!
			gl.ANGLE_instanced_arrays.drawElementsInstancedANGLE(gl.TRIANGLES, indices, gl.UNSIGNED_INT, 0, instances);
		}

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

	// array of function id's wasm can call on us, self is pointer to WasmApp
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
		},
		function load_deps_8(self){
			let deps = []
			let num_deps = self.mu32[self.parse++];
			for(let i = 0; i < num_deps; i++){
				deps.push(self.parse_string());
			}
			self.load_deps(deps);
		},
		function alloc_texture_9(self){
			let texture_id = self.mu32[self.parse++];
			let width = self.mu32[self.parse++];
			let height = self.mu32[self.parse++];
			let data_ptr = self.mu32[self.parse++];
			self.alloc_texture(texture_id, width, height, data_ptr);
		},
		function request_animation_frame_10(self){
			self.request_animation_frame()
		},
		function set_document_title_11(self){
			self.set_document_title(self.parse_string())
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
			self.gl.uniformMatrix2fv(loc, false, new Float32Array(self.memory.buffer, off, 4))
		},
		"mat3":function set_mat3(self, loc, off){
			self.gl.uniformMatrix3fv(loc, false, new Float32Array(self.memory.buffer, off, 9))
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