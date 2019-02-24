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

		ensure(new_slots){
			if(this.used + new_slots > this.slots){
				let new_slots = Math.max(this.used+new_slots, this.slots * 2)
				this.pointer = this.exports.wasm_realloc(this.pointer, new_slots);
				this.mf32 = new Float32Array(this.exports.memory.buffer, this.pointer, new_slots);
				this.mu32 = new Uint32Array(this.exports.memory.buffer, this.pointer, new_slots);
   				this.slots = new_slots
			}
			this.used += new_slots;
		}

		init(){
			let pos = this.used;
			this.ensure(1);
			this.mu32[pos] = 1;
		}

		end(){
			let pos = this.used;
			this.ensure(1);
			this.mu32[pos] = 0;
		}

	}

	class WasmInstance{
		constructor(canvas, webasm){
			this.canvas = canvas;
			this.webasm = webasm;
			this.exports = webasm.instance.exports;

			// lets start
			this.app = this.exports.wasm_init();
			
			var init_msg = new WasmRecv(this)

			init_msg.init();
			init_msg.end();
			this.current_msg = new WasmRecv(this);
			this.init_send( this.exports.wasm_recv(this.app, init_msg.pointer))
			// the current message
			this.parser = [
				function end_0(self){
					return true;
				},
				function log_1(self){
					var str = "";
					var len = self.mu32[self.parse++];
					for(var i = 0; i < len ; i++){
						str += String.fromCharCode(self.mu32[self.parse++]);
					}
					console.log(str);
				}
			]
			this.parse_send()
		}

		init_send(pointer){
			this.pointer = pointer;
			this.parse = 1;
			this.mf32 = new Float32Array(this.exports.memory.buffer, pointer);
			this.mu32 = new Uint32Array(this.exports.memory.buffer, pointer);
		}

		parse_send(){
			while(1){
				let type = this.mu32[this.parse++];
				if(this.parser[type](this)){
					break;
				}
			}
		}
	}

	var wasm_instances = [];

	function init(){
		for(let i = 0; i < canvasses.length; i++){
			// we found a canvas.
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