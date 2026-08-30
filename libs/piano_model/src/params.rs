// Design-space parameters of the instrument. Every number that voices the
// piano — radiation curve corners, loss constants, felt stiffness scaling,
// unison detail, attack-noise levels, phantom-partial levels — lives here,
// with defaults that ARE the shipped instrument. Piano::new uses the
// defaults; Piano::new_with_params exists so verification tooling can walk
// the design space against reference recordings without recompiling.
// Everything is still derived physics: these are the material and geometry
// constants of the design, not per-note tweaks.

macro_rules! design_params {
    ($($(#[$doc:meta])* $name:ident = $default:expr,)*) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct DesignParams {
            $($(#[$doc])* pub $name: f64,)*
        }
        impl Default for DesignParams {
            fn default() -> Self {
                Self { $($name: $default,)* }
            }
        }
        impl DesignParams {
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name)),*];
            pub fn get(&self, name: &str) -> Option<f64> {
                match name { $(stringify!($name) => Some(self.$name),)* _ => None }
            }
            pub fn set(&mut self, name: &str, v: f64) -> bool {
                match name { $(stringify!($name) => { self.$name = v; true },)* _ => false }
            }
        }
    }
}

design_params! {
    // --- radiativity curve R(f) (soundboard.rs) -------------------------
    /// lower -3 dB knee pair: f/(f+rad_hp1) * f/(f+rad_hp2)
    rad_hp1 = 42.23177271808394,
    rad_hp2 = 20.96065338537515,
    /// top roll-off corner (Hz)
    rad_lp = 3983.521895537696,
    /// top roll-off order: amplitude (1/(1+(f/lp)^2))^(rad_lp_pow/2); 1.0 = -6 dB/oct
    rad_lp_pow = 0.3,
    /// low-mid body emphasis (see soundboard::radiativity): amplitude gain
    /// rad_body on a 4th-order shelf below rad_body_hz — the main-resonance
    /// region of the board, where the middle octaves' fundamentals radiate
    rad_body = 1.2460032137074408,
    rad_body_hz = 234.02385502318697,
    // --- string losses (keys.rs) ---------------------------------------
    /// fundamental T60 at A0 (s): t60 = t60_base*(1-t)^t60_pow + t60_min
    t60_base = 19.188979032721246,
    t60_pow = 1.8013173439456476,
    t60_min = 0.4161927367741921,
    /// quadratic-in-frequency loss (1/s per kHz^2): a2 = a2_lo + a2_slope*t.
    /// Near-flat across the compass: the old 0.44 slope gave a C6 partial at
    /// 5.5 kHz sigma ~13/s (dead in 150 ms) where the reference recordings
    /// hold their treble partials 3..6 nearly flat through the first 300 ms;
    /// plain treble wire has LESS internal loss than wound bass, not 8x more.
    a2_lo = 0.04,
    a2_slope = 0.13940587690768688,
    // --- unison / coupling ---------------------------------------------
    /// unison detune (cents): det_lo + det_slope*t
    det_lo = 0.13385990784691343,
    det_slope = 0.2178485243160299,
    /// scale on the Weinreich decay-rate split between unison members
    wein = 1.39237583,
    /// how much the split collapses toward the treble (spread = 1 - wt*t)
    wein_treble = 0.46802472382524163,
    /// split CONTRAST exponent toward the top octave: the measured treble
    /// prompt/aftersound ratio (e.g. ~6:1 at A6) exceeds what the mid-range
    /// split table reaches; multipliers are raised to this power over the
    /// top ~2 octaves (1.0 = off)
    wein_top = 0.6150972469261853,
    /// input-weight bias between unison normal modes: the hammer strikes
    /// the strings IN PHASE, so the fast in-phase normal mode receives
    /// nearly all the drive and the slow anti-phase modes only the
    /// mistuning residue (Weinreich). in_w ~ sigma_mult^wein_inw,
    /// mean-normalised (0 = equal drive, the old behaviour).
    wein_inw = 0.19179190081138914,
    // --- strike comb -----------------------------------------------------
    /// floor under |sin(n pi x0/L)|: finite hammer width, moving contact
    /// point and non-rigid termination keep real comb nulls shallow
    comb_fill = 0.169534713015154,
    /// strike position x0/L = spos_lo - (spos_lo-spos_hi)*t^spos_pow
    spos_lo = 0.11587894721096614,
    spos_hi = 0.06534209294294828,
    spos_pow = 1.3,
    // --- hammer / felt ---------------------------------------------------
    /// felt stiffness exponent of 10: feltk_lo + feltk_span*t
    feltk_lo = 7.801180314385711,
    feltk_span = 4.88370182145988,
    /// felt power p: feltp_lo + feltp_span*t
    feltp_lo = 2.332016978017561,
    feltp_span = 1.4310157297332198,
    /// mezzo-forte hammer speed (m/s) used for voicing estimates
    v_mf = 2.312304890125284,
    /// lock-up onset as a fraction of mf compression
    lock_frac = 0.9449535181140405,
    /// lock-up weight across compass: lockw_lo + (lockw_hi-lockw_lo)*t
    lockw_lo = 0.5930862690881274,
    lockw_hi = 0.9901777706497228,
    /// contact roughness depth scale
    rough_depth = 0.5244549024215722,
    /// agraffe-return shaping: one-pole corner = img_fc_mul / T1, and the
    /// per-round-trip survival g = img_g_base + img_g_slope * T1 (capped).
    /// Over-smoothing the returning ripple flattens bass partials 5-15.
    img_fc_mul = 1.0049153996616689,
    img_g_base = 0.7456994499471171,
    img_g_slope = 136.05520752462382,
    /// hammer EFFECTIVE striking mass (kg): hm_base + hm_span*(1-t)^hm_pow.
    /// This is the mass the string sees during contact (the head rotates
    /// about the shank flange), roughly half the head mass: ~10 g at A0
    /// falling to ~2 g at C8, with C4 at ~3 g — the Chaigne-Askenfelt C4
    /// simulation value. The old law used full HEAD masses (C4 5.7 g),
    /// which doubled the impedance relaxation time m/(2Z): every contact
    /// was too long and the pulse spectrum fell off an octave too low —
    /// most audible as a dull, null-ridden treble.
    hm_base = 0.0019,
    hm_span = 0.009081009461201344,
    hm_pow = 1.6,
    /// string tension (N): tens_base + tens_span*(1-t)^4
    tens_base = 700.0,
    tens_span = 800.0,
    // --- inharmonicity ---------------------------------------------------
    /// B = 10^(b_lo + b_span*t)
    b_lo = -4.35,
    b_span = 2.7,
    // --- voicing normalisation ------------------------------------------
    trim_ref = 0.0000014,
    top_taper = 1.8,
    // --- attack complex --------------------------------------------------
    /// key-bottom thump: amplitude, lowpass corner (Hz), length (ms), velocity power.
    /// vpow is an AMPLITUDE exponent: 1.4 makes thump POWER grow ~v^2.8,
    /// slightly faster than the tone's ~v^2 — the relative thump then falls
    /// gently toward forte, which is Askenfelt's observation (structure-borne
    /// noise is most audible at soft dynamics). The old vpow=2 grew thump
    /// power v^4 and made forte MORE thumpy than piano, backwards.
    thump_amp = 2.8032681023264323,
    thump_hz = 75.38766655988347,
    thump_ms = 10.43075635256142,
    thump_vpow = 1.4,
    /// action/key resonance burst
    click_amp = 1.177404900554593,
    click_hz = 622.0515133765824,
    click_ms = 22.0,
    click_vpow = 2.0,
    // --- soundboard ------------------------------------------------------
    /// mode damping sigma = board_sig_base + f/board_sig_div
    board_sig_base = 25.226116050700934,
    board_sig_div = 29.099497696648502,
    /// mode spacing ratio (log density)
    board_ratio = 1.074,
    /// modal output tilt scale
    board_tilt = 2.019497104629639,
    /// non-modal direct radiation plateau gain in the board
    board_direct = 4.893033144662416,
    // --- mix -------------------------------------------------------------
    /// per-voice panned direct radiation plateau gain
    direct_string = 4.573637157399161,
    sym_in = 0.002,
    sym_out = 0.35,
    // --- phantom partials / longitudinal modes (0 = off) ----------------
    /// output gain of the per-voice longitudinal/phantom bank
    ph_gain = 0.25,
    /// broadband high-passed squared-signal leak gain
    ph_direct = 0.14530196932570796,
    /// high-pass corner on the squared drive (Hz)
    ph_hp = 2025.1335111269293,
    /// longitudinal mode damping sigma base (1/s) and per-kHz slope
    ph_sigma = 36.507879170477786,
    ph_sigma_slope = 19.06577048950454,
    /// scale on the estimated longitudinal/transverse speed ratio
    ph_ratio = 0.8226252915215968,
    /// per-mode level tilt: mode m gets 1/m^ph_tilt
    ph_tilt = 0.027592217112763565,
    /// drive normalisation (bridge-force units -> unity-ish)
    ph_norm = 1.398605991609466,
    // --- commuted-style body excitation (0 = off) ------------------------
    /// Per-strike diffuse body-tap excitation injected into the string
    /// input, standing for the dense high-order body response the sparse
    /// modal board cannot carry: deterministic noise through a lowpass
    /// whose bandwidth contracts over the burst (tapping a soundboard
    /// sounds like exactly this). Amplitude in force units at ff.
    cs_amp = 22.5,
    /// burst length (ms)
    cs_ms = 120.08456789850372,
    /// initial / final lowpass corner (Hz)
    cs_hi = 4927.404752213667,
    cs_lo = 285.5619002801874,
    /// velocity exponent on (speed / 6 m/s), amplitude-domain. Kept mild
    /// deliberately: the body shock is MOST audible at soft dynamics
    /// (Askenfelt), and listeners preferred the soft-playing "air" it gives;
    /// steeper laws mute it exactly where it is heard.
    cs_vpow = 1.5324616836292717,
    /// tap-length taper toward the treble: len *= ((1-t) + 0.12)^cs_taper
    /// (0 = uniform; treble attacks are ms-scale, bass tens of ms)
    cs_taper = 0.6121728450234013,
    // --- direct hammer-blow shock into the bridge (0 = off) --------------
    /// The blow reaches the bridge as a near-instant compression pulse
    /// (Askenfelt's string precursor: the longitudinal wave arrives ~0.2 ms
    /// after contact at -9..-14 dB re the transverse wave) — a high-passed
    /// copy of the contact force injected into the board input.
    /// Held at a level where the voicing slider (0..2.5) remains an audible
    /// control — the last search pass wanted it near zero, which would have
    /// made the shipped "knock" slider inert.
    knock_amp = 0.010,
    knock_hp = 2543.632464951487,
    // --- soundboard mode count -------------------------------------------
    board_modes = 64.94444007,
    // --- sympathetic coupling ideas beyond one-directional drive ---------
    /// Bath-loading: a sounding string loses energy through the bridge into
    /// every OPEN (undamped) string. First-order coupling to that bath adds
    /// damping to the source: sigma_extra = couple_loss * open_fraction.
    /// Only ever increases damping, so the coupled system stays stable by
    /// construction (1/s at full pedal).
    couple_loss = 1.1490405851666021,
    /// Damped strings still couple: felt heavily damps but does not silence
    /// a string. Relative drive of a DAMPED key's sympathetic bank (its
    /// fast-decaying rotations are already baked by the damper model).
    sym_damped = 0.827246162634356,
    /// Bus chunk-power gate above which damped banks are driven at all.
    sym_gate = 0.0003,
    // --- duplex / aliquot scale (0 = off) --------------------------------
    /// The non-speaking string segments behind the bridge: a shared bank of
    /// lightly damped resonators in the duplex band, rung by bridge force.
    duplex_gain = 0.010756870555741215,
    duplex_sigma = 3.2817462588012702,
    duplex_lo = 1375.3970333862883,
    duplex_hi = 4478.283835,
    // --- una corda: sympathetic drive of the unstruck third string ------
    /// The shifted hammer misses one string of a triple; that string rings
    /// via the bridge at its own detune (smooth aftersound + beat). Small
    /// direct in-gain approximates the bridge transfer.
    uc_third = 0.12,
    // --- per-key voicing scatter (0 = uniform instrument) ----------------
    // Shipped at 0.2: costs ~4 points on the single-note reference metric
    // (each jittered key drifts from its own optimum) but a real piano is
    // 88 individually voiced hammers, not 88 copies — the variation is the
    // point. 0.0 restores the uniform instrument.
    scatter = 0.25,
}

