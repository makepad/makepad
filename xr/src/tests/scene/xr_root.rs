mod tests {
    use super::*;

    fn sample(captured_at: f64, vertical_split: f32) -> BoxSyncPoseSample {
        BoxSyncPoseSample {
            anchor: XrAnchor {
                left: vec3f(-0.1, vertical_split * 0.5, -0.4),
                right: vec3f(0.1, -vertical_split * 0.5, -0.4),
            },
            captured_at,
            vertical_split,
        }
    }

    #[test]
    fn box_sync_detector_emits_every_full_reversal_extrema() {
        let mut runtime = XrSyncAnchorRuntime::default();
        let mut emitted = Vec::new();
        let samples = [
            sample(0.00, 0.00),
            sample(0.10, 0.03),
            sample(0.20, 0.06),
            sample(0.30, 0.03),
            sample(0.40, 0.00),
            sample(0.50, -0.03),
            sample(0.60, -0.06),
            sample(0.70, -0.03),
            sample(0.80, 0.00),
            sample(0.90, 0.03),
            sample(1.00, 0.06),
        ];

        for sample in samples {
            if let Some(sync_anchor) = runtime.update_detector_sample(Some(sample)) {
                emitted.push(sync_anchor);
            }
        }

        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[0].extrema, XrSyncAnchorExtrema::High);
        assert_eq!(emitted[0].captured_at, 0.20);
        assert_eq!(emitted[1].extrema, XrSyncAnchorExtrema::Low);
        assert_eq!(emitted[1].captured_at, 0.60);
    }
}
