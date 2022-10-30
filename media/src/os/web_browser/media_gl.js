import {WasmWebGL} from "/makepad/platform/src/os/web_browser/web_gl.js"

export class WasmMediaGL extends WasmWebGL {
    constructor(wasm, dispatch, canvas) {
        super (wasm, dispatch, canvas);
    }
    
    FromWasmSpawnAudioOutput(args) {
        
        if (this.audio_context) {
            return
        }
        const start_worklet = async () => {
            await this.audio_context.audioWorklet.addModule("/makepad/media/src/os/web_browser/audio_worklet.js", {credentials: 'omit'});
            
            const audio_worklet = new AudioWorkletNode(this.audio_context, 'audio-worklet', {
                numberOfInputs: 0,
                numberOfOutputs: 1,
                outputChannelCount: [2],
                processorOptions: {thread_info: this.alloc_thread_stack(args.closure_ptr)}
            });
            
            audio_worklet.port.onmessage = (e) => {
                let data = e.data;
                switch (data.message_type) {
                    case "console_log":
                    console.log(data.value);
                    break;
                    
                    case "console_error":
                    console.error(data.value);
                    break;
                    
                    case "signal":
                    this.to_wasm.ToWasmSignal(data)
                    this.do_wasm_pump();
                    break;
                }
            };
            audio_worklet.onprocessorerror = (err) => {
                console.error(err);
            }
            audio_worklet.connect(this.audio_context.destination);
            
            return audio_worklet;
        };
        
        let user_interact_hook = (arg) => {
            if(this.audio_context.state === "suspended"){
                this.audio_context.resume();
            }
        }
        this.audio_context = new AudioContext({
            latencyHint: "interactive",
            sampleRate: 44100
        });
        start_worklet();
        window.addEventListener('mousedown', user_interact_hook)
        window.addEventListener('touchstart', user_interact_hook)
    }
    
    FromWasmStartMidiInput() {
        if(navigator.requestMIDIAccess){
            navigator.requestMIDIAccess().then((midi) => {
                let reload_midi_ports = () => {
                    
                    let inputs = [];
                    let input_id = 0;
                    for (let input_pair of midi.inputs) {
                        let input = input_pair[1];
                        inputs.push({
                            uid: "" + input.id,
                            name: input.name,
                            manufacturer: input.manufacturer,
                        });
                        input.onmidimessage = (e) => {
                            let data = e.data;
                            this.to_wasm.ToWasmMidiInputData({
                                input_id: input_id,
                                data: (data[0] << 16) | (data[1] << 8) | data[2],
                            });
                            this.do_wasm_pump();
                        }
                        input_id += 1;
                    }
                    this.to_wasm.ToWasmMidiInputList({inputs});
                    this.do_wasm_pump();
                }
                midi.onstatechange = (e) => {
                    reload_midi_ports();
                }
                reload_midi_ports();
            }, () => {
                console.error("Cannot open midi");
            });
        }
    }
}