// ---------------------------------------------------------------------------
// Voicing: the runtime mechanism mix. Every field is a multiplier on the
// shipped level of one audible mechanism (1.0 = the reference-matched
// concert grand). These are exactly the mechanisms that read as "different
// pianos" rather than broken/fixed in listening tests, so they are exposed
// as a first-class voicing layer. All of them are safe to set between
// process() calls: they are plain scalars consumed at note-on or per
// chunk — no table rebuild, no allocation.
//
// What is deliberately NOT here: the strike-vs-pluck modal quadrature (a
// struck string's partials start at zero and build as damped sines — see
// modal.rs; making that optional would let the instrument be switched back
// into sounding plucked), the radiation curve, string scaling and unison
// tables (construction-time physics; use Piano::new_with_params for those).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voicing {
    /// Commuted body-tap excitation (the diffuse wooden "body" in the
    /// attack). 0.0 gives the cleaner, digital-piano-like attack.
    pub body_tap: f32,
    /// Direct hammer-blow shock into the bridge (percussive front edge).
    pub knock: f32,
    /// Hammer contact roughness (fortissimo grit).
    pub roughness: f32,
    /// Phantom partials / longitudinal string modes (bass metallic sheen).
    pub phantoms: f32,
    /// Key-bottom thump + action-resonance burst (mechanical key noise).
    pub attack_noise: f32,
    /// Sympathetic resonance sends: open-string bloom, damped-string
    /// coupling, duplex scale and bridge bath-loading together.
    pub sympathetic: f32,
}

