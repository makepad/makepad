//! Run a shader author's frame checks over the ordinary offline audition.
use crate::{
    audition, audition_input, compile_audition, compile_items_with, parse, set_audition_time, with_audition_recording,
    AudioBindings, AudioImports, AudioShader, AudioShaderRef, Backend, Instance, Kind, SampleView,
    ShaderError, AUDITION_RATE, MAX_FRAMES,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelftestBackend {
    Native,
    Wasm,
    Interp,
}

impl SelftestBackend {
    fn for_shader(shader: &AudioShader) -> Self {
        #[cfg(target_arch = "wasm32")]
        if crate::wasm::slot(shader.program_key()).is_some() {
            return Self::Wasm;
        }
        match shader.backend() {
            Backend::Native => Self::Native,
            Backend::Interp => Self::Interp,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelftestReport {
    /// The backend that rendered the shader.
    pub backend: SelftestBackend,
    /// The predicate evaluated on the audition.
    pub check: &'static str,
    /// Zero-based frame on the song clock.
    pub first_failed_frame: Option<usize>,
}

impl SelftestReport {
    pub fn passed(&self) -> bool {
        self.first_failed_frame.is_none()
    }
}

/// Render the shader on the host and interpret the supplied predicate on its stereo output.
/// Diagnostics use byte offsets into the shader, a newline, then the test.
pub fn selftest(shader: &str, test: &str, bindings: &AudioBindings<'_>) -> Result<SelftestReport, Vec<ShaderError>> {
    let render = compile_audition(shader, false, bindings)?;
    let check = match render.kind {
        Kind::Instrument => "check_voice",
        Kind::Effect => "check_effect",
        Kind::Control => "check_control",
    };
    let input = audition_input(render.kind);
    let source = format!("{shader}\n{test}");
    let checker = compile_checker(&source, shader.len() + 1, check, &input, bindings)?;
    let backend = SelftestBackend::for_shader(&render);
    let audio = audition(&render, &input);
    // Check every declared control port. A mono port supplies both lanes of out.
    let pairs = if render.kind == Kind::Control {
        render.port_info.iter().map(|p| (p.first as usize, (p.first + p.channels - 1) as usize)).collect()
    } else { vec![(0, 1)] };
    let mut first_failed_frame = None;
    for (left_ch, right_ch) in pairs {
        let mut instance = Instance::new(checker.clone(), AUDITION_RATE);
        set_audition_time(&mut instance, 0);
        let block = MAX_FRAMES as usize;
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        for at in (0..audio.outputs[left_ch].len()).step_by(block) {
            let end = (at + block).min(audio.outputs[left_ch].len());
            let n = end - at;
            left[..n].fill(0.0);
            right[..n].fill(0.0);
            instance.process(&audio.outputs[left_ch][at..end], &audio.outputs[right_ch][at..end], &mut left[..n], &mut right[..n]);
            if let Some(i) = left[..n].iter().position(|&value| value == 0.0) {
                first_failed_frame = Some(first_failed_frame.map_or(at + i, |f: usize| f.min(at + i)));
                break;
            }
        }
    }
    Ok(SelftestReport { backend, check, first_failed_frame })
}

/// Lower the pasted declarations with the predicate as the only per-frame entry.
/// The original render program keeps its code and its link on the host.
fn compile_checker(
    source: &str,
    test_start: usize,
    check: &str,
    input: &[Vec<f32>; 2],
    bindings: &AudioBindings<'_>,
) -> Result<AudioShaderRef, Vec<ShaderError>> {
    let tokens = parse::lex(source).map_err(|e| vec![e])?;
    let mut items = parse::Parser::new(&tokens).items().map_err(|e| vec![e])?;
    let predicate = items.iter().find_map(|item| match item {
        parse::Item::Fn(f) if f.name == check && f.span.start >= test_start => Some(f),
        _ => None,
    }).ok_or_else(|| vec![ShaderError::new(test_start, source.len(), format!("the test must define `{check}`"))])?;
    let span = predicate.span;
    items.retain(|item| match item {
        parse::Item::Fn(f) if f.span.start < test_start => !matches!(f.name.as_str(), "voice" | "effect" | "effect_block" | "block"),
        parse::Item::Decl { name, span, .. } if span.start < test_start => name != "mono",
        _ => true,
    });

    // The effect entry receives the recorded output. For an effect, also bind the
    // actual stereo input as a table indexed by the same absolute frame.
    let mut samples = bindings.samples.to_vec();
    let recording;
    let (table, input_arg) = if check == "check_effect" {
        recording = input[0].iter().zip(&input[1]).flat_map(|(&l, &r)| [l, r]).collect::<Vec<_>>();
        samples.insert(0, SampleView { name: ":input", channels: 2, frames: input[0].len() as u32, data: &recording });
        ("let input = sample(2)", "input[frame], ")
    } else {
        ("", "")
    };
    let entry = format!("{table}
        var position: i32 = 0
        fn effect(l, r) {{
            let frame = position
            position += 1
            if {check}(frame, {input_arg}vec2(l, r)) == true {{ 1.0 }} else {{ 0.0 }}
        }}");
    let mut tokens = parse::lex(&entry).map_err(|e| vec![e])?;
    for t in &mut tokens {
        // Internal globals cannot be spelled by the author. Diagnostics about
        // the generated call (arity or return type) point to the predicate.
        if let parse::Tk::Ident(name) = &mut t.tk {
            if matches!(name.as_str(), "input" | "position") {
                name.insert(0, ':');
            }
        }
        t.start = span.start;
        t.end = span.end;
    }
    items.extend(parse::Parser::new(&tokens).items().map_err(|e| vec![e])?);
    let roots = [check.to_owned(), "effect".to_owned(), "init".to_owned()].into_iter().collect();
    with_audition_recording(&AudioBindings { samples: &samples, ..*bindings }, |b| {
        let mut imports = AudioImports::new(source.len());
        let items = imports.resolve(items.clone(), b.modules).map_err(|e| vec![e])?;
        let items = makepad_script_compute::module::prune(items, &roots);
        compile_items_with(items, imports, Backend::Interp, false, b).map(AudioShaderRef::from_arc)
    })
}
