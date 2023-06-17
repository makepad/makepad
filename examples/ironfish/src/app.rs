use crate::{
    makepad_widgets::*,
    makepad_audio_graph::*,
    makepad_platform::midi::*,
    
    makepad_synth_ironfish::ironfish::*,
    makepad_audio_widgets::piano::*,
    sequencer::*,
    makepad_audio_widgets::display_audio::*
};

//use std::fs::File;
//use std::io::prelude::*;
live_design!{  
    import makepad_widgets::frame::*
    import makepad_widgets::button::Button;
    import makepad_example_ironfish::app_desktop::AppDesktop
    import makepad_example_ironfish::app_mobile::AppMobile
    import makepad_widgets::desktop_window::DesktopWindow
    import makepad_widgets::multi_window::MultiWindow
    
    import makepad_audio_graph::mixer::Mixer;
    import makepad_audio_graph::instrument::Instrument;
    import makepad_synth_ironfish::ironfish::IronFish;
    import makepad_widgets::designer::Designer;
    import makepad_widgets::slides_view::Slide;
    import makepad_widgets::slides_view::SlideChapter;
    import makepad_widgets::slides_view::SlideBody;
    import makepad_widgets::slides_view::SlidesView;
    
    //import makepad_example_fractal_zoom::mandelbrot::Mandelbrot;
    //import makepad_example_numbers::number_grid::NumberGrid;
    // APP
    //ui: <AppMobile> {}
    App = {{App}} {
        
        audio_graph: {
            root: <Mixer> {
                c1 = <Instrument> {
                    <IronFish> {}
                }
            }
        }
        ui: <DesktopWindow> {
            window: {inner_size: vec2(1280, 1000), dpi_override:2},
            pass: {clear_color: #2A}
            block_signal_event: true; 
            <AppDesktop> {}
        }
        
        /*
        ui= <MultiWindow> {
            mobile =<DesktopWindow> {
                window: {inner_size: vec2(1280, 1000), dpi_override:2},
                pass: {clear_color: #2A}
                block_signal_event: true; 
                <AppDesktop> {}
            }
            <DesktopWindow> {
                window: {position: vec2(0, 400), inner_size: vec2(800, 800)},
                pass: {clear_color: #2A}
                block_signal_event: true;
                layout: {padding: {top: 30}},
                <Designer> {}
            }
            <DesktopWindow> {
                window: {position: vec2(0, 0), inner_size: vec2(400, 800)},
                pass: {clear_color: #2A}
                block_signal_event: true; 
                <AppMobile> {}
            }
        }*/
/*
        ui=<DesktopWindow> {
            window: {inner_size: vec2(1920, 1080)},
            
            pass: {clear_color: #2A}
            block_signal_event: true; 
            <SlidesView> {
                goal_pos: 0.0
                
                <SlideChapter> {
                    title = {label: "MAKEPAD.\nDESIGNING MODERN\nUIs FOR RUST."},
                    <SlideBody> {label: "Rik Arends\n"}
                }
                <Slide> {
                    title = {label: "A long long time ago …"},
                    <SlideBody> {label: "… in a galaxy nearby\n   Cloud9 IDE & ACE"}
                }
                <Slide> {
                    title = {label: "HTML as an IDE UI?\nMadness!"},
                    <SlideBody> {label: "- Integrating design and code was hard\n- Could not innovate editing\n- Too slow, too hard to control"}
                }
                <Slide> {
                    title = {label: "Let's start over!"},
                    <SlideBody> {label: "- JavaScript and WebGL for UI\n- Write shaders to style UI\n- A quick demo"}
                }
                <Slide> {
                    title = {label: "Maybe JavaScript\nwas the problem?"},
                    <SlideBody> {label: "- Great livecoding, but …\n- Chrome crashing tabs after 30 minutes\n- Too slow"}
                }
                <Slide> {
                    title = {label: "Rust appears"},
                    <SlideBody> {label: "- Let's try again: Native + Wasm\n- Makepad in Rust\n- Startup with Eddy and Sebastian"}
                }
                <Slide> {title = {label: "Rust is fast: SIMD Mandelbrot"}, 
                    layout: {align: {x: 0.0, y: 0.5}, flow: Down, spacing: 10, padding: 50}
                    draw_bg: { color: #x1A, radius: 5.0 }
                    <Frame>{
                        layout:{padding: 0, align:{x:0.5}, spacing: 20}
                        <Box>{
                            draw_bg: { color: #x2A } 
                            walk: { margin: 0.0}
                            layout:{ padding: 0.0 }
                            <Mandelbrot> {walk:{width:Fill, height:Fill}}
                        }
                    }
                }

                <Slide> {title = {label: "Instanced rendering"}, 
                    layout: {align: {x: 0.0, y: 0.5}, flow: Down, spacing: 10, padding: 50}
                    draw_bg: { color: #x1A, radius: 5.0 }
                    <Frame>{
                        layout:{padding: 0, align:{x:0.5}, spacing: 20}
                        <Box>{
                            draw_bg: { color: #x2A }
                            walk: { margin: 0.0}
                            layout:{ padding: 0.0 }
                            <NumberGrid> {walk:{width:Fill, height:Fill}}
                        }
                    }
                }
                
                <Slide> {
                    title = {label: "Our goal:\nUnify coding and UI design again."},
                    <SlideBody> {label: "As it was in Visual Basic.\nNow with modern design."}
                }

                <Slide> {title = {label: "Ironfish Desktop"}, 
                    <Box>{
                        draw_bg: { color: #x2A }
                        walk: { margin: 10.0, width: 1600 }
                        layout:{ padding: 0.0 }
                        <AppDesktop> {}
                    }
                }
                
                <Slide> {title = {label: "Ironfish Mobile"}, 
                    <Frame>{
                        layout:{padding: 0, align:{x:0.5}}
                        walk: { margin: { top: 0 }}
                        <AppMobile> {walk:{width:400, height: Fill}}
                    }
                }
                
                <Slide> {title = {label: "Multi modal"}, 
                    <Frame>{
                        layout:{padding: 0, align:{x:0.5}, spacing: 20}

                        <AppMobile> {walk:{width:400, height: Fill}}

                        <Box>{
                            draw_bg: { color: #x2A }
                            walk: { margin: 0.0}
                            layout:{ padding: 0.0 }
                            <AppDesktop> {
                                walk:{width: Fill, height: Fill}
                            }
                        }
                    }
                }
                
                <Slide> {title = {label: "Visual design"}, 
                    layout: {align: {x: 0.0, y: 0.5}, flow: Down, spacing: 10, padding: 50}
                    <Frame>{
                        layout:{padding: 0, align:{x:0.5}, spacing: 20}
                        <Box>{
                            draw_bg: { color: #x2A }
                            walk: { margin: 0.0}
                            layout:{ padding: 0.0 }
                            <AppDesktop> {walk:{width:900}}
                        }

                        <Box>{
                            draw_bg: { color: #x2A }
                            walk: { margin: 0.0}
                            layout:{ padding: 0.0 }
                            <Designer> {walk:{width:900}}
                        } 
                    }
                }
                
                <Slide> {
                    title = {label: "Our UI language: Live."},
                    <SlideBody> {label: "- Live editable\n- Design tool manipulates text\n- Inheritance structure\n- Rust-like module system"}
                }
                
                <Slide> {
                    title = {label: "These slides are a Makepad app"},
                    <SlideBody> {label: "- Show source\n"}
                    <SlideBody> {label: "- Show Rust API\n"}
                }                
                
                <Slide> {
                    title = {label: "Future"},
                    <SlideBody> {label: "- Release of 0.4.0 soon\n- Windows, Linux, Mac, Web and Android\n- github.com/makepad/makepad\n- twitter: @rikarends @makepad"}
                }                
                
                <Slide> {
                    title = {label: "Build for Android"},
                    <SlideBody> {label: "- SDK installer\n- Cargo makepad android\n"}
                }                
            }
        }*/
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
    #[live] ui: WidgetRef,
    #[rust] _presets: Vec<SynthPreset>,
    #[live] audio_graph: AudioGraph,
    #[rust] midi_input: MidiInput,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_audio_widgets::live_design(cx);
        crate::makepad_audio_graph::live_design(cx);
        crate::makepad_synth_ironfish::live_design(cx);
        crate::sequencer::live_design(cx);
        crate::app_desktop::live_design(cx);
        crate::app_mobile::live_design(cx);
       //makepad_example_fractal_zoom::mandelbrot::live_design(cx);
        //makepad_example_numbers::number_grid::live_design(cx);
    }
}

impl App {
    
    pub fn data_bind(&mut self, mut db: DataBindingMap) {
        // sequencer
        db.bind(id!(sequencer.playing), ids!(playpause.checkbox));
        db.bind(id!(sequencer.bpm), ids!(speed.slider));
        db.bind(id!(sequencer.rootnote), ids!(rootnote.dropdown));
        db.bind(id!(sequencer.scale), ids!(scaletype.dropdown));
        db.bind(id!(arp.enabled), ids!(arp.checkbox));
        //db.bind(id!(arp.octaves), ids!(arp.octaves.slider));
        
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
        db.bind(id!(supersaw1.spread), ids!(osc2.supersaw.spread.slider));
        db.bind(id!(supersaw1.diffuse), ids!(osc2.supersaw.diffuse.slider));
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
        
        // sequencer
        db.bind(id!(sequencer.steps), ids!(sequencer));
        
        db.apply(id!(osc1.osc_type), ids!(osc1.supersaw, visible), | v | v.enum_eq(id!(SuperSaw)));
        db.apply(id!(osc2.osc_type), ids!(osc2.supersaw, visible), | v | v.enum_eq(id!(SuperSaw)));
        db.apply(id!(osc1.osc_type), ids!(osc1.hypersaw, visible), | v | v.enum_eq(id!(HyperSaw)));
        db.apply(id!(osc2.osc_type), ids!(osc2.hypersaw, visible), | v | v.enum_eq(id!(HyperSaw)));
        db.apply(id!(osc1.osc_type), ids!(osc1.harmonic, visible), | v | v.enum_eq(id!(HarmonicSeries)));
        db.apply(id!(osc2.osc_type), ids!(osc2.harmonic, visible), | v | v.enum_eq(id!(HarmonicSeries)));
        
        db.apply(id!(mod_envelope.a), ids!(mod_env.display, draw_bg.attack), | v | v);
        db.apply(id!(mod_envelope.h), ids!(mod_env.display, draw_bg.hold), | v | v);
        db.apply(id!(mod_envelope.d), ids!(mod_env.display, draw_bg.decay), | v | v);
        db.apply(id!(mod_envelope.s), ids!(mod_env.display, draw_bg.sustain), | v | v);
        db.apply(id!(mod_envelope.r), ids!(mod_env.display, draw_bg.release), | v | v);
        db.apply(id!(volume_envelope.a), ids!(vol_env.display, draw_bg.attack), | v | v);
        db.apply(id!(volume_envelope.h), ids!(vol_env.display, draw_bg.hold), | v | v);
        db.apply(id!(volume_envelope.d), ids!(vol_env.display, draw_bg.decay), | v | v);
        db.apply(id!(volume_envelope.s), ids!(vol_env.display, draw_bg.sustain), | v | v);
        db.apply(id!(volume_envelope.r), ids!(vol_env.display, draw_bg.release), | v | v);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        let preset_lists = self.ui.get_swipe_list_set(ids!(preset_list));
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                if let Some(mut list) = preset_lists.has_widget(&next).borrow_mut() {
                    for i in 0..10 {
                        if let Some(item) = list.get_entry(cx, LiveId(i as u64).into(), live_id!(Entry)) {
                            item.get_button(id!(label)).set_label(&format!("Button id {i}"));
                            item.draw_widget_all(cx);
                        }
                    }
                }
            }
            return
        }
        let ui = self.ui.clone();
        let mut synth_db = DataBindingStore::new();
        let mut actions = ui.handle_widget_event(cx, event);
        
        // handle preset lists events
        for list in preset_lists.iter() {
            for item in list.items_with_actions(&actions).iter() {
                // check for actions inside the list item
                if item.get_button(id!(delete)).clicked(&actions) {
                    // delete the item in the data
                    list.redraw(cx); 
                }
            }
        }
        
        if let Event::Construct = event {
            let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
            synth_db.nodes = ironfish.settings.live_read();
            ui.get_piano(id!(piano)).set_key_focus(cx);
            self.midi_input = cx.midi_input();
        }
        
        if let Event::MidiPorts(ports) = event {
            cx.use_midi_inputs(&ports.all_inputs());
        }
        
        if let Event::AudioDevices(devices) = event {
            cx.use_audio_outputs(&devices.default_output());
        }
        
        ui.get_radio_button_set(ids!(
            oscillators.tab1,
            oscillators.tab2,
        )).selected_to_visible(cx, &ui, &actions, ids!(
            oscillators.osc1,
            oscillators.osc2,
        ));
        
        ui.get_radio_button_set(ids!(
            filter_modes.tab1,
            filter_modes.tab2,
        )).selected_to_visible(cx, &ui, &actions, ids!(
            preset_pages.tab1_frame,
            preset_pages.tab2_frame,
        ));
        
        ui.get_radio_button_set(ids!(
            mobile_modes.tab1,
            mobile_modes.tab2,
            mobile_modes.tab3,
        )).selected_to_visible(cx, &ui, &actions, ids!(
            application_pages.tab1_frame,
            application_pages.tab2_frame,
            application_pages.tab3_frame,
        ));
        
        let display_audio = ui.get_display_audio_set(ids!(display_audio));
        
        let mut buffers = 0;
        self.audio_graph.handle_event_with(cx, event, &mut | cx, action | {
            match action {
                AudioGraphAction::DisplayAudio {buffer, voice, ..} => {
                    display_audio.process_buffer(cx, None, voice, buffer);
                    buffers += 1;
                }
                AudioGraphAction::VoiceOff {voice} => {
                    display_audio.voice_off(cx, voice);
                }
            };
        });
        
        let piano = ui.get_piano_set(ids!(piano));
        
        while let Some((_, data)) = self.midi_input.receive() {
            self.audio_graph.send_midi_data(data);
            if let Some(note) = data.decode().on_note() {
                piano.set_note(cx, note.is_on, note.note_number)
            }
        }
        
        for note in piano.notes_played(&actions) {
            self.audio_graph.send_midi_data(MidiNote {
                channel: 0,
                is_on: note.is_on,
                note_number: note.note_number,
                velocity: note.velocity
            }.into());
        }
        
        if ui.get_button_set(ids!(panic)).clicked(&actions) {
            cx.midi_reset();
            self.audio_graph.all_notes_off();
        }
        
        let sequencer = ui.get_sequencer(id!(sequencer));
        // lets fetch and update the tick.
        
        if ui.get_button_set(ids!(clear_grid)).clicked(&actions) {
            sequencer.clear_grid(cx, &mut actions);
        }
        
        if ui.get_button_set(ids!(grid_down)).clicked(&actions) {
            sequencer.grid_down(cx, &mut actions);
        }
        
        if ui.get_button_set(ids!(grid_up)).clicked(&actions) {
            sequencer.grid_up(cx, &mut actions);
        }
        
        self.data_bind(synth_db.widgets_to_data(cx, &actions, &ui));
        self.data_bind(synth_db.data_to_widgets(cx, &actions, &ui));
        
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
        ironfish.settings.apply_over(cx, &synth_db.nodes);
    }
    /*
    pub fn preset(&mut self, cx: &mut Cx, index: usize, save: bool) {
        let ironfish = self.audio_graph.by_type::<IronFish>().unwrap();
        let file_name = format!("preset_{}.txt", index);
        if save {
            let nodes = ironfish.settings.live_read();
            let data = nodes.to_cbor(0).unwrap();
            let data = makepad_miniz::compress_to_vec(&data, 10);
            let data = makepad_base64::base64_encode(&data, &makepad_base64::BASE64_URL_SAFE);
            log!("Saving preset {}", file_name);
            let mut file = File::create(&file_name).unwrap();
            file.write_all(&data).unwrap();
        }
        else if let Ok(mut file) = File::open(&file_name) {
            log!("Loading preset {}", file_name);
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if let Ok(data) = makepad_base64::base64_decode(&data) {
                if let Ok(data) = makepad_miniz::decompress_to_vec(&data) {
                    let mut nodes = Vec::new();
                    nodes.from_cbor(&data).unwrap();
                    ironfish.settings.apply_over(cx, &nodes);
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
    }*/
    
}