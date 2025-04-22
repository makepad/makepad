use crate::{
    makepad_audio_graph::*, makepad_audio_widgets::display_audio::*,
    makepad_audio_widgets::piano::*, makepad_synth_ironfish::ironfish::*, makepad_widgets::*,
    sequencer::*,
};
//use std::fs::File;
//use std::io::prelude::*;
live_design! {
    use link::widgets::*
    use link::theme::*
    use makepad_example_ironfish::app_desktop::AppDesktop
    use makepad_example_ironfish::app_mobile::AppMobile

    use makepad_audio_graph::mixer::Mixer;
    use makepad_audio_graph::instrument::Instrument;
    use makepad_synth_ironfish::ironfish::IronFish;
    
    use makepad_draw::shader::std::*;
/*
    BlurStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d

            uniform blursize: 0.0,
            uniform blurstd: 0.0,
            uniform blurx: 1.0,
            uniform blury: 0.0,
            varying g1: float,
            varying g2: float,
            varying g3: float,
            varying g4: float,
            varying g5: float,

            varying gaussscale: float,

            varying o0: vec2,

            varying o1a: vec2,
            varying o2a: vec2,
            varying o3a: vec2,
            varying o4a: vec2,
            varying o5a: vec2,

            varying o1b: vec2,
            varying o2b: vec2,
            varying o3b: vec2,
            varying o4b: vec2,
            varying o5b: vec2,

            fn vertex(self) -> vec4
            {
                let x = self.blurx;
                let y = self.blury;

                let offset = 0.003 * self.blursize / max(x,y);
                let standard_deviation = 0.0001 + self.blurstd *0.003;
                let st_dev_sqr = standard_deviation * standard_deviation;

                let off1 = offset;
                let off2 = 2.0*offset;
                let off3 = 3.0*offset;
                let off4 = 4.0*offset;
                let off5 = 5.0*offset;

                let mainscale = (1.0 / sqrt(2*PI*st_dev_sqr));
                let stddevscale = 1.0/ (2*st_dev_sqr);

                self.g1 =  pow(E, -((off1*off1)* stddevscale));
                self.g2 =  pow(E, -((off2*off2)* stddevscale));
                self.g3 =  pow(E, -((off3*off3)* stddevscale));
                self.g4 =  pow(E, -((off4*off4)* stddevscale));
                self.g5 =  pow(E, -((off5*off5)* stddevscale));

                self.gaussscale = 1.0/(1.0 +  (self.g1 + self.g2 + self.g3 + self.g4 + self.g5 )*2.0);

                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);
                self.o0 = self.pos;

                self.o1a = self.o0 + vec2(off1*x,off1*y);
                self.o2a = self.o0 + vec2(off2*x,off2*y);
                self.o3a = self.o0 + vec2(off3*x,off3*y);
                self.o4a = self.o0 + vec2(off4*x,off4*y);
                self.o5a = self.o0 + vec2(off5*x,off5*y);

                self.o1b = self.o0 - vec2(off1*x,off1*y);
                self.o2b = self.o0 - vec2(off2*x,off2*y);
                self.o3b = self.o0 - vec2(off3*x,off3*y);
                self.o4b = self.o0 - vec2(off4*x,off4*y);
                self.o5b = self.o0 - vec2(off5*x,off5*y);

                return pos;
            }

            fn pixel(self) -> vec4{
                let col = sample2d(self.image, self.o0) ;
                col +=  (sample2d(self.image, self.o1a) + sample2d(self.image, self.o1b)) * self.g1;
                col +=  (sample2d(self.image, self.o2a) + sample2d(self.image, self.o2b)) * self.g2 ;
                col +=  (sample2d(self.image, self.o3a) + sample2d(self.image, self.o3b)) * self.g3 ;
                col +=  (sample2d(self.image, self.o4a) + sample2d(self.image, self.o4b)) * self.g4 ;
                col +=  (sample2d(self.image, self.o5a) + sample2d(self.image, self.o5b)) * self.g5 ;
                col = col * self.gaussscale;

                return col ;
            }
        }
    }


    ShadowStage = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d

            uniform shadowopacity:  0.9,
            uniform shadowx: 1.0,
            uniform shadowy: 1.0,

            varying o0: vec2,
            varying oShadow: vec2,
            
            fn vertex(self) -> vec4
            {
                
                let dpi = self.dpi_factor;
                
               
                let pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size);

                self.o0 = self.pos;
                self.oShadow = self.pos - vec2(self.shadowx * dpi, self.shadowy * dpi)*0.001;

                return pos;
            }

            fn pixel(self) -> vec4{
                
                let shadow = sample2d(self.image, self.oShadow);
                let main = sample2d(self.image, self.o0);

                let col =  (vec4(0.0,0.0,0.0,self.shadowopacity)  * shadow.a ) * ( 1 - main.a) + main;

                //col +=  (sample2d(self.image, self.o0) )*0.3;
                

                return col;
            }
        }
    }
*/

    App = {{App}} {

        audio_graph: {
            root: <Mixer> {
                c1 = <Instrument> {
                    <IronFish> {}
                }
            }
        }
        ui: <Root>{
            main_window = <Window> {
                window: {inner_size: vec2(1280, 1000)},
                pass: {clear_color: #2A}
                block_signal_event: true;
                /*body2 = <View>{
                    step4 = <BlurStage>{
                        width: Fill,
                        height: Fill,
                        draw_bg:{blury: 0.0, blurx: 10.0}
                        step3 = <BlurStage>{
                            width: Fill,
                            height: Fill,
                            draw_bg:{blury: 10.0, blurx: 0.0}
                            step2 = <BlurStage>{
                                width: Fill,
                                height: Fill,
                                draw_bg:{blury: 7.07, blurx: 7.07}
                                step1 = <BlurStage>{
                                    width: Fill,
                                    height: Fill,
                                    draw_bg:{blury: -7.07, blurx: 7.07}
                                    <AppDesktop> {}
                                }
                            }
                        }
                    }
                }*/
                body = <AppDesktop> {}
                /*<View>
                {
                    
                    width: Fill,
                    height: Fill,
                    
                    shadowstep = <ShadowStage> {
                        width: Fill,
                        height: Fill,
                        draw_bg:{shadowy: 10.0, shadowx: 10.0, shadowopacity: 2.0}
                        padding: 10
                        <AppDesktop> {}
                    }
                }*/
            }
        }
    }
}
app_main!(App);

pub struct SynthPreset {
    pub id: LiveId,
    pub name: String,
    pub fav: bool,
}

#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    _presets: Vec<SynthPreset>,
    #[live]
    audio_graph: AudioGraph,
    #[rust]
    midi_input: MidiInput,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_audio_widgets::live_design(cx);
        crate::makepad_audio_graph::live_design(cx);
        crate::makepad_synth_ironfish::live_design(cx);
        crate::sequencer::live_design(cx);
        crate::app_desktop::live_design(cx);
        crate::app_mobile::live_design(cx);
    }
}