impl Default for Voicing {
    fn default() -> Self {
        VoicingPreset::ConcertGrand.voicing()
    }
}

impl Voicing {
    pub(crate) fn clamped(mut self) -> Self {
        let c = |v: &mut f32| {
            *v = if v.is_finite() { v.clamp(0.0, 2.5) } else { 1.0 };
        };
        c(&mut self.body_tap);
        c(&mut self.knock);
        c(&mut self.roughness);
        c(&mut self.phantoms);
        c(&mut self.attack_noise);
        c(&mut self.sympathetic);
        self
    }
}

/// Ready-made voicings. All of them are the same physical instrument; they
/// differ the way two pianos of the same make differ after voicing work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoicingPreset {
    /// The reference-matched instrument: everything at its matched level.
    ConcertGrand,
    /// Harder, more percussive: more knock and grit, a touch more phantom
    /// sheen, slightly drier sympathetic field.
    BrightStage,
    /// Softer salon voicing: felt needled deep, little knock, the room of
    /// sympathetic strings a little more present.
    MellowChamber,
    /// The clean "digital piano" variant: no body-tap in the attack,
    /// reduced mechanism noise — closer to an idealised string+board.
    CleanDigital,
}

impl VoicingPreset {
    pub const ALL: [VoicingPreset; 4] = [
        VoicingPreset::ConcertGrand,
        VoicingPreset::BrightStage,
        VoicingPreset::MellowChamber,
        VoicingPreset::CleanDigital,
    ];

