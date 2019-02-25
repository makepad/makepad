(function(root){

	// message we can send to wasm
	class WasmRecv{
		constructor(wasm_instance){
			this.wasm_instance = wasm_instance;
			this.exports = wasm_instance.exports;
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

		init(){
			let pos = this.fit(1);
			this.mu32[pos] = 1;
		}

		end(){
			let pos = this.fit(1);
			this.mu32[pos] = 0;
		}

	}

	class WasmInstance{
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
			to_wasm.init();
			to_wasm.end();

			this.current_msg = new WasmRecv(this);
			this.new_send_pointer( this.exports.wasm_recv(this.app, to_wasm.pointer))
			// the current message
			this.shaders = [];
			this.parse_send()
		}

		on_screen_resize(){
			var pixelRatio = window.devicePixelRatio;
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
			var sw = canvas.width = w * pixelRatio;
			var sh = canvas.height = h * pixelRatio;
			canvas.style.width = w + 'px';
			canvas.style.height = h + 'px';
			this.gl.viewport(0,0,sw,sh);
	
			this.pixel_ratio = window.devicePixelRatio;
			this.width = canvas.offsetWidth;
			this.height = canvas.offsetHeigh;
			// send the wasm a screenresize event
		}

		new_send_pointer(send_pointer){
			this.send_pointer = send_pointer;
			this.parse = 1;
			this.mf32 = new Float32Array(this.memory.buffer, send_pointer);
			this.mu32 = new Uint32Array(this.memory.buffer, send_pointer);
		}

		parse_send(){
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

			this.shaders[ash.shader_id] = {
				program:program,
				ash:ash
			};
		}

		init_webgl_context(){
			window.addEventListener('resize', function(){
				this.onScreenResize()
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
	}

	WasmInstance.prototype.send_fn_table = [
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
		}
	]

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
						new WasmInstance(canvas, results)
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