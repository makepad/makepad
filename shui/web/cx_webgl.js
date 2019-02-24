(function(root){
	function init(){
		for(let i = 0; i < canvasses.length; i++){
			// we found a canvas.
			var canvas = canvasses[i]
			let wasmfile = canvas.getAttribute("wasm");
			if(wasmfile){
				fetch(wasmfile)
					.then(response => response.arrayBuffer())
					.then(bytes => WebAssembly.instantiate(bytes, {}))
					.then(results => {
						// we have our wasm loaded
						let app = results.instance.exports.wasm_init();
						console.log(app)
					});
			}
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