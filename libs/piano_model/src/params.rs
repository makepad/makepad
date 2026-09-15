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
    /// Low-frequency knee: second-order below rad_hp1 (the board's first
    /// resonance — below it the panel is smaller than the wavelength and
    /// radiation efficiency collapses) times a first-order at rad_hp2.
    /// Re-pinned 2026-08-31 against the real multi-velocity corpus: the
    /// fundamental-to-cluster balance of the bottom six measured keys
    /// (A0..C2, three layers each) demands a knee near 90 Hz — with the
    /// old 40/16 Hz corners the model's C2 FUNDAMENTAL was its strongest
    /// partial (real: -26 dB below the cluster) and A1's sat +17-21 dB
    /// hot: the bass-guitar balance, bought partly by a 50-88 Hz
    /// radiation step that had been calibrated against the falsified MP3
    /// C2 row and is now deleted.
    rad_hp1 = 90.0,
    rad_hp2 = 40.0,
    /// top roll-off corner (Hz)
    rad_lp = 5292.031102101306,
    /// top roll-off order: amplitude (1/(1+(f/lp)^2))^(rad_lp_pow/2); 1.0 = -6 dB/oct
    rad_lp_pow = 0.30407600034647625,
    /// low-mid body emphasis (see soundboard::radiativity): amplitude gain
    /// rad_body on a 4th-order shelf below rad_body_hz — the main-resonance
    /// region of the board, where the middle octaves' fundamentals radiate
    /// velocity-coupling flattening corner (Hz) of the DIRECT radiation
    /// paths (board direct + panned string direct). These paths
    /// differentiate the bridge force and flatten above this corner, so
    /// below it they fall 6 dB/oct — at the old fixed 150 Hz they gutted
    /// 60-150 Hz on the two loudest paths (which also bypass the modal
    /// board's body emphasis entirely): a lean, radio-like lower register.
    rad_vel_hz = 153.24610995333614,
    rad_body = 2.2,
    rad_body_hz = 150.0,
    // --- string losses (keys.rs) ---------------------------------------
    /// fundamental T60 at A0 (s): t60 = t60_base*(1-t)^t60_pow + t60_min
    t60_base = 26.333895237530335,
    t60_pow = 1.8506141782917336,
    /// treble floor raised: the learned lane's corrected multi-velocity
    /// measurements put the physical top-octave fundamental decay 1.6-3.3x
    /// faster than recorded pianos; 0.42 s at C8 was the compounded result
    /// of successive search passes
    t60_min = 0.9,
    /// quadratic-in-frequency loss (1/s per kHz^2): a2 = a2_lo + a2_slope*t.
    /// Between the extremes of two earlier passes: 0.44 slope killed a C6
    /// 5.5 kHz partial in 150 ms (dead treble), but the 0.04/0.010 floor
    /// that replaced it was calibrated against the reference samples'
    /// LOOPED sustain (their late decay is a crossfade artifact) and let
    /// the model's 4.5-6.5 kHz aftersound ring at sigma ~3.8/s (T60 1.8 s)
    /// — measured DOUBLE the recordings' whole-note treble decay over the
    /// trustworthy first second (C5 ref 38 dB/s vs model 18; C6 33 vs 19):
    /// the lingering metallic haze under "bell ring". These values put
    /// C5/C6 whole-note slopes at the reference's own prompt rates while
    /// costing ~2 dB in the first 90 ms.
    a2_lo = 0.09,
    a2_slope = 0.04,
    /// wound-string winding-friction loss (1/s per kHz), scaled (1-t)^3 so
    /// it lives on the copper-wound bass and vanishes by the plain-wire
    /// mids. Coulomb-type inter-winding friction costs a roughly constant
    /// energy fraction per cycle — a loss LINEAR in frequency, distinct
    /// from the f^2 air/viscous term. The old 16.0 was calibrated to the
    /// reference bass's PROMPT decay (sigma ~15/s at C2's p8 over the
    /// first 100 ms) but applied as one constant sigma for all time —
    /// the prompt stage of a double decay pressed onto the aftersound
    /// too. Measured result: the model's bass upper-partial bed sat
    /// 20-27 dB under the recordings over 0-300 ms (A0 2-8 kHz vs
    /// low-band: ref -14.8 dB, model -42) and the whole-note bass
    /// envelope fell at 18 dB/s where the recordings fall at 3-5 — bright
    /// for one instant, then a dead thud: the plucked-guitar signature
    /// the listener flagged. Since the per-partial normal-mode reduction
    /// landed (keys.rs), the PROMPT loss is carried by the bridge
    /// coupling and this term is only the intrinsic winding loss the
    /// aftersound decays at.
    /// 2.0 -> 0.25, 2026-09-01, measured per partial against the REAL
    /// multi-velocity corpus (Salamander C5, 48k/24bit): the real A0's
    /// partials from 200 Hz to 850 Hz all decay at 2-12 dB/s over the
    /// first second (sigma 0.25-1.4, no visible trend with frequency)
    /// and at 1-5 dB/s after; C1 the same. At 2.0 the model's A0 partials
    /// at 400-850 Hz fell at 15-40 dB/s (sigma 1.7-4.6, 3-8x the real)
    /// and its 0.5-2 kHz share dropped from -3 dB to -25 dB over two
    /// seconds where the real note HOLDS -8..-11 dB throughout: the note
    /// collapsed to its p3-p6 cluster within half a second — a plucked
    /// bass, "not a hammered string". A linear-in-f loss of 2/kHz is
    /// simply not in the recordings below 1 kHz; the frequency-dependent
    /// part of the real wound-string loss is the quadratic one below
    /// (a2_wound), which only bites above ~1.5 kHz (the real A0's 2-8 kHz
    /// band decays at ~18 dB/s, sigma ~2).
    a1_wound = 0.25,
    /// wound-string quadratic loss (1/s per kHz^2), scaled (1-t)^3 like
    /// a1_wound: the viscous/air-drag loss of a thick copper-wound string
    /// is larger than plain wire's (bigger diameter, rougher surface).
    /// Sized so the real A0's ~18 dB/s at 2-8 kHz is met (sigma ~2.7 at
    /// 3 kHz with the base a2) while 0.3-1 kHz stays at the measured
    /// 0.3-0.7/s.
    a2_wound = 0.12,
    /// The linear-in-f term the PLAIN-WIRE keys keep (the old a1_wound
    /// value, still scaled (1-t)^3): the wound law above is gated to the
    /// copper-wound strings (idx < 24, ~A#2) with a short blend, because
    /// the mid register was calibrated with this term in place and its
    /// 1-2 kHz sustain is already hotter than the Salamander C4's
    /// (real 1-2 kHz share 210-410 ms: -14 dB; model -8) — taking the
    /// loss away there moved C4 the wrong way.
    a1_plain = 2.0,
    /// quartic-in-frequency loss (1/s per kHz^4): real string losses grow
    /// FASTER than f^2 at the top of the band (air drag leaves the viscous
    /// regime, felt/termination micro-losses); with only the f^2 term one
    /// coefficient cannot both keep C6's 5 kHz partials singing (needs
    /// a2 ~ 0.05) and kill a C4 25th partial at 8 kHz the way real strings
    /// do (reference: -104 dB at 8 kHz where we measured -83). This term
    /// is ~1/s at 5.5 kHz and ~5/s at 8 kHz at the default.
    a4 = 0.0010380816636116807,
    // --- unison / coupling ---------------------------------------------
    /// unison detune (cents): det_lo + det_slope*t. Sets the ANTI
    /// (mistuned anti-phase) mode's offset, i.e. the unison beat rate.
    /// Raised from the searched 0.12: with the old fixed-split structure
    /// beats were microscopic by design; real unisons sit ~1-2 cents
    /// apart and the measured reference beat depths are +-0.3..5 dB.
    det_lo = 0.9,
    det_slope = 0.25,
    /// Bridge-coupling prompt loss (1/s): the vertical-polarisation
    /// coupling scale for the per-partial normal-mode reduction (see
    /// keys.rs). Per partial it is shaped by the squared-and-capped
    /// admittance proxy, so admittance peaks reach ~5x this while
    /// valleys drop to the floor — the measured reference bass shows
    /// prompt sigma 6..34/s on strongly coupled partials and almost none
    /// on others, over an aftersound at 0.02..1.2/s. This irregular
    /// per-partial double decay is what the old fixed Weinreich
    /// multipliers (sigma ratio 4.3 on every partial of every key)
    /// provably could not express — the plucked-harp signature.
    bridge_couple = 20.0,
    /// admittance floor: even off-resonance partials couple somewhat
    bridge_couple_floor = 0.03,
    /// compass taper exponent on (1-t). NOTE the honest discrepancy: the
    /// weak-coupling literature (Woodhouse 2021) puts the coupling scale
    /// at ~2 f0 Z ReY, GROWING toward the treble, while this taper (and
    /// the singles factor in keys.rs) is calibrated the other way from
    /// the Salamander staircases (its bass bridge presents the lowest
    /// admittance to the lowest strings — that is why bass notes last).
    /// Reconciling needs per-position bridge admittance data we do not
    /// have; the calibrated curve reproduces the measured per-note
    /// knees, so it stands.
    bridge_couple_taper = 2.6,
    // --- per-partial normal-mode reduction (keys.rs mode tables) --------
    /// Horizontal-polarisation drive share: the hammer imparts mostly
    /// vertical motion; termination asymmetry leaks this fraction
    /// (amplitude) into the horizontal polarisation, which decays nearly
    /// intrinsically and becomes the aftersound.
    pol_drive = 0.3,
    /// Horizontal radiation share through the bridge relative to
    /// vertical (the bridge's second admittance direction; a real bridge
    /// wants the full 2x2 admittance matrix — this is its second
    /// diagonal, reduced to a share).
    pol_rad = 0.55,
    /// Horizontal bridge-coupling loss as a fraction of the vertical:
    /// the aftersound decays NEARLY intrinsically (measured slow stages
    /// 0.02..1.2 1/s where the old build's slow members ran 0.46..1.68 —
    /// "the pedal is not pressed").
    pol_couple = 0.02,
    /// Bridge-rocking cross coupling between the polarisations, as a
    /// fraction of sqrt(Gv*Gh): mixes the eigenvectors and makes the
    /// residues complex.
    pol_cross = 0.25,
    /// Polarisation detune (cents): the slow false-beat of a held note.
    pol_det = 1.2,
    /// Horizontal intrinsic-loss factor: the t60 law above was fitted as
    /// a SINGLE-decay law, i.e. to the blend of prompt and aftersound;
    /// the aftersound stage itself decays slower (the real C4 holds
    /// ~1.4 dB/s from 1.5 to 4 s — sigma ~0.16 — where the single-decay
    /// law gives 0.71). The horizontal mode's base loss is scaled by
    /// this before the coupling terms are added.
    pol_sig = 0.5,
    /// Anti-phase (mistuned unison) mode: bridge output share (its
    /// radiation is the mistuning residue) and its small share of the
    /// vertical coupling loss (imperfect bridge cancellation).
    anti_gain = 0.18,
    anti_couple = 0.015,
    // --- strike comb -----------------------------------------------------
    /// floor under |sin(n pi x0/L)|: finite hammer width, moving contact
    /// point and non-rigid termination keep real comb nulls shallow
    comb_fill = 0.20626157651445576,
    /// strike position x0/L = spos_lo - (spos_lo-spos_hi)*t^spos_pow
    /// + spos_bass * max(0, 1 - t/spos_bass_t)
    spos_lo = 0.09877467490112493,
    spos_hi = 0.06120083330744137,
    spos_pow = 1.3,
    /// Bass strike-point correction (2026-09-01): the Salamander A0 and C2
    /// ladders carry their strike-comb nulls at partials 8, 16 and 24-26
    /// at every layer, i.e. the real bass is struck at L/8, where the
    /// searched law put A0 at L/10 (nulls at 10 and 20, right where the
    /// recordings hold partials at -10 dB). Adds 0.0262 at A0 (-> 0.125),
    /// fading linearly to nothing by G#3 so the middle and treble keep
    /// their searched positions exactly.
    spos_bass = 0.0262,
    spos_bass_t = 0.40,
    // --- hammer / felt ---------------------------------------------------
    /// felt stiffness exponent of 10: feltk_lo + feltk_span*t
    feltk_lo = 7.44,
    feltk_span = 4.7,
    /// felt power p: feltp_lo + feltp_span*t
    feltp_lo = 2.332016978017561,
    feltp_span = 1.4310157297332198,
    /// Bass-hammer felt regime (2026-09-01, measured against the
    /// Salamander corpus). Below felt_bass_t the felt exponent is lowered
    /// by feltp_bass*(1 - t/felt_bass_t)^2 and log10 K by feltk_bass
    /// times the same ramp, so the bottom octave's hammers work in a
    /// nearly LINEAR-spring regime while everything from ~D#3 up is
    /// untouched. Why: the real bass ladder is velocity-INVARIANT below
    /// ~1 kHz — the Salamander A0's partials 8-20 sit at -25/-24/-24 dB
    /// rel the strongest at pp/mf/ff, C2's at -31/-29/-27 — and the
    /// measured bass contact time varies only ~20-30% over the dynamic
    /// range (Askenfelt & Jansson). With the compass-wide felt law
    /// (p 2.33 at A0) the model's A0 contact ran 6.2 -> 3.0 ms from pp
    /// to ff and its p8-20 swung -35 -> -19 dB (16 dB where the real
    /// instrument shows 1): pianissimo bass was a dull thud and forte a
    /// bright pluck that then died. Heavy, deep, hysteretic bass felt
    /// that never compacts under playing loads behaves far closer to a
    /// linear spring (Stulov; Giordano & Winans report the lowest
    /// dynamic exponents on bass hammers) — the effective exponent here
    /// is that of the felt working against the string's yield, not the
    /// rigid-anvil loading curve.
    /// The bass regime's own felt law: exponent feltp_bass and
    /// log10 K = feltk_bass_lo + feltk_bass_slope*t, blended into the
    /// compass law with weight w = (1 - t/felt_bass_t)^felt_bass_pow
    /// (w = 1 at A0, 0 from felt_bass_t up; felt_bass_t = 0 disables).
    /// Blending log K and p linearly keeps the force at ~1 mm of felt
    /// compression — hence the contact time — continuous across the
    /// blend, so nothing steps.
    /// Measured result at these values (A0 / C2, pp-mf-ff = Salamander
    /// layers 4/9/14): contact 2.8/2.7/2.7 ms and 3.15/3.15/3.15 ms (the
    /// old law: 6.2/3.8/3.0 and 5.5/3.8/3.2); partials 8-20 rel strongest
    /// at 100 ms -25.9/-24.2/-21.2 dB vs real -25/-24/-24, and
    /// -30/-27.5/-25.3 vs real -31.5/-28.7/-27.3. The hammer's
    /// pianissimo-to-fortissimo brightening in the bass now comes from
    /// where the recordings put it — the 2-8 kHz band (real A0 -57/-38/-29,
    /// model -43/-37/-29) — not from the sub-kHz ladder.
    feltp_bass = 1.05,
    feltk_bass_lo = 4.3,
    feltk_bass_slope = 2.2,
    /// the regime holds fully (w = 1) up to felt_bass_t0 (~C2, the top of
    /// the wound doubles), then ramps out to zero at felt_bass_t (~G#3;
    /// A3 and C4 are untouched — C4's own velocity bloom already matches
    /// the recordings, 17 vs 18 dB)
    felt_bass_t0 = 0.17,
    felt_bass_t = 0.40,
    felt_bass_pow = 1.0,
    /// Fraction of the felt's loading/unloading (hysteresis) rate
    /// modulation removed on the same ramp. The (1 + lambda du/dt) factor
    /// stiffens a forte blow's loading ~2.3x against ~1.5x at piano — a
    /// velocity-dependent stiffness on top of the power law. Measured
    /// with every other nonlinearity removed, it alone kept a 6 dB pp->ff
    /// swing in A0's sub-kHz ladder (the real swing is 1 dB); with it
    /// gone the bass pulse shape is exactly velocity-invariant and its
    /// mean stiffening is folded into feltk_bass_lo. The mid/treble
    /// hysteresis is unchanged.
    lambda_bass = 1.0,
    /// extra resting hardness of the last half-octave's hammers, as an
    /// additional exponent of 10 on felt_k ramped over t = 0.75..1.0
    /// (zero at and below C6). The top hammers of a concert grand are
    /// hard-pressed and lacquered: contact stays sub-millisecond even at
    /// piano. The soft-felt integration alone gave C7 a 2.9 ms pianissimo
    /// contact (3-6x the measured instrument) — with the treble speed-range
    /// compression bounding fortissimo, mezzo C7 pulses lost their second
    /// partial entirely (-28 dB; the dull-treble gate sits at -22).
    /// Hardening the resting felt lifts mezzo/piano treble brightness the
    /// way the real instrument gets it: from the hammer, not from the blow.
    feltk_top = 2.0,
    /// mezzo-forte hammer speed (m/s) used for voicing estimates
    v_mf = 2.461108502067713,
    /// Hammer-speed range compression toward the treble, pivoted at v_mf:
    /// speed' = v_mf * (speed/v_mf)^q with
    /// q = 1 - vel_q_depth * clamp((t - vel_q_start)/vel_q_ramp, 0, 1).
    /// The rendered level span from velocity 30 to 127 measured ~23-26 dB
    /// across A0..C4 but 39-48 dB at C5..C7: with the key's own partials
    /// far above the force-pulse corner, every octave the corner moves
    /// with contact time multiplies the level swing, and the top octaves
    /// got twice the dynamic slope of the rest of the compass — forte
    /// trebles leapt out of the texture like struck bells ("bell ring")
    /// while the same keys vanished at piano. A real action cannot do
    /// this either: measured top-octave dynamic ranges are the NARROWEST
    /// on the instrument (light hammers on short key travel bound both
    /// ends of the speed range). The compression is exact in the level
    /// domain (a log-log chain rule): span scales by q, pivoting at the
    /// mezzo-forte point the compass-evenness calibration was done at,
    /// so velocity ~66 renders exactly as before on every key.
    vel_q_depth = 0.42,
    vel_q_start = 0.42,
    vel_q_ramp = 0.30,
    /// lock-up onset as a fraction of mf compression
    lock_frac = 0.95,
    /// lock-up weight across compass: lockw_lo + (lockw_hi-lockw_lo)*t
    lockw_lo = 0.5695980245899225,
    lockw_hi = 1.5,
    /// wood-core second contact stage (treble "ping"): Hertzian stiffness
    /// as a multiple of the key's felt k, weighted onto the top octaves
    /// where the felt is thin enough to bottom out; engages above
    /// core_frac of the mezzo-forte compression. See hammer.rs.
    core_mul = 5.0,
    core_frac = 0.92,
    /// contact roughness depth scale. Calibrated so the ff force-pulse
    /// ripple lands near Askenfelt's measured 5-15 percent — a search had
    /// pushed it to ~30 percent rms, and those broadband sidebands were
    /// most of a +10..16 dB surplus at 4-8 kHz over the reference ("lots
    /// of high freq overtones").
    rough_depth = 0.09,
    /// agraffe-return shaping: one-pole corner = img_fc_mul / T1, and the
    /// per-round-trip survival g = img_g_base + img_g_slope * T1 (capped).
    /// Over-smoothing the returning ripple flattens bass partials 5-15.
    img_fc_mul = 1.0049153996616689,
    img_g_base = 0.8045793152550815,
    img_g_slope = 102.47388602336531,
    /// hammer EFFECTIVE striking mass (kg): hm_base + hm_span*(1-t)^hm_pow.
    /// This is the mass the string sees during contact (the head rotates
    /// about the shank flange), roughly half the head mass: ~10 g at A0
    /// falling to ~2 g at C8, with C4 at ~3 g — the Chaigne-Askenfelt C4
    /// simulation value. The old law used full HEAD masses (C4 5.7 g),
    /// which doubled the impedance relaxation time m/(2Z): every contact
    /// was too long and the pulse spectrum fell off an octave too low —
    /// most audible as a dull, null-ridden treble.
    hm_base = 0.0019,
    hm_span = 0.00962086900952221,
    hm_pow = 1.6,
    /// string tension (N): tens_base + tens_span*(1-t)^4
    tens_base = 700.0,
    tens_span = 800.0,
    // --- inharmonicity ---------------------------------------------------
    /// B = 10^(b_lo + b_span*t).
    /// Fitted to the reference recordings with a sequential partial
    /// tracker: log10 B = -4.771 + 2.891 t (C2 7.1e-5, C4 3.2e-4,
    /// C6 1.9e-3). The old (-4.35, 2.7) law sat ~0.42 decades above that
    /// across the whole compass — C5's 12th partial +190 cents vs the
    /// recording's +104 — and also above the model's OWN string geometry
    /// (B = pi^3 E d^4 / 64 T L^2 from the scaling tables runs 1.4-2.5x
    /// LOWER than the law was claiming). Doubled stretch is inaudible at
    /// piano (partials 8-20 are 30-50 dB down) and lands exactly at forte,
    /// where those partials sit within 15 dB of the peak: a bell, not a
    /// string. The ladder tests are structurally blind to B (they look for
    /// peaks where the model's law predicts them), so this is pinned to
    /// the tracker fit, shaded toward the geometry.
    b_lo = -4.63,
    b_span = 2.78,
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
    thump_amp = 2.43,
    thump_hz = 209.59266202305628,
    thump_ms = 10.593438076609301,
    thump_vpow = 1.4,
    /// action/key resonance burst
    click_amp = 1.05,
    click_hz = 1105.0359893795112,
    click_ms = 22.0,
    click_vpow = 2.0,
    // --- soundboard ------------------------------------------------------
    /// mode damping sigma = board_sig_base + f/board_sig_div.
    /// PINNED to the measured physics, not searched: quality factors land
    /// at Q ~19-37 across the band (Giordano's soundboard measurements:
    /// Q ~20-40). Two earlier passes each raised the damping for a locally
    /// good reason (attack rise, then score) and the cumulative result was
    /// a small dead board — the low-mid after-ring ("body") died 25 dB
    /// faster than a real board's, and per-note noise was papering over
    /// it. The strike still speaks instantly because the DIRECT radiation
    /// paths and the knock carry the transient; the board's own modes are
    /// SUPPOSED to bloom over tens of milliseconds underneath. The
    /// whole-note objective cannot see any of this (it never measures a
    /// release tail), so these are not search knobs.
    board_sig_base = 8.0,
    board_sig_div = 12.0,
    /// mode spacing ratio (log density)
    /// mode spacing ratio (log density): 1.049 puts ~120 modes across
    /// 60 Hz..20 kHz — the dense, overlapping lattice of a LARGE plate
    /// (the old 1.074 spacing spent 65 modes reaching the same span)
    board_ratio = 1.049,
    /// modal output tilt scale
    board_tilt = 2.0591058304844374,
    /// non-modal direct radiation plateau gain in the board
    board_direct = 3.2786425917612156,
    // --- mix -------------------------------------------------------------
    /// per-voice panned direct radiation plateau gain
    direct_string = 2.8456267520038554,
    sym_in = 0.002,
    sym_out = 0.35,
    // --- phantom partials / longitudinal modes (0 = off) ----------------
    /// output gain of the per-voice longitudinal/phantom bank.
    /// 0.25 -> 0.08 (2026-09-01): with the bass hammer re-voiced, the FREE
    /// longitudinal modes were the single largest 2-8 kHz source of a
    /// forte C1 after the attack — the bank alone put the 50-100 ms
    /// 2-8 kHz share at -14 dB against the recording's -33 (A0/C2 were
    /// within 3 dB, C1's three modes at 2.0-2.7 kHz happen to sit inside
    /// the high-passed drive). Bank & Sujbert measured the free
    /// longitudinal mode of a real F1 dying in ~0.15 s and the sustained
    /// phantom content coming from the FORCED response (ph_direct, kept);
    /// the free modes are a colour, not a voice.
    ph_gain = 0.08,
    /// FORCED-response phantom path: the high-passed quadratic signal
    /// itself, fed to the bridge alongside the free-mode bank. Bank &
    /// Sujbert (JASA 2005) measured exactly this split on a recorded F1:
    /// the FREE longitudinal mode died in ~0.15 s while the FORCED
    /// phantoms — sum/difference products of transverse partials — persist
    /// with decay comparable to the partials themselves: sustaining tonal
    /// energy through the bass cluster, resupplied as long as the strings
    /// ring (a mechanism a plucked rendering lacks by definition). A
    /// design search once raised this for cheap high-band score when it
    /// was un-gated and driven by the un-weighted square of everything —
    /// that read as rasp and was zeroed. It is now wound-gated in keys.rs
    /// (full on the bass, gone by C4) and the drive is slope-weighted per
    /// the published equation (see voice.rs), which makes it discrete
    /// partial products, not spray.
    ph_direct = 0.35,
    /// high-pass corner on the squared drive (Hz)
    ph_hp = 1876.8296431768078,
    /// longitudinal mode damping sigma base (1/s) and per-kHz slope
    ph_sigma = 7.699981217781476,
    ph_sigma_slope = 18.781099465421295,
    /// scale on the estimated longitudinal/transverse speed ratio
    /// scale on the estimated longitudinal/transverse speed ratio; the
    /// physical ratio for real scales — searching it far below 1 parks the
    /// longitudinal series among the transverse mid partials, which is not
    /// where phantoms live
    ph_ratio = 0.7600465590066104,
    /// per-mode level tilt: mode m gets 1/m^ph_tilt
    ph_tilt = 0.0,
    /// phantom OUTPUT taper toward the treble: gain *= ((1-t)+0.12)^ph_taper.
    /// Phantom/longitudinal partials are a wound-bass phenomenon; the
    /// per-key drive normalisation alone left the mid/treble bank so hot
    /// that its eight inharmonic tones sat within ~2.5 dB of the true
    /// partials across 2.5-9 kHz on a forte C4 — heard as "grindy, hairy,
    /// frequencies that don't belong".
    /// Structurally bass-weighted: at the default the bank is at full
    /// strength on the wound bass, ~-12 dB by A4 and gone in the treble.
    /// Three separate search passes tried to park a phantom resonator
    /// between the upper partials of MID keys (a 4 kHz inharmonic tone on
    /// A4 measured +36 dB over its phantom-off level) because inter-partial
    /// energy escapes the ladder metric while feeding the noise terms —
    /// and every time the listener called it "grindy / frequencies that
    /// don't belong". Real phantom-partial audibility is a wound-bass
    /// phenomenon; the knob's lower bound now enforces that.
    ph_taper = 1.8,
    /// drive normalisation (bridge-force units -> unity-ish)
    ph_norm = 2.013475554770291,
    // --- commuted-style body excitation (0 = off) ------------------------
    /// Per-strike diffuse body-tap excitation injected into the string
    /// input, standing for the dense high-order body response the sparse
    /// modal board cannot carry: deterministic noise through a lowpass
    /// whose bandwidth contracts over the burst (tapping a soundboard
    /// sounds like exactly this). Amplitude in force units at ff.
    cs_amp = 23.67337834279433,
    /// burst length (ms)
    cs_ms = 45.835740786878716,
    /// fixed spectral-tilt corner (Hz): one of the tap's four poles sits
    /// here permanently, so the burst falls ~-6 dB/oct above it the way a
    /// real board tap does, instead of staying flat noise out to cs_hi
    /// (flat-to-5-kHz per-note noise is exactly "radio static")
    cs_tilt = 2535.693225975999,
    /// initial / final lowpass corner (Hz)
    cs_hi = 4571.594317036107,
    cs_lo = 149.6840196786902,
    /// velocity exponent on (speed / 6 m/s), amplitude-domain. Kept mild
    /// deliberately: the body shock is MOST audible at soft dynamics
    /// (Askenfelt), and listeners preferred the soft-playing "air" it gives;
    /// steeper laws mute it exactly where it is heard.
    cs_vpow = 1.761657571640498,
    /// tap-length taper toward the treble: len *= ((1-t) + 0.12)^cs_taper
    /// (0 = uniform; treble attacks are ms-scale, bass tens of ms)
    cs_taper = 2.5,
    // --- direct hammer-blow shock into the bridge (0 = off) --------------
    /// The blow reaches the bridge as a near-instant compression pulse
    /// (Askenfelt's string precursor: the longitudinal wave arrives ~0.2 ms
    /// after contact at -9..-14 dB re the transverse wave) — a high-passed
    /// copy of the contact force injected into the board input.
    /// Held at a level where the voicing slider (0..2.5) remains an audible
    /// control — the last search pass wanted it near zero, which would have
    /// made the shipped "knock" slider inert.
    knock_amp = 0.0103109771923993,
    knock_hp = 4375.411956759815,
    // --- soundboard mode count -------------------------------------------
    board_modes = 128.0,
    // --- sympathetic coupling ideas beyond one-directional drive ---------
    /// Bath-loading: a sounding string loses energy through the bridge into
    /// every OPEN (undamped) string. First-order coupling to that bath adds
    /// damping to the source: sigma_extra = couple_loss * open_fraction.
    /// Only ever increases damping, so the coupled system stays stable by
    /// construction (1/s at full pedal).
    couple_loss = 1.1355357753472148,
    /// Damped strings still couple: felt heavily damps but does not silence
    /// a string. Relative drive of a DAMPED key's sympathetic bank (its
    /// fast-decaying rotations are already baked by the damper model).
    sym_damped = 0.42394254036533097,
    /// Bus chunk-power gate above which damped banks are driven at all.
    sym_gate = 0.0003,
    // --- duplex / aliquot scale (0 = off) --------------------------------
    /// The non-speaking string segments behind the bridge: a shared bank of
    /// lightly damped resonators in the duplex band, rung by bridge force.
    duplex_gain = 0.010756870555741215,
    duplex_sigma = 3.0,
    duplex_lo = 1403.4127369614819,
    duplex_hi = 4407.2138139258395,
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
    /// How much of the mechanical attack (thump, click, damper felt)
    /// radiates THROUGH the soundboard rather than directly. 0.0 — the
    /// DEFAULT, chosen blind by the listener ("almost like a real
    /// piano") — sends it down a tight case-coloured direct path
    /// (band-limited ~700 Hz, never a raw flat click). 1.0 couples it
    /// through the board: a woodier, more present attack that the same
    /// listener judged "way too much hammer" in direct comparison, kept
    /// as a legitimate voicing. Continuous between.
    pub attack_body: f32,
    /// Sympathetic resonance sends: open-string bloom, damped-string
    /// coupling, duplex scale and bridge bath-loading together.
    pub sympathetic: f32,
}

