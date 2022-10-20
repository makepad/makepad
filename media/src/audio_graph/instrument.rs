use {
    crate::{
        audio::*,
        makepad_platform::*,
        makepad_platform::thread::*,
        midi::*,
        audio_graph::*,
    },
};

live_design!{
    Instrument= {{Instrument}} {
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live)]
#[live_design_fn(audio_component!(Instrument))]
struct Instrument {
    #[rust] step_order: Vec<LiveId>,
    #[rust] steps: ComponentMap<LiveId, AudioComponentRef>,
    
    #[rust] from_ui: FromUISender<FromUI>,
}

struct Step {
    graph_node: Box<dyn AudioGraphNode + Send >,
    input_buffer: AudioBuffer,
}

struct Node {
    _from_ui: FromUIReceiver<FromUI>,
    steps: Vec<Step>
}

impl AudioGraphNode for Node {
    fn all_notes_off(&mut self){
        for step in &mut self.steps {
            step.graph_node.all_notes_off();
        }
    }
    fn handle_midi_data(&mut self, data: MidiData) {
        for step in &mut self.steps {
            step.graph_node.handle_midi_data(data);
        }
    }
    
    fn render_to_audio_buffer(&mut self, time: AudioTime, outputs: &mut [&mut AudioBuffer], inputs: &[&AudioBuffer], display:&mut DisplayAudioGraph) {
        // reverse over the steps chaining the audio nodes
        let steps = &mut self.steps;
        let num_steps = steps.len();
        for i in (0..num_steps).rev() {
            if i == 0 { // first one uses our main output buffer
                let step = &mut steps[0];
                if i == num_steps - 1 { // last one uses external inputs
                    step.graph_node.render_to_audio_buffer(time, outputs, inputs, display);
                }
                else{
                    step.graph_node.render_to_audio_buffer(time, outputs, &[&step.input_buffer], display);
                }
            }
            else {
                let (step0, step1) = steps.split_at_mut(i);
                let output_buffer = &mut step0[i - 1].input_buffer;
                output_buffer.resize_like(outputs[0]);
                if i == num_steps - 1 { // last one uses external inputs
                    step1[0].graph_node.render_to_audio_buffer(time, &mut[output_buffer], inputs, display);
                }
                else {
                    let step = &mut step1[0];
                    step.graph_node.render_to_audio_buffer(time, &mut[output_buffer], &[&step.input_buffer], display);
                }
            };
        }
    }
}

impl LiveHook for Instrument {
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if from.is_from_doc() {
            self.step_order.push(nodes[index].id);
        }
        self.steps.get_or_insert(cx, nodes[index].id, | cx | {AudioComponentRef::new(cx)})
            .apply(cx, from, index, nodes)
    }
    
    fn after_apply(&mut self, _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        // so.. alright.. if we have a file_id we can gc the inputs
        if from.is_from_doc() {
            self.steps.retain_visible();
        }
    }
}

impl AudioComponent for Instrument {
    fn get_graph_node(&mut self, cx:&mut Cx) -> Box<dyn AudioGraphNode + Send> {
        self.from_ui.new_channel();
        let mut steps = Vec::new();
        for step_id in &self.step_order {
            if let Some(input) = self.steps.get_mut(step_id).unwrap().as_mut() {
                steps.push(Step {
                    graph_node: input.get_graph_node(cx),
                    input_buffer: AudioBuffer::default()
                });
            }
        }
        Box::new(Node {
            steps,
            _from_ui: self.from_ui.receiver()
        })
    }
    
    fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)) {
        for step in self.steps.values_mut(){
            if let Some(step) = step.as_mut(){
                step.handle_event_fn(cx, event, dispatch_action)
            }
        }
    }
    
      fn audio_query(&mut self, query: &AudioQuery, callback: &mut Option<AudioQueryCb>) -> AudioResult {
        for input in self.steps.values_mut(){
            input.audio_query(query, callback)?;
        }
        AudioResult::not_found()
    }
}