impl LiveHook for App{
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if apply.from.is_update_from_doc(){
            self.init_ui_state(cx);
        }
    }
}

impl App {
    pub fn init_ui_state(&mut self, cx:&mut Cx){
        let ui = self.ui.clone();
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
        let db = DataBindingStore::from_nodes(ironfish.settings.live_read());
        Self::data_bind(db.data_to_widgets(cx, &ui));
    }
    
    pub fn data_bind(mut db: DataBindingMap) {
        // sequencer
        db.bind(id!(sequencer.playing), ids!(playpause));
        db.bind(id!(sequencer.bpm), ids!(speed.slider));
        db.bind(id!(sequencer.rootnote), ids!(rootnote.dropdown));
        db.bind(id!(sequencer.scale), ids!(scaletype.dropdown));
        db.bind(id!(arp.enabled), ids!(arp.checkbox));
        db.bind(id!(arp.octaves), ids!(arpoctaves.slider));

        // Mixer panel
        db.bind(id!(osc_balance), ids!(balance.slider));
        db.bind(id!(noise), ids!(noise.slider));
        db.bind(id!(sub_osc), ids!(sub.slider));
        db.bind(id!(portamento), ids!(porta.slider));

        // DelayFX Panel
        db.bind(id!(delay.delaysend), ids!(delaysend.slider));
        db.bind(id!(delay.delayfeedback), ids!(delayfeedback.slider));

        db.bind(id!(bitcrush.enable), ids!(crushenable.checkbox));
        db.bind(id!(bitcrush.amount), ids!(crushamount.slider));

        db.bind(id!(delay.difference), ids!(delaydifference.slider));
        db.bind(id!(delay.cross), ids!(delaycross.slider));
        db.bind(id!(delay.length), ids!(delaylength.slider));

        // Chorus panel
        db.bind(id!(chorus.mix), ids!(chorusmix.slider));
        db.bind(id!(chorus.mindelay), ids!(chorusdelay.slider));
        db.bind(id!(chorus.moddepth), ids!(chorusmod.slider));
        db.bind(id!(chorus.rate), ids!(chorusrate.slider));
        db.bind(id!(chorus.phasediff), ids!(chorusphase.slider));
        db.bind(id!(chorus.feedback), ids!(chorusfeedback.slider));

        // Reverb panel
        db.bind(id!(reverb.mix), ids!(reverbmix.slider));
        db.bind(id!(reverb.feedback), ids!(reverbfeedback.slider));

        //LFO Panel
        db.bind(id!(lfo.rate), ids!(rate.slider));
        db.bind(id!(filter1.lfo_amount), ids!(lfoamount.slider));
        db.bind(id!(lfo.synconkey), ids!(sync.checkbox));
        
        //Volume Envelope
        db.bind(id!(volume_envelope.a), ids!(vol_env.attack.slider));
        db.bind(id!(volume_envelope.h), ids!(vol_env.hold.slider));
        db.bind(id!(volume_envelope.d), ids!(vol_env.decay.slider));
        db.bind(id!(volume_envelope.s), ids!(vol_env.sustain.slider));
        db.bind(id!(volume_envelope.r), ids!(vol_env.release.slider));

        //Mod Envelope
        db.bind(id!(mod_envelope.a), ids!(mod_env.attack.slider));
        db.bind(id!(mod_envelope.h), ids!(mod_env.hold.slider));
        db.bind(id!(mod_envelope.d), ids!(mod_env.decay.slider));
        db.bind(id!(mod_envelope.s), ids!(mod_env.sustain.slider));
        db.bind(id!(mod_envelope.r), ids!(mod_env.release.slider));
        db.bind(id!(filter1.envelope_amount), ids!(modamount.slider));

        // Filter panel
        //db.bind(id!(filter1.filter_type), ids!(filter_type.dropdown));
        db.bind(id!(filter1.cutoff), ids!(cutoff.slider));
        db.bind(id!(filter1.resonance), ids!(resonance.slider));

        // Osc1 panel
        db.bind(id!(supersaw1.spread), ids!(osc1.supersaw.spread.slider));
        db.bind(id!(supersaw1.diffuse), ids!(osc1.supersaw.diffuse.slider));
        db.bind(id!(supersaw1.spread), ids!(osc1.supersaw.spread.slider));
        db.bind(id!(supersaw1.diffuse), ids!(osc1.supersaw.diffuse.slider));
        db.bind(id!(supersaw1.spread), ids!(osc1.hypersaw.spread.slider));
        db.bind(id!(supersaw1.diffuse), ids!(osc1.hypersaw.diffuse.slider));

        db.bind(id!(osc1.osc_type), ids!(osc1.type.dropdown));
        db.bind(id!(osc1.transpose), ids!(osc1.transpose.slider));
        db.bind(id!(osc1.detune), ids!(osc1.detune.slider));
        db.bind(id!(osc1.harmonic), ids!(osc1.harmonicshift.slider));
        db.bind(id!(osc1.harmonicenv), ids!(osc1.harmonicenv.slider));
        db.bind(id!(osc1.harmoniclfo), ids!(osc1.harmoniclfo.slider));

        // Osc2 panel
        db.bind(id!(supersaw2.spread), ids!(osc2.supersaw.spread.slider));
        db.bind(id!(supersaw2.diffuse), ids!(osc2.supersaw.diffuse.slider));
        db.bind(id!(supersaw2.spread), ids!(osc2.supersaw.spread.slider));
        db.bind(id!(supersaw2.diffuse), ids!(osc2.supersaw.diffuse.slider));
        db.bind(id!(supersaw2.spread), ids!(osc2.hypersaw.spread.slider));
        db.bind(id!(supersaw2.diffuse), ids!(osc2.hypersaw.diffuse.slider));

        db.bind(id!(osc2.osc_type), ids!(osc2.type.dropdown));
        db.bind(id!(osc2.transpose), ids!(osc2.transpose.slider));
        db.bind(id!(osc2.detune), ids!(osc2.detune.slider));
        db.bind(id!(osc2.harmonic), ids!(osc2.harmonicshift.slider));
        db.bind(id!(osc2.harmonicenv), ids!(osc2.harmonicenv.slider));
        db.bind(id!(osc2.harmoniclfo), ids!(osc2.harmoniclfo.slider));

        db.bind(id!(blur.size), ids!(blursize.slider));
        db.bind(id!(blur.std), ids!(blurstd.slider));




        db.bind(id!(shadow.opacity), ids!(shadowopacity.slider));
        db.bind(id!(shadow.x), ids!(shadowx.slider));
        db.bind(id!(shadow.y), ids!(shadowy.slider));

        // sequencer
        db.bind(id!(sequencer.steps), ids!(sequencer));

        db.apply(id!(osc1.osc_type), ids!(osc1.supersaw, visible), |v| {
            v.enum_eq(id!(SuperSaw))
        });
        db.apply(id!(osc2.osc_type), ids!(osc2.supersaw, visible), |v| {
            v.enum_eq(id!(SuperSaw))
        });
        db.apply(id!(osc1.osc_type), ids!(osc1.hypersaw, visible), |v| {
            v.enum_eq(id!(HyperSaw))
        });
        db.apply(id!(osc2.osc_type), ids!(osc2.hypersaw, visible), |v| {
            v.enum_eq(id!(HyperSaw))
        });
        db.apply(id!(osc1.osc_type), ids!(osc1.harmonic, visible), |v| {
            v.enum_eq(id!(HarmonicSeries))
        });
        db.apply(id!(osc2.osc_type), ids!(osc2.harmonic, visible), |v| {
            v.enum_eq(id!(HarmonicSeries))
        });

        db.apply(
            id!(mod_envelope.a),
            ids!(mod_env.display, draw_bg.attack),
            |v| v,
        );
        db.apply(
            id!(mod_envelope.h),
            ids!(mod_env.display, draw_bg.hold),
            |v| v,
        );
        db.apply(
            id!(mod_envelope.d),
            ids!(mod_env.display, draw_bg.decay),
            |v| v,
        );
        db.apply(
            id!(mod_envelope.s),
            ids!(mod_env.display, draw_bg.sustain),
            |v| v,
        );
        db.apply(
            id!(mod_envelope.r),
            ids!(mod_env.display, draw_bg.release),
            |v| v,
        );
        db.apply(
            id!(volume_envelope.a),
            ids!(vol_env.display, draw_bg.attack),
            |v| v,
        );
        db.apply(
            id!(volume_envelope.h),
            ids!(vol_env.display, draw_bg.hold),
            |v| v,
        );
        db.apply(
            id!(volume_envelope.d),
            ids!(vol_env.display, draw_bg.decay),
            |v| v,
        );
        db.apply(
            id!(volume_envelope.s),
            ids!(vol_env.display, draw_bg.sustain),
            |v| v,
        );
        db.apply(
            id!(volume_envelope.r),
            ids!(vol_env.display, draw_bg.release),
            |v| v,
        );

        /*db.apply(id!(shadow.opacity), ids!(shadowstep, draw_bg.shadowopacity), |v| v);
        db.apply(id!(shadow.x), ids!(shadowstep, draw_bg.shadowx), |v| v);
        db.apply(id!(shadow.y), ids!(shadowstep, draw_bg.shadowy), |v| v);

        db.apply(id!(blur.size), ids!(step1, draw_bg.blursize), |v| v);
        db.apply(id!(blur.std), ids!(step1, draw_bg.blurstd), |v| v);
        db.apply(id!(blur.size), ids!(step2, draw_bg.blursize), |v| v);
        db.apply(id!(blur.std), ids!(step2, draw_bg.blurstd), |v| v);
        db.apply(id!(blur.size), ids!(step3, draw_bg.blursize), |v| v);
        db.apply(id!(blur.std), ids!(step3, draw_bg.blurstd), |v| v);
        db.apply(id!(blur.size), ids!(step4, draw_bg.blursize), |v| v);
        db.apply(id!(blur.std), ids!(step4, draw_bg.blurstd), |v| v);*/
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.preset(cx,0,false);
        self.ui.piano(id!(piano)).set_key_focus(cx);
        self.midi_input = cx.midi_input();
        cx.switch_to_xr();
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        
        let ui = self.ui.clone();
        let piano = ui.piano(id!(piano));

        ui.radio_button_set(ids!(oscillators.tab1, oscillators.tab2,))
            .selected_to_visible(cx, &ui, actions, ids!(oscillators.osc1, oscillators.osc2,));

        ui.radio_button_set(ids!(filter_modes.tab1, filter_modes.tab2,))
            .selected_to_visible(
                cx,
                &ui,
                actions,
                ids!(preset_pages.tab1_frame, preset_pages.tab2_frame,),
            );

        ui.radio_button_set(ids!(
            mobile_modes.tab1,
            mobile_modes.tab2,
            mobile_modes.tab3,
        ))
        .selected_to_visible(
            cx,
            &ui,
            actions,
            ids!(
                application_pages.tab1_frame,
                application_pages.tab2_frame,
                application_pages.tab3_frame,
            ),
        );

        for note in piano.notes_played(&actions) {
            self.audio_graph.send_midi_data(
                MidiNote {
                    channel: 0,
                    is_on: note.is_on,
                    note_number: note.note_number,
                    velocity: note.velocity,
                }
                .into(),
            );
        }

        if ui.button_set(ids!(panic)).clicked(&actions) {
            //log!("hello world");
            cx.midi_reset();
            self.audio_graph.all_notes_off();
        }

        let sequencer = ui.sequencer(id!(sequencer));
        // lets fetch and update the tick.

        if ui.button_set(ids!(clear_grid)).clicked(&actions) {
            sequencer.clear_grid(cx);
        }

        if ui.button_set(ids!(grid_down)).clicked(&actions) {
            sequencer.grid_down(cx);
        }

        if ui.button_set(ids!(grid_up)).clicked(&actions) {
            sequencer.grid_up(cx);
        }
        
      
        if let Some((index,km)) = ui.button_set(ids!(preset_1, preset_2, preset_3, preset_4, preset_5, preset_6, preset_7,preset_8)).which_clicked_modifiers(&actions){
            self.preset(cx, index, km.shift);
        }
        
        let mut db = DataBindingStore::new();
        db.data_bind(cx, actions, &ui, Self::data_bind);

        
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();

        //sequencer.set_step(ironfish.sequencer);

        ironfish.settings.apply_over(cx, &db.nodes);
    }

