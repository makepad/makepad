//! glTF preview locomotion: keyboard axes in, a root transform + clip
//! choice out. Pure state — no Cx, no GameWorld — so Asset UI / VJ play
//! authored idle/walk/run/jump clips on a skinned mesh without the Arcade
//! sim.
//!
//! Clips are in place (horizontal pelvis drift is stripped by the motion
//! domain). Movement comes from this controller's transform. Jump is a
//! ballistic arc on y; the viewer floor is y=0.

/// Walk speed, m/s — matches the HY-Motion natural walk (~1.0 m/s) with a
/// touch of game feel.
pub const WALK_SPEED: f32 = 1.25;
/// Run speed, m/s. This is deliberately a little under 2.5x walk speed:
/// distinct enough to feel game-controlled without making the generated
/// run cadence look as though it is skating over the ground.
pub const RUN_SPEED: f32 = 3.0;
/// Yaw turn smoothing rate (exponential approach), 1/s.
pub const TURN_RATE: f32 = 12.0;
/// Playable jump tuning: a 1.0s arc with a 0.75m apex. The native backend
/// trims generated jumps to roughly this physical crouch-to-landing action.
pub const JUMP_SPEED: f32 = 3.0;
pub const GRAVITY: f32 = 6.0;
/// The generated jump is a long, non-looping performance with lead-in and
/// recovery, while play mode needs one responsive action.  Sample this
/// one-second crouch-to-landing window over the controller's ballistic
/// airtime instead of compressing the entire multi-second clip.
pub const JUMP_ACTION_SECONDS: f32 = 1.0;
/// In the campaign v4 source, deepest crouch/landing bracket t=1.267..2.267
/// and the apex is t=1.767: 53% of the 3.333s padded clip.
pub const JUMP_ACTION_CENTER_FRACTION: f32 = 0.53;
/// Keep the character on the viewer's ground slab.
pub const ARENA_HALF: f32 = 7.0;

/// Locomotion states, mapped deterministically onto the motion domain's
/// clip-name contract (idle/walk/run/jump). `run` is optional on older
/// artifacts; the presentation falls back to their walk clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocoState {
    Idle,
    Walk,
    Run,
    Jump,
}

/// Map normalized controller airtime to the measured active part of an
/// authored jump. The endpoints stay inside the clip, and a clip shorter
/// than the target action window is used in full.
pub fn jump_clip_time(air_progress: f32, clip_duration: f32) -> f32 {
    if !clip_duration.is_finite() || clip_duration <= 0.0 {
        return 0.0;
    }
    // Backend-trimmed clips are already the action; tolerate its one-frame
    // export padding rather than trimming it again in the viewer.
    let window = if clip_duration <= JUMP_ACTION_SECONDS * 1.2 {
        clip_duration
    } else {
        JUMP_ACTION_SECONDS
    };
    let center = clip_duration * JUMP_ACTION_CENTER_FRACTION;
    let start = (center - window * 0.5).clamp(0.0, clip_duration - window);
    start + air_progress.clamp(0.0, 1.0) * window
}

impl LocoState {
    /// Clip-name candidates, best fit first, resolved through the engine's
    /// case-insensitive substring matcher (`SkinnedModel::clip_index_any`).
    /// First entries are the motion domain's deterministic contract
    /// (MOTION_CLIP_NAMES); the rest cover hand-authored rig vocabularies
    /// (KayKit/Kenney) so the play mode also works on library rigs.
    pub fn clip_candidates(self) -> &'static [&'static str] {
        match self {
            LocoState::Idle => &["idle", "unarmed_idle", "static"],
            LocoState::Walk => &["walk", "walking_a", "walking_b", "run"],
            LocoState::Run => &["run", "running", "sprint", "jog"],
            LocoState::Jump => &["jump", "jump_start", "jump_idle"],
        }
    }
}

/// One tick of input, already folded from the key set.
#[derive(Clone, Copy, Default)]
pub struct PlayInput {
    /// Strafe axis, -1 (left) ..= 1 (right).
    pub axis_x: f32,
    /// Movement axis, -1 (back) ..= 1 (forward, away from the camera).
    pub axis_z: f32,
    /// Run modifier held this tick. It only has an effect while moving.
    pub run: bool,
    /// Jump key held this tick (the controller edge-detects).
    pub jump: bool,
}

