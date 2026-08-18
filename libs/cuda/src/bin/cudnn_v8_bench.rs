//! Isolated cuDNN v7 vs v8 conv microbench. Does not run the UNet walk.

fn main() {
    if let Err(e) = makepad_cuda::cudnn_v8_bench::run() {
        eprintln!("PBR_CUDNN_BENCH_FAIL {e}");
        std::process::exit(1);
    }
}
