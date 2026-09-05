//! Op-level parity: the DEVICE ops (CUDA on the fleet, the host-backed
//! Metal path on macOS) against the portable reference in [`crate::rife_cpu`],
//! on random tensors — no checkpoint required, so this gate runs anywhere
//! a device backend exists.

#[cfg(test)]
mod tests {
    use crate::backend::{
        gpu_conv2d_planar_strided, gpu_download, gpu_pixel_shuffle_planar,
        gpu_realesrgan_lrelu, gpu_rife_conv_transpose2d, gpu_rife_fill,
        gpu_rife_merge_rgb8, gpu_rife_res_conv, gpu_rife_scale, gpu_rife_warp, gpu_upload,
    };
    use crate::rife::{rife_device_available, ConvWeight, DeconvWeight, ResConvWeight};
    use crate::rife_cpu::{
        conv2d, conv_transpose2d, leaky_relu_in_place, pixel_shuffle, res_conv, warp, Planes,
    };

    /// Deterministic pseudo-random floats in `[-1, 1]`.
    fn noise(seed: u64, len: usize) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    fn planes(seed: u64, channels: usize, width: usize, height: usize) -> Planes {
        let mut p = Planes::new(channels, width, height);
        p.data = noise(seed, channels * width * height);
        p
    }

    fn assert_close(name: &str, a: &[f32], b: &[f32], tol: f32) {
        assert_eq!(a.len(), b.len(), "{name}: length mismatch");
        let mut worst = 0.0f32;
        for (x, y) in a.iter().zip(b.iter()) {
            worst = worst.max((x - y).abs());
        }
        assert!(worst <= tol, "{name}: max abs diff {worst} > {tol}");
    }