impl Default for Voicing {
    /// The reference-matched instrument: every mechanism at its shipped
    /// level. This is the approved sound, so it is written out here rather
    /// than reached through a table of named alternatives.
    fn default() -> Self {
        Self {
            body_tap: 1.0,
            knock: 1.0,
            roughness: 1.0,
            phantoms: 1.0,
            attack_noise: 1.0,
            attack_body: 0.0,
            sympathetic: 1.0,
        }
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
        self.attack_body = if self.attack_body.is_finite() { self.attack_body.clamp(0.0, 1.0) } else { 0.0 };
        self
    }
}

// ---------------------------------------------------------------------------
// Instrument presets: a voicing and a room over the reference design, the
// way a stage piano's instrument list works. Applied live
// (Piano::apply_preset_live) — nothing here rebuilds the instrument.
// ---------------------------------------------------------------------------

use crate::fx::ReverbPreset;

/// One shipped instrument: a voicing and a room over the reference design.
///
/// There used to be a `design: Option<fn(&mut DesignParams)>` here, so a
/// preset could describe a DIFFERENT instrument and force a rebuild. Every
/// preset that used it is gone, so the override is gone with it: an
/// instrument is a voicing and a room, and `DesignParams` is reached
/// directly (`Piano::new_with_params`) by the verification tooling that
/// walks the design space.
#[derive(Clone, Copy)]
pub struct PianoPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub voicing: Voicing,
    pub room: ReverbPreset,
    pub reverb_mix: f32,
    pub is_default: bool,
}

const V: Voicing = Voicing {
    body_tap: 1.0,
    knock: 1.0,
    roughness: 1.0,
    phantoms: 1.0,
    attack_noise: 1.0,
    sympathetic: 1.0,
    attack_body: 0.0,
};

/// The shipped instrument list.
///
/// One entry: the reference-matched instrument built from
/// `DesignParams::default()` at the reference voicing. It was twenty-one
/// named departures from this one; the departures are gone. Adding another
/// instrument means adding a row here — that is the whole mechanism.
pub const PIANO_PRESETS: &[PianoPreset] = &[
    PianoPreset {
        name: "Concert Grand",
        description: "The reference-matched nine-foot grand in a small hall",
        voicing: V,
        room: ReverbPreset::SmallHall,
        reverb_mix: 0.30,
        is_default: true,
    },
];
