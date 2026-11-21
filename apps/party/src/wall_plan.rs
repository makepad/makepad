//! The size the big screen's pictures are decoded at: fitted within the
//! screen, never enlarged, so the screen holds about its own pixels per
//! loop frame, for the loop it shows and the next.

/// A picture's size within `w` x `h` pixels, keeping its aspect and never
/// enlarging it. Picture and screen dimensions are at least one pixel.
pub fn fit_within(width: usize, height: usize, w: usize, h: usize) -> (usize, usize) {
    let scale = (w as f64 / width as f64).min(h as f64 / height as f64).min(1.0);
    ((width as f64 * scale).floor().max(1.0) as usize, (height as f64 * scale).floor().max(1.0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_within_the_screen_without_enlarging() {
        assert_eq!(fit_within(4096, 3072, 1920, 1080), (1440, 1080), "a large photo fits by its height");
        assert_eq!(fit_within(800, 600, 1920, 1080), (800, 600), "a small photo is never enlarged");
        assert_eq!(fit_within(2048, 4096, 1920, 1080), (540, 1080), "a tall photo fits by its height");
        assert_eq!(fit_within(4096, 1024, 1920, 1080), (1920, 480), "a wide photo fits by its width");
        assert_eq!(fit_within(4096, 3072, 1001, 1000), (1001, 750), "rounding stays within the screen");
    }
}