    fn handle_midi_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        cx.use_midi_inputs(&ports.all_inputs());
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        let piano = self.ui.piano_set(ids!(piano));
        while let Some((_, data)) = self.midi_input.receive() {
            self.audio_graph.send_midi_data(data);
            if let Some(note) = data.decode().on_note() {
                piano.set_note(cx, note.is_on, note.note_number)
            }
        }
    }
}
impl App{
    #[cfg(target_arch = "wasm32")]
    pub fn preset(&mut self, _cx: &mut Cx, _index: usize, _save: bool) {
        
    }
    
    #[cfg(not(target_arch = "wasm32"))]
    pub fn preset(&mut self, cx: &mut Cx, index: usize, save: bool) {
        use std::fs::File;
        use std::io::prelude::*;
        
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
        
        let file_name = format!("examples/ironfish/preset_{}.txt", index);
        if save {
            let nodes = ironfish.settings.live_read();
            let data = nodes.to_cbor(0).unwrap();
            let data = makepad_miniz::compress_to_vec(&data, 10);
            let data = makepad_base64::base64_encode(&data, &makepad_base64::BASE64_URL_SAFE);
            log!("Saving preset {}", file_name);
            let mut file = File::create(&file_name).unwrap();
            file.write_all(&data).unwrap();
        }
        else if let Ok(mut file) = std::fs::File::open(&file_name) {
            log!("Loading preset {}", file_name);
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if let Ok(data) = makepad_base64::base64_decode(&data) {
                if let Ok(data) = makepad_miniz::decompress_to_vec(&data) {
                    let mut nodes = Vec::new();
                    nodes.from_cbor(&data).unwrap();
                    ironfish.settings.apply_over(cx, &nodes);
                    self.init_ui_state(cx);
                    //self.imgui.root_frame().bind_read(cx, &nodes);
                }
                else {
                    log!("Error decompressing preset");
                }
            }
            else {
                log!("Error base64 decoding preset");
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        self.audio_graph
            .handle_event_with(cx, event, &mut |cx, action| {
                let display_audio = self.ui.display_audio_set(ids!(display_audio));
                match action {
                    AudioGraphAction::DisplayAudio { buffer, voice, .. } => {
                        display_audio.process_buffer(cx, None, voice, buffer, 1.0);
                    }
                    AudioGraphAction::VoiceOff { voice } => {
                        display_audio.voice_off(cx, voice);
                    }
                };
            });
    }
}
