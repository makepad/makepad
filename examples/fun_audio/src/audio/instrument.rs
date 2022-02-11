use {
    crate::{
        audio::*,
        makepad_component::*,
        makepad_platform::{*, audio::*, midi::*}
    },
};

live_register!{
    Instrument: {{Instrument}} {
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live)]
#[live_register(audio_component_factory!(Instrument))]
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
    from_ui: FromUIReceiver<FromUI>,
    steps: Vec<Step>
}

impl AudioGraphNode for Node {
    fn handle_midi_1_data(&mut self, data: Midi1Data) {
        for step in &mut self.steps {
            step.graph_node.handle_midi_1_data(data);
        }
    }
    
    fn render_to_audio_buffer(&mut self, time: AudioTime, outputs: &mut [&mut AudioBuffer], inputs: &[&AudioBuffer]) {
        // reverse over the steps chaining the audio nodes
        let steps = &mut self.steps;
        let num_steps = steps.len();
        for i in (0..num_steps).rev() {
            if i == 0 { // first one uses our main output buffer
                let step = &mut steps[0];
                if i == num_steps - 1 { // last one uses external inputs
                    step.graph_node.render_to_audio_buffer(time, outputs, inputs);
                }
                else{
                    step.graph_node.render_to_audio_buffer(time, outputs, &[&step.input_buffer]);
                }
            }
            else {
                let (step0, step1) = steps.split_at_mut(i);
                let output_buffer = &mut step0[i - 1].input_buffer;
                output_buffer.resize_like(outputs[0]);
                if i == num_steps - 1 { // last one uses external inputs
                    step1[0].graph_node.render_to_audio_buffer(time, &mut[output_buffer], inputs);
                }
                else {
                    let step = &mut step1[0];
                    step.graph_node.render_to_audio_buffer(time, &mut[output_buffer], &[&step.input_buffer]);
                }
            };
        }
    }
}

impl LiveHook for Instrument {
    fn apply_value_unknown(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if apply_from.is_from_doc() {
            self.step_order.push(nodes[index].id);
        }
        self.steps.get_or_insert(cx, nodes[index].id, | cx | {AudioComponentRef::new(cx)})
            .apply(cx, apply_from, index, nodes)
    }
    
    fn after_apply(&mut self, _cx: &mut Cx, apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        // so.. alright.. if we have a file_id we can gc the inputs
        if apply_from.is_from_doc() {
            self.steps.retain_visible();
        }
    }
}

impl AudioComponent for Instrument {
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send> {
        self.from_ui.new_channel();
        let mut steps = Vec::new();
        for step_id in &self.step_order {
            if let Some(input) = self.steps.get_mut(step_id) {
                if let Some(input) = input.as_mut() {
                    steps.push(Step {
                        graph_node: input.get_graph_node(),
                        input_buffer: AudioBuffer::default()
                    });
                }
            }
        }
        Box::new(Node {
            steps,
            from_ui: self.from_ui.receiver()
        })
    }
    
    fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)) {
        for step in self.steps.values_mut(){
            if let Some(step) = step.as_mut(){
                step.handle_event_with_fn(cx, event, dispatch_action)
            }
        }
    }
}