    #[test]
    fn device_ops_match_the_reference() {
        if !rife_device_available() {
            return;
        }
        let (w, h) = (23usize, 17usize);

        // conv2d, stride 2, pad 1, k3 — the IFBlock shape.
        let x = planes(1, 5, w, h);
        let cw = ConvWeight {
            in_channels: 5,
            out_channels: 7,
            kw: 3,
            kh: 3,
            stride: 2,
            pad: 1,
            weights: noise(2, 7 * 5 * 9),
            bias: noise(3, 7),
        };
        let cpu = conv2d(&x, &cw).unwrap();
        let dev = gpu_upload(&x.data, 5, w * h).unwrap();
        let dev = gpu_conv2d_planar_strided(
            &dev, w, h, cpu.width, cpu.height, "t", "conv", &cw.weights, &cw.bias, 7, 3, 3,
            1, 1, 2, 2,
        )
        .unwrap();
        assert_close("conv2d s2", &cpu.data, &gpu_download(&dev).unwrap(), 2e-4);

        // conv_transpose2d k4 s2 p1 — the lastconv/encode shape.
        let x = planes(4, 6, w, h);
        let dw = DeconvWeight {
            in_channels: 6,
            out_channels: 5,
            kw: 4,
            kh: 4,
            stride: 2,
            pad: 1,
            weights: noise(5, 6 * 5 * 16),
            bias: noise(6, 5),
        };
        let cpu = conv_transpose2d(&x, &dw).unwrap();
        let dev = gpu_upload(&x.data, 6, w * h).unwrap();
        let dev = gpu_rife_conv_transpose2d(
            &dev, w, h, "t", "deconv", &dw.weights, &dw.bias, 5, 4, 4, 1, 2,
        )
        .unwrap();
        assert_close("deconv", &cpu.data, &gpu_download(&dev).unwrap(), 2e-4);

        // warp with wild flow (exercises the border clamps).
        let x = planes(7, 3, w, h);
        let mut flow = planes(8, 2, w, h);
        for v in flow.data.iter_mut() {
            *v *= 9.0;
        }
        let cpu = warp(&x, &flow).unwrap();
        let dx = gpu_upload(&x.data, 3, w * h).unwrap();
        let df = gpu_upload(&flow.data, 2, w * h).unwrap();
        let dev = gpu_rife_warp(&dx, &df, w, h).unwrap();
        assert_close("warp", &cpu.data, &gpu_download(&dev).unwrap(), 1e-5);

        // res_conv (conv + beta + residual + leaky).
        let x = planes(9, 4, w, h);
        let rw = ResConvWeight {
            conv: ConvWeight {
                in_channels: 4,
                out_channels: 4,
                kw: 3,
                kh: 3,
                stride: 1,
                pad: 1,
                weights: noise(10, 4 * 4 * 9),
                bias: noise(11, 4),
            },
            beta: noise(12, 4),
        };
        let cpu = res_conv(&x, &rw).unwrap();
        let dev_x = gpu_upload(&x.data, 4, w * h).unwrap();
        let dev_conv = gpu_conv2d_planar_strided(
            &dev_x, w, h, w, h, "t", "res", &rw.conv.weights, &rw.conv.bias, 4, 3, 3, 1, 1,
            1, 1,
        )
        .unwrap();
        let dev = gpu_rife_res_conv(&dev_conv, &dev_x, &rw.beta, crate::rife::RIFE_LRELU_SLOPE)
            .unwrap();
        assert_close("res_conv", &cpu.data, &gpu_download(&dev).unwrap(), 2e-4);

        // pixel shuffle x2 (13 planes out, the lastconv layout).
        let x = planes(13, 52, w, h);
        let cpu = pixel_shuffle(&x, 2).unwrap();
        let dev = gpu_upload(&x.data, 52, w * h).unwrap();
        let dev = gpu_pixel_shuffle_planar(&dev, w, h, 13, 2, &vec![0.0; 13]).unwrap();
        assert_close("pixel_shuffle", &cpu.data, &gpu_download(&dev).unwrap(), 1e-6);

        // lrelu + scale + fill.
        let mut x = planes(14, 3, w, h);
        let dev = gpu_upload(&x.data, 3, w * h).unwrap();
        leaky_relu_in_place(&mut x, 0.2);
        let dev = gpu_realesrgan_lrelu(&dev, 0.2).unwrap();
        assert_close("lrelu", &x.data, &gpu_download(&dev).unwrap(), 1e-6);
        let dev = gpu_rife_scale(&dev, 2.5).unwrap();
        let scaled: Vec<f32> = x.data.iter().map(|v| v * 2.5).collect();
        assert_close("scale", &scaled, &gpu_download(&dev).unwrap(), 1e-5);
        let filled = gpu_rife_fill(2, 9, 0.75).unwrap();
        assert_close("fill", &vec![0.75f32; 18], &gpu_download(&filled).unwrap(), 0.0);

        // resize (both directions at the awkward block ratios).
        use crate::backend::gpu_birefnet_resize_bilinear;
        use crate::rife_cpu::resize_bilinear;
        let x = planes(20, 6, 64, 48);
        for (tw, th) in [(4usize, 3usize), (16, 12), (37, 29), (128, 96)] {
            let cpu = resize_bilinear(&x, tw, th);
            let dev = gpu_upload(&x.data, 6, 64 * 48).unwrap();
            let dev = gpu_birefnet_resize_bilinear(&dev, 64, 48, tw, th, false).unwrap();
            assert_close(
                &format!("resize {tw}x{th}"),
                &cpu.data,
                &gpu_download(&dev).unwrap(),
                1e-5,
            );
        }

        // merge: sigmoid blend + crop + quantize.
        let w0 = planes(15, 3, w, h);
        let w1 = planes(16, 3, w, h);
        let mask = planes(17, 1, w, h);
        let plane = w * h;
        let (cw_, ch_) = (w - 3, h - 2);
        let mut expect = vec![0u8; cw_ * ch_ * 3];
        for y in 0..ch_ {
            for x_ in 0..cw_ {
                let src = y * w + x_;
                let m = 1.0 / (1.0 + (-mask.data[src]).exp());
                for c in 0..3 {
                    let v = w0.data[c * plane + src] * m + w1.data[c * plane + src] * (1.0 - m);
                    expect[(y * cw_ + x_) * 3 + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        let d0 = gpu_upload(&w0.data, 3, plane).unwrap();
        let d1 = gpu_upload(&w1.data, 3, plane).unwrap();
        let dm = gpu_upload(&mask.data, 1, plane).unwrap();
        let merged = gpu_rife_merge_rgb8(&d0, &d1, &dm, w, h, cw_, ch_).unwrap();
        assert_eq!(expect, merged, "merge_rgb8");
    }
}
