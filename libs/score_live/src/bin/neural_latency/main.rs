//! How soon an edit to a neural voice's line is heard, and how fast the
//! voice renders, on this machine: `neural_latency [edits]`.
//!
//! A score whose loop sings one line on the neural voice (`engine: @neural`)
//! plays in real time through the same path a game or Stage plays a score
//! (a live composer thread ahead of a clock, the audio graph rendered in
//! device-sized passes, here paced by the wall clock and sent to no device).
//! Every few seconds the line's melody moves between E4 and C5 and its words
//! change; the output's pitch tells when the edited line is first heard. The
//! installed voice sings (`sung::neural::model_path`).
//!
//! The neural voice runs natively only (`makepad-score-player` leaves it out
//! of a browser build), so on wasm the tool is an empty program.

#[cfg(not(target_arch = "wasm32"))]
mod native;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    native::run();
}