/// The character's playable state: position on the ground plane, facing,
/// and the vertical arc. `update` integrates one fixed step and returns the
/// locomotion state the presentation should show.
pub struct Locomotion {
    pub pos: [f32; 3],
    /// Facing, radians about +Y; drives the model transform.
    pub yaw: f32,
    pub vel_y: f32,
    pub grounded: bool,
    jump_held_prev: bool,
    pub state: LocoState,
}

impl Default for Locomotion {
    fn default() -> Self {
        Self {
            pos: [0.0; 3],
            yaw: 0.0,
            vel_y: 0.0,
            grounded: true,
            jump_held_prev: false,
            state: LocoState::Idle,
        }
    }
}

impl Locomotion {
    /// Integrate one step. `cam_yaw` makes the axes camera-relative: the
    /// scene camera's horizontal forward is `(sin yaw, 0, -cos yaw)`
    /// (scene.rs orbit math), so axis_z pushes along it and axis_x along
    /// screen-right `(cos yaw, 0, sin yaw)`.
    pub fn update(&mut self, dt: f32, input: &PlayInput, cam_yaw: f32) -> LocoState {
        // Camera-relative planar move vector, clamped to unit length so
        // diagonals don't speed up.
        let (fx, fz) = (cam_yaw.sin(), -cam_yaw.cos());
        let (rx, rz) = (cam_yaw.cos(), cam_yaw.sin());
        let mut mx = fx * input.axis_z + rx * input.axis_x;
        let mut mz = fz * input.axis_z + rz * input.axis_x;
        let len = (mx * mx + mz * mz).sqrt();
        if len > 1.0 {
            mx /= len;
            mz /= len;
        }
        let moving = len > 0.05;

        // Face the travel direction (smoothed, wrap-aware). The model's
        // authored facing is the presentation's concern (asset yaw offset).
        if moving {
            let target = mx.atan2(mz);
            let mut delta = target - self.yaw;
            while delta > std::f32::consts::PI {
                delta -= 2.0 * std::f32::consts::PI;
            }
            while delta < -std::f32::consts::PI {
                delta += 2.0 * std::f32::consts::PI;
            }
            self.yaw += delta * (TURN_RATE * dt).min(1.0);
        }

        // Planar travel (air control allowed — viewer, not a platformer).
        // The clip transition is crossfaded by MeshView; keeping the game
        // transform responsive avoids a sluggish half-speed interval when
        // the player presses or releases Shift.
        let move_speed = if input.run { RUN_SPEED } else { WALK_SPEED };
        self.pos[0] = (self.pos[0] + mx * move_speed * dt).clamp(-ARENA_HALF, ARENA_HALF);
        self.pos[2] = (self.pos[2] + mz * move_speed * dt).clamp(-ARENA_HALF, ARENA_HALF);

        // Jump arc: rising edge only, from the ground.
        let jump_pressed = input.jump && !self.jump_held_prev;
        self.jump_held_prev = input.jump;
        if jump_pressed && self.grounded {
            self.vel_y = JUMP_SPEED;
            self.grounded = false;
        }
        if !self.grounded {
            self.vel_y -= GRAVITY * dt;
            self.pos[1] += self.vel_y * dt;
            if self.pos[1] <= 0.0 {
                self.pos[1] = 0.0;
                self.vel_y = 0.0;
                self.grounded = true;
            }
        }

        self.state = if !self.grounded {
            LocoState::Jump
        } else if moving && input.run {
            LocoState::Run
        } else if moving {
            LocoState::Walk
        } else {
            LocoState::Idle
        };
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn run(loco: &mut Locomotion, ticks: usize, input: PlayInput, cam_yaw: f32) -> LocoState {
        let mut state = loco.state;
        for _ in 0..ticks {
            state = loco.update(DT, &input, cam_yaw);
        }
        state
    }

    #[test]
    fn rest_is_idle() {
        let mut loco = Locomotion::default();
        assert_eq!(run(&mut loco, 30, PlayInput::default(), 0.0), LocoState::Idle);
        assert_eq!(loco.pos, [0.0; 3]);
    }

    #[test]
    fn forward_input_walks_away_from_camera_and_faces_travel() {
        let mut loco = Locomotion::default();
        let input = PlayInput { axis_z: 1.0, ..Default::default() };
        // Camera at yaw 0 looks along -Z, so forward = -Z.
        let state = run(&mut loco, 60, input, 0.0);
        assert_eq!(state, LocoState::Walk);
        assert!(loco.pos[2] < -0.9 * WALK_SPEED * 0.9, "traveled -Z: {:?}", loco.pos);
        assert!(loco.pos[0].abs() < 1.0e-3);
        // Facing settles on the travel direction: atan2(0, -1) = PI.
        assert!((loco.yaw.abs() - std::f32::consts::PI).abs() < 0.05, "yaw {}", loco.yaw);
        // Releasing input returns to idle without drift.
        let pos = loco.pos;
        assert_eq!(run(&mut loco, 30, PlayInput::default(), 0.0), LocoState::Idle);
        assert_eq!(loco.pos, pos);
    }

    #[test]
    fn axes_are_camera_relative() {
        // Camera orbited a quarter turn (yaw = PI/2): forward = +X.
        let mut loco = Locomotion::default();
        let input = PlayInput { axis_z: 1.0, ..Default::default() };
        run(&mut loco, 60, input, std::f32::consts::FRAC_PI_2);
        assert!(loco.pos[0] > 0.9, "traveled +X: {:?}", loco.pos);
        assert!(loco.pos[2].abs() < 0.01);
    }

    #[test]
    fn diagonal_does_not_exceed_walk_speed() {
        let mut loco = Locomotion::default();
        let input = PlayInput { axis_x: 1.0, axis_z: 1.0, ..Default::default() };
        run(&mut loco, 60, input, 0.0);
        let dist = (loco.pos[0] * loco.pos[0] + loco.pos[2] * loco.pos[2]).sqrt();
        assert!(dist <= WALK_SPEED * 1.001, "diagonal speed clamped: {dist}");
        assert!(dist >= WALK_SPEED * 0.98);
    }

    #[test]
    fn shift_and_movement_select_distinct_run_at_run_speed() {
        let mut loco = Locomotion::default();
        let input = PlayInput { axis_z: 1.0, run: true, ..Default::default() };
        assert_eq!(run(&mut loco, 60, input, 0.0), LocoState::Run);
        assert!((loco.pos[2].abs() - RUN_SPEED).abs() < 0.02, "run distance: {:?}", loco.pos);

        // Releasing Shift while movement remains held changes the requested
        // clip immediately; MeshView crossfades the actual poses.
        assert_eq!(run(
            &mut loco,
            1,
            PlayInput { axis_z: 1.0, ..Default::default() },
            0.0,
        ), LocoState::Walk);
    }

    #[test]
    fn shift_without_movement_does_not_leave_idle() {
        let mut loco = Locomotion::default();
        let input = PlayInput { run: true, ..Default::default() };
        assert_eq!(run(&mut loco, 60, input, 0.0), LocoState::Idle);
        assert_eq!(loco.pos, [0.0; 3]);
    }

    #[test]
    fn diagonal_run_is_normalized_to_run_speed() {
        let mut loco = Locomotion::default();
        let input = PlayInput {
            axis_x: 1.0,
            axis_z: 1.0,
            run: true,
            ..Default::default()
        };
        run(&mut loco, 60, input, 0.0);
        let dist = (loco.pos[0] * loco.pos[0] + loco.pos[2] * loco.pos[2]).sqrt();
        assert!((dist - RUN_SPEED).abs() < 0.02, "diagonal run distance: {dist}");
    }

    #[test]
    fn jump_arcs_and_lands_back_to_idle() {
        let mut loco = Locomotion::default();
        // Rising edge starts the jump...
        let jumping = PlayInput { jump: true, ..Default::default() };
        assert_eq!(run(&mut loco, 1, jumping, 0.0), LocoState::Jump);
        assert!(!loco.grounded);
        // ...held jump does NOT re-trigger at the apex...
        let mut peak = 0.0f32;
        let mut ticks_airborne = 0;
        for _ in 0..240 {
            if run(&mut loco, 1, jumping, 0.0) == LocoState::Jump {
                ticks_airborne += 1;
                peak = peak.max(loco.pos[1]);
            } else {
                break;
            }
        }
        assert!(loco.grounded, "landed");
        assert_eq!(loco.pos[1], 0.0);
        assert_eq!(loco.state, LocoState::Idle);
        // Ballistic sanity: apex near v^2/2g, airtime near 2v/g.
        let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
        assert!((peak - apex).abs() < 0.1, "apex {peak} vs {apex}");
        let airtime = ticks_airborne as f32 * DT;
        assert!((airtime - 2.0 * JUMP_SPEED / GRAVITY).abs() < 0.1, "airtime {airtime}");
        // Re-press only after release: releasing then pressing jumps again.
        assert_eq!(run(&mut loco, 1, PlayInput::default(), 0.0), LocoState::Idle);
        assert_eq!(run(&mut loco, 1, jumping, 0.0), LocoState::Jump);
    }

    #[test]
    fn generated_jump_uses_centered_active_window_at_near_native_speed() {
        let duration = 100.0 / 30.0;
        let start = jump_clip_time(0.0, duration);
        let apex = jump_clip_time(0.5, duration);
        let end = jump_clip_time(1.0, duration);
        assert!((end - start - JUMP_ACTION_SECONDS).abs() < 1.0e-6);
        assert!((apex - duration * JUMP_ACTION_CENTER_FRACTION).abs() < 1.0e-6);
        assert!((start - 1.266_666_6).abs() < 0.01, "start {start}");
        assert!((end - 2.266_666_7).abs() < 0.01, "end {end}");

        let airtime = 2.0 * JUMP_SPEED / GRAVITY;
        let playback_rate = JUMP_ACTION_SECONDS / airtime;
        assert!(
            (1.0..=1.25).contains(&playback_rate),
            "jump playback should stay near authored speed, got {playback_rate}x"
        );
    }

    #[test]
    fn jump_window_clamps_progress_and_handles_short_or_invalid_clips() {
        assert_eq!(jump_clip_time(-1.0, 0.5), 0.0);
        assert_eq!(jump_clip_time(2.0, 0.5), 0.5);
        assert!((jump_clip_time(0.5, 1.1) - 0.55).abs() < 1.0e-6);
        assert_eq!(jump_clip_time(0.5, f32::NAN), 0.0);
        assert_eq!(jump_clip_time(0.5, 0.0), 0.0);
    }

    #[test]
    fn walk_input_midair_stays_jump_until_landing() {
        let mut loco = Locomotion::default();
        run(&mut loco, 1, PlayInput { jump: true, ..Default::default() }, 0.0);
        let both = PlayInput { axis_z: 1.0, jump: true, ..Default::default() };
        assert_eq!(run(&mut loco, 5, both, 0.0), LocoState::Jump);
        // Lands into walk with input still held.
        let state = run(&mut loco, 240, PlayInput { axis_z: 1.0, ..Default::default() }, 0.0);
        assert_eq!(state, LocoState::Walk);
    }

    #[test]
    fn sprinting_jump_lands_back_into_run() {
        let mut loco = Locomotion::default();
        let takeoff = PlayInput {
            axis_z: 1.0,
            run: true,
            jump: true,
            ..Default::default()
        };
        assert_eq!(run(&mut loco, 1, takeoff, 0.0), LocoState::Jump);
        assert_eq!(
            run(
                &mut loco,
                240,
                PlayInput { axis_z: 1.0, run: true, ..Default::default() },
                0.0,
            ),
            LocoState::Run
        );
    }

    #[test]
    fn arena_clamp_holds_the_slab_edge() {
        let mut loco = Locomotion::default();
        let input = PlayInput { axis_z: 1.0, ..Default::default() };
        run(&mut loco, 60 * 30, input, 0.0);
        assert!(loco.pos[2] >= -ARENA_HALF - 1.0e-3);
        assert!(loco.pos[2] <= -ARENA_HALF + 0.01, "pinned at the edge: {:?}", loco.pos);
    }

    #[test]
    fn clip_candidates_match_the_motion_contract() {
        // The first candidate of each state IS the motion domain's clip name
        // The native motion contract's deterministic input -> clip mapping.
        assert_eq!(LocoState::Idle.clip_candidates()[0], "idle");
        assert_eq!(LocoState::Walk.clip_candidates()[0], "walk");
        assert_eq!(LocoState::Run.clip_candidates()[0], "run");
        assert_eq!(LocoState::Jump.clip_candidates()[0], "jump");
    }
}
