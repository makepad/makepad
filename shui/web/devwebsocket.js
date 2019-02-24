// devwebsocket is MIT licensed, copyright Makepad
let Url = require('url')
let Crypto = require('crypto')
let Http = require('http')

function NodeWebSocket(request, socket, header){
	if(arguments.length === 1){
		this.initClient(request)
	}
	if(arguments.length === 3){
		this.initServer(request, socket, header)
	}
}

module.exports = NodeWebSocket

var proto = NodeWebSocket.prototype

proto.onMessage = function(){
}

proto.onOpen = function(){	
}

proto.onClose = function(){
}

proto.onError = function(){
}

proto.initClient = function(clientUrl){
	let url = this.clientUrl = Url.parse(clientUrl)

	let socketHost = url.hostname+':'+client
	let socketKey = Buffer.from('13-'+Date.now()).toString('base64')
	let sha1 = Crypto.creatHash('sha1')
	sha1.update(socketKey+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
	let expectHeader = sha1.digest('base64')

	let httpOpts = {
		port:url.port,
		host:url.hostname,
		path:url.path,
		headers:{
			'connection':'Upgrade',
			'upgrade':'websocket',
			'pragma':'no-cache',
			'host':socketHost,
			'origin':'http://'+socketHost,
			'sec-websocket-version':13,
			'sec-websocket-key':socketKey
		}
	}

	let request = http.request(httpOpts)

	request.on('error', function(e){
		console.log("Node Websocket Client error "+e)
		this.onError(e)
	})

	request.on('response', function(){
		console.error("Unexpected response")
	})

	request.on('upgrade', function(response, socket, header){
		if(response.headers['sec-websocket-accept'] !== expectHeader){
			console.log("Node Websocket Client error, unexpected accept ")
			if(this.onError) this.onError("unexpected accept")
		}
		this.socket = socket
		this.initialize()

		this.onOpen()
	})

	request.end()

}

proto.initServer = function(request, socket){

	let version = request.headers['sec-websocket-version']
	if(version != 13){
		console.log("Node Websocket incompatible socket version header (need 13)")
		return socket.destroy()
	}

	this.socket = socket

	let socketKey = request.headers['sec-websocket-key']
	let sha1 = Crypto.createHash('sha1')
	sha1.update(socketKey + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
	let serverAck = 'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+
		sha1.digest('base64')+'\r\n\r\n'

	socket.write(serverAck)
	this.initialize()
	
	let pingFrame = Buffer.alloc(2)
	pingFrame[0] = 9|128
	pingFrame[1] = 0

	this.pingInterval = setInterval(function(){
		if(!this.socket) clearInterval(this.pingInterval)
		else this.socket.write(pingFrame)
	}.bind(this), 10000)
}

proto.initialize = function(){
	this.maxBuffer = 10000000
	this.headerBuf = Buffer.alloc(14)
	this.outputBuf = Buffer.alloc(10000000)
	this.parseState = this.parseOpcode
	this.bytesExpected = 1
	this.bytesWritten = 0
	this.bytesRead = 0
	this.partialString = ''
	this.inputBuf = null
	this.maskOffset = 0
	this.maskCount = 0
	this.bytesPayload = 0
	this.readyState = 1

	this.socket.on('data', function(data){
		this.inputBuf = data
		this.bytesRead = 0
		while(this.parseState());
	}.bind(this))

	this.socket.on('close', function(){
		this.close()
	}.bind(this))
}

proto.close = function(){
	if(!this.socket) return
	this.onClose()
	if(this.pingInterval) clearInterval(this.pingInterval)
	this.socket.destroy()
	this.readyState = 3
	this.socket = undefined
}

proto.error = function(t){
	this.onError(t)
	this.close()
}

proto.send = function(data, partial, continuation, masking){
	if(!this.socket) return

	let head
	let buf = Buffer.alloc(data.length, data)
	let buflen = buf.length

	if(buflen < 126){
		head = Buffer.alloc(2)
		head[1] = buflen		
	}
	else if(buflen < 65536){
		head = Buffer.alloc(4)
		head[1] = 126
		head.writeUInt16BE(buflen, 2)
	}
	else{
		head = Buffer.alloc(10)
		head[1] = 127
		head[2] = head[3] = head[4] = head[5] = 0
		head.writeUInt32BE(buflen, 6)
	}
	let type = 1
	if(data instanceof Buffer || data instanceof ArrayBuffer) type = 2
	if(continuation) type = 0
	head[0] = (partial?0:128) | type

	this.socket.write(head)

	if(masking){
		head[1] |= 128
		let mask = Buffer.alloc(4)
		mask[0] = 0x7f
		mask[1] = 0x7f
		mask[2] = 0x7f
		mask[3] = 0x7f
		this.socket.write(mask)
		for(var i = 0; i < buflen; i++){
			buf[i] ^= mask[i&3]
		}
	}

	this.socket.write(buf)
}

proto.parseOpcode = function(){

	if(this.parseHead()) return false
	let frame = this.headerBuf[0] & 128
	let type = this.headerBuf[0] & 15

	if(type <= 2){
		this.isPartial = !frame
		this.isBinary = type ===2 || type === 0 && this.lastType === 2
		this.bytesExpected = 1
		this.parseState = this.parseLen1
		if(type) this.lastType = 2
		return true
	}
	if(type == 8) return this.close()
	if(type == 9){
		this.bytesExpected = 1
		this.parseState = this.parsePing
		return true
	}
	if(type == 10){
		this.bytesExpected = 1
		this.parseState = this.parsePong
		return true
	}
	return this.error("Opcode not supported "+type)
}

proto.parseHead = function(){
	let exp = this.bytesExpected
	while(this.bytesExpected > 0 && this.bytesRead < this.inputBuf.length && this.bytesWritten < this.headerBuf.length){
		this.headerBuf[this.bytesWritten++] = this.inputBuf[this.bytesRead++]
		this.bytesExpected--
	}
	if(this.bytesWritten > this.headerBuf.length) return this.error("Unexpected data in header")
	return this.bytesExpected != 0
}

proto.parseData = function(){
	if(this.isMasked){
		while(this.bytesExpected > 0 && this.bytesRead < this.inputBuf.length){
			this.outputBuf[this.bytesWritten++] = this.inputBuf[this.bytesRead++] ^ this.headerBuf[this.maskOffset+(this.maskCount++&3)]
			this.bytesExpected--
		}
	}
	else{
		if(this.bytesWritten > this.outputBuf.length) return this.error("Output buffer overflow")
		while(this.bytesExpected > 0 && this.bytesRead < this.inputBuf.length){
			this.outputBuf[this.bytesWritten++] = this.inputBuf[this.bytesRead++]
			this.bytesExpected--
		}
	}

	if(this.bytesExpected) return false

	if(this.isBinary){ // we received a binary message
		if(this.isPartial){
			console.log("IMPLEMENT BINARY")
		}
		else{
			console.log("IMPLEMENT BINARY")
		}
	}
	else{
		let msg = this.outputBuf.toString('utf8',0,this.bytesWritten)
		if(!this.isPartial){
			this.onMessage({
				data:this.partialString + msg
			})
			this.partialString = ''
		}
		else this.partialString += msg
	}
	
	this.bytesWritten = 0
	this.bytesExpected = 1
	this.parseState = this.parseOpcode
	return true
}

proto.parseMask = function(){
	if(this.parseHead()) return false
	if(!this.bytesPayload){
		this.bytesExpected = 1
		this.bytesWritten = 0
		this.parseState = this.parseOpcode
		return true
	}
	this.maskOffset = this.bytesWritten -4
	this.bytesWritten = 0
	this.maskCount = 0
	this.bytesExpected = this.bytesPayload

	if(this.bytesPayload > this.maxBuffer) return this.error("Socket buffer size too large")
	if(this.bytesPayload > this.outputBuf.length) this.outputBuf = Buffer.alloc(this.bytesPayload)
	this.parseState = this.parseData		
	return true
}

proto.parseLen8 = function(){
	if(this.parseHead()) return false
	this.bytesPayload = this.headerBuf.readUInt32BE(this.bytesWritten - 4)
	if(this.isMasked){
		this.bytesExpected = 4
		this.parseState = this.parseMask		
	}
	else{
		this.bytesExpected = this.bytesPayload
		this.parseState = this.parseData
		this.bytesWritten = 0
	}
	return true
}

proto.parseLen2 = function(){
	if(this.parseHead()) return false
	this.bytesPayload = this.headerBuf.readUInt16BE(this.bytesWritten - 2)
	if(this.isMasked){
		this.bytesExpected = 4
		this.parseState = this.parseMask
	}
	else{
		this.bytesExpected = this.bytesPayload
		this.parseState = this.parseData
		this.bytesWritten = 0
	}
	return true
}

proto.parseLen1 = function(){
	if(this.parseHead()) return false

	this.isMasked = this.headerBuf[this.bytesWritten-1]&128
	let type = this.headerBuf[this.bytesWritten-1]&127
	if(type < 126){
		this.bytesPayload = type
		if(!this.isMasked){
			this.bytesExpected = this.bytesPayload
			this.parseState = this.parseData
			this.bytesWritten = 0
		}
		else{
			this.bytesExpected = 4
			this.parseState = this.parseMask
		}
	}
	else if(type == 126){
		this.bytesExpected = 2
		this.parseState = this.parseLen2
	}
	else if(type == 127){
		this.bytesExpected = 8
		this.parseState = this.parseLen8
	}
	return true
}

let pongFrame = Buffer.alloc(2)
pongFrame[0] = 10|128
pongFrame[1] = 0

proto.parsePing = function(){
	if(this.parseHead()) return false
	if(this.headerBuf[this.bytesWritten - 1] & 128){
		this.bytesExpected = 4
		this.bytesPayload = 0
		this.parseState = this.parseMask	
		return true
	}
	this.bytesExpected = 1
	this.bytesWritten = 0
	this.parseState = this.parseOpcode
	this.socket.write(pongFrame)
	return true
}

proto.parsePong = function(){
	if(this.parseHead()) return false
	if(this.headerBuf[this.bytesWritten - 1] & 128){
		this.bytesExpected = 4
		this.bytesPayload = 0
		this.parseState = this.parseMask
		return true
	}
	this.bytesExpected = 1
	this.bytesWritten = 0
	this.parseState = this.parseOpcode
	return true
}