    pub fn name(self) -> &'static str {
        match self {
            VoicingPreset::ConcertGrand => "Concert Grand",
            VoicingPreset::BrightStage => "Bright Stage",
            VoicingPreset::MellowChamber => "Mellow Chamber",
            VoicingPreset::CleanDigital => "Clean Digital",
        }
    }

    pub fn voicing(self) -> Voicing {
        match self {
            VoicingPreset::ConcertGrand => Voicing {
                body_tap: 1.0,
                knock: 1.0,
                roughness: 1.0,
                phantoms: 1.0,
                attack_noise: 1.0,
                sympathetic: 1.0,
            },
            VoicingPreset::BrightStage => Voicing {
                body_tap: 1.0,
                knock: 1.6,
                roughness: 1.3,
                phantoms: 1.15,
                attack_noise: 1.2,
                sympathetic: 0.85,
            },
            VoicingPreset::MellowChamber => Voicing {
                body_tap: 0.9,
                knock: 0.35,
                roughness: 0.55,
                phantoms: 0.8,
                attack_noise: 0.7,
                sympathetic: 1.15,
            },
            VoicingPreset::CleanDigital => Voicing {
                body_tap: 0.0,
                knock: 0.6,
                roughness: 0.8,
                phantoms: 0.9,
                attack_noise: 0.8,
                sympathetic: 0.8,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Instrument presets: named points in (design x voicing x room) space, the
// way a stage piano's instrument list works. A preset with a `design`
// override describes a DIFFERENT INSTRUMENT (other felt, scale or body)
// and needs a rebuild (Piano::new_with_preset — construction, not the
// audio path); a preset without one is a voicing+room of the reference
// instrument and can be applied live (Piano::apply_preset_live).
// Names ending in "(effect)" are honest about being effects reachable
// from a hammer-string model rather than faithful emulations; "Electric
// Tines" is experimental — a tine's bell-like series (B ~ 1 puts partial
// 2 at 4.5x the fundamental) with the body and sympathetic field removed.
// ---------------------------------------------------------------------------

use crate::fx::ReverbPreset;

#[derive(Clone, Copy)]
pub struct PianoPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub voicing: Voicing,
    pub design: Option<fn(&mut DesignParams)>,
    pub room: ReverbPreset,
    pub reverb_mix: f32,
    pub is_default: bool,
}

impl PianoPreset {
    /// The construction-time design for this preset.
    pub fn design_params(&self) -> DesignParams {
        let mut d = DesignParams::default();
        if let Some(f) = self.design {
            f(&mut d);
        }
        d
    }

    /// True if selecting this preset requires rebuilding the instrument.
    pub fn needs_rebuild(&self) -> bool {
        self.design.is_some()
    }
}

const V: Voicing =
    Voicing { body_tap: 1.0, knock: 1.0, roughness: 1.0, phantoms: 1.0, attack_noise: 1.0, sympathetic: 1.0 };

/// The shipped instrument list, ordered acoustic -> character -> effects.
pub const PIANO_PRESETS: &[PianoPreset] = &[
    PianoPreset {
        name: "Concert Grand",
        description: "The reference-matched nine-foot grand in a small hall",
        voicing: V,
        design: None,
        room: ReverbPreset::SmallHall,
        reverb_mix: 0.30,
        is_default: true,
    },
    PianoPreset {
        name: "Studio Grand",
        description: "Close, controlled, a touch more hammer — a pop/session piano",
        voicing: Voicing { knock: 1.2, roughness: 1.1, sympathetic: 0.8, ..V },
        design: Some(|d| d.rad_lp = 3200.0),
        room: ReverbPreset::Studio,
        reverb_mix: 0.22,
        is_default: false,
    },
    PianoPreset {
        name: "Salon Grand",
        description: "Softer felt, gentler blow — a parlour instrument",
        voicing: Voicing { knock: 0.45, roughness: 0.7, attack_noise: 0.8, sympathetic: 1.1, ..V },
        design: Some(|d| {
            d.feltk_lo -= 0.35;
            d.rad_lp = 2000.0;
        }),
        room: ReverbPreset::SmallHall,
        reverb_mix: 0.35,
        is_default: false,
    },
    PianoPreset {
        name: "Romantic Grand",
        description: "Warm, singing, long — an older instrument for the 19th-century repertoire",
        voicing: Voicing { sympathetic: 1.25, phantoms: 1.1, ..V },
        design: Some(|d| {
            d.feltk_lo -= 0.2;
            d.det_lo = 0.35;
            d.t60_base = 22.0;
        }),
        room: ReverbPreset::ConcertHall,
        reverb_mix: 0.40,
        is_default: false,
    },
    PianoPreset {
        name: "Studio Bright",
        description: "Hard-voiced recording piano that cuts through a mix",
        voicing: Voicing { knock: 1.5, roughness: 1.25, attack_noise: 1.1, ..V },
        design: Some(|d| {
            d.feltk_lo += 0.3;
            d.lockw_lo = 5.5;
        }),
        room: ReverbPreset::Studio,
        reverb_mix: 0.18,
        is_default: false,
    },
    PianoPreset {
        name: "Upright",
        description: "Shorter strings, smaller board, the room close around it",
        voicing: Voicing { body_tap: 1.25, sympathetic: 0.7, attack_noise: 1.2, knock: 0.8, ..V },
        design: Some(|d| {
            d.t60_base = 12.0;
            d.spos_lo = 0.115;
            d.board_modes = 48.0;
            d.rad_hp1 = 130.0;
            d.board_sig_base = 60.0;
        }),
        room: ReverbPreset::PracticeRoom,
        reverb_mix: 0.15,
        is_default: false,
    },
    PianoPreset {
        name: "Honky-Tonk",
        description: "Unisons way out of tune, hard worn hammers — saloon piano",
        voicing: Voicing { roughness: 1.5, knock: 1.3, attack_noise: 1.3, ..V },
        design: Some(|d| {
            d.det_lo = 4.0;
            d.det_slope = 5.0;
            d.feltk_lo += 0.35;
            d.b_lo += 0.3;
        }),
        room: ReverbPreset::PracticeRoom,
        reverb_mix: 0.12,
        is_default: false,
    },
    PianoPreset {
        name: "Felt Piano",
        description: "A blanket in the action: the intimate felt-piano sound",
        voicing: Voicing { knock: 0.15, roughness: 0.5, attack_noise: 1.35, body_tap: 0.9, sympathetic: 0.9, ..V },
        design: Some(|d| {
            d.feltk_lo -= 0.7;
            d.feltp_lo = 2.1;
            d.rad_lp = 1500.0;
            d.rad_lp_pow = 0.8;
        }),
        room: ReverbPreset::PracticeRoom,
        reverb_mix: 0.20,
        is_default: false,
    },
    PianoPreset {
        name: "Hard Percussive",
        description: "Voiced up hard for rhythmic playing — all attack",
        voicing: Voicing { knock: 2.0, roughness: 1.6, attack_noise: 1.2, phantoms: 1.2, ..V },
        design: Some(|d| {
            d.feltk_lo += 0.5;
            d.lockw_lo = 6.5;
        }),
        room: ReverbPreset::Studio,
        reverb_mix: 0.15,
        is_default: false,
    },
    PianoPreset {
        name: "Clean Digital",
        description: "The idealised string-and-board sound of a good digital piano",
        voicing: Voicing { body_tap: 0.0, knock: 0.6, roughness: 0.8, attack_noise: 0.8, sympathetic: 0.8, ..V },
        design: None,
        room: ReverbPreset::Studio,
        reverb_mix: 0.25,
        is_default: false,
    },
    PianoPreset {
        name: "Electric Tines",
        description: "Tine keys, pickup-direct (experimental — bell-like second partial, no board)",
        voicing: Voicing { body_tap: 0.0, knock: 0.35, roughness: 0.6, phantoms: 0.0, attack_noise: 0.9, sympathetic: 0.0 },
        design: Some(|d| {
            d.b_lo = 0.0;
            d.b_span = 0.0;
            d.t60_base = 26.0;
            d.t60_pow = 1.0;
            d.t60_min = 1.2;
            d.a2_lo = 0.9;
            d.a2_slope = 0.3;
            d.feltk_lo = 7.2;
            d.feltp_lo = 2.1;
            d.feltp_span = 0.4;
            d.det_lo = 0.12;
            d.det_slope = 0.1;
            d.rad_hp1 = 40.0;
            d.rad_hp2 = 20.0;
            d.rad_lp = 3800.0;
            d.rad_lp_pow = 1.0;
            d.spos_lo = 0.09;
            d.board_modes = 32.0;
            d.board_sig_base = 70.0;
        }),
        room: ReverbPreset::Studio,
        reverb_mix: 0.30,
        is_default: false,
    },
    PianoPreset {
        name: "Tack Piano",
        description: "Tacks in the hammers: thin, jangly, immediate",
        voicing: Voicing { knock: 1.7, roughness: 1.3, ..V },
        design: Some(|d| {
            d.feltk_lo += 0.9;
            d.feltp_lo = 2.9;
            d.det_lo = 2.2;
        }),
        room: ReverbPreset::PracticeRoom,
        reverb_mix: 0.12,
        is_default: false,
    },
    PianoPreset {
        name: "Wire Cembalo (effect)",
        description: "Near-rigid tiny hammers at the string end — plucked-adjacent, not a harpsichord",
        voicing: Voicing { body_tap: 0.0, knock: 0.5, attack_noise: 0.25, roughness: 0.4, sympathetic: 0.6, ..V },
        design: Some(|d| {
            d.feltk_lo = 9.6;
            d.hm_base = 0.0015;
            d.hm_span = 0.002;
            d.spos_lo = 0.07;
            d.spos_hi = 0.05;
            d.t60_base = 9.0;
            d.b_lo = -4.6;
        }),
        room: ReverbPreset::SmallHall,
        reverb_mix: 0.25,
        is_default: false,
    },
    PianoPreset {
        name: "Toy Piano (effect)",
        description: "Little bars in a little box",
        voicing: Voicing { knock: 1.4, attack_noise: 1.4, body_tap: 0.8, sympathetic: 0.3, ..V },
        design: Some(|d| {
            d.b_lo = -1.15;
            d.t60_base = 3.5;
            d.t60_min = 0.4;
            d.board_modes = 24.0;
            d.board_sig_base = 90.0;
            d.rad_hp1 = 300.0;
            d.feltk_lo += 0.2;
            d.hm_base = 0.002;
        }),
        room: ReverbPreset::PracticeRoom,
        reverb_mix: 0.10,
        is_default: false,
    },
    PianoPreset {
        name: "Dampers Lifted",
        description: "The whole instrument open: every string answers every note",
        voicing: Voicing { sympathetic: 2.5, ..V },
        design: None,
        room: ReverbPreset::ConcertHall,
        reverb_mix: 0.35,
        is_default: false,
    },
    PianoPreset {
        name: "Cathedral Wash",
        description: "Lifted dampers in a vast space — for slow, pedalled music",
        voicing: Voicing { sympathetic: 2.2, phantoms: 1.2, ..V },
        design: None,
        room: ReverbPreset::Cathedral,
        reverb_mix: 0.70,
        is_default: false,
    },
    PianoPreset {
        name: "Phantom Metal (effect)",
        description: "The string's nonlinear voice turned all the way up — metallic, breathing",
        voicing: Voicing { phantoms: 2.5, roughness: 1.5, knock: 1.3, sympathetic: 1.2, ..V },
        design: Some(|d| d.a2_lo = 0.03),
        room: ReverbPreset::Cathedral,
        reverb_mix: 0.30,
        is_default: false,
    },
    PianoPreset {
        name: "Dry Close",
        description: "Dead room, short strings, tape-era close pickup",
        voicing: Voicing { sympathetic: 0.25, body_tap: 0.8, knock: 0.9, ..V },
        design: Some(|d| {
            d.t60_base = 8.0;
            d.t60_min = 0.3;
            d.rad_lp = 2200.0;
        }),
        room: ReverbPreset::Studio,
        reverb_mix: 0.08,
        is_default: false,
    },
    PianoPreset {
        name: "Dark Velvet",
        description: "Deep-needled felt and a closed lid",
        voicing: Voicing { roughness: 0.6, knock: 0.4, sympathetic: 1.15, ..V },
        design: Some(|d| {
            d.rad_lp = 1400.0;
            d.rad_lp_pow = 1.3;
            d.feltk_lo -= 0.5;
        }),
        room: ReverbPreset::SmallHall,
        reverb_mix: 0.40,
        is_default: false,
    },
    PianoPreset {
        name: "Glass Grand",
        description: "Open lid, bright wire, air all the way up",
        voicing: Voicing { phantoms: 1.3, knock: 1.2, ..V },
        design: Some(|d| {
            d.rad_lp = 6500.0;
            d.rad_lp_pow = 0.2;
            d.feltk_lo += 0.25;
        }),
        room: ReverbPreset::ConcertHall,
        reverb_mix: 0.30,
        is_default: false,
    },
];
