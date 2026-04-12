#include "stable-diffusion.h"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <string>

struct Args {
    std::string diffusion_model;
    std::string vae;
    std::string clip_l;
    std::string t5xxl;
    std::string prompt;
    int64_t seed = 42;
    float cfg_scale = 1.0f;
    int width = 256;
    int height = 256;
    int steps = 20;
    int warmup_runs = 1;
    int measured_runs = 1;
};

static void usage() {
    std::fprintf(
        stderr,
        "usage: flux_ref_warm_bench "
        "--diffusion-model PATH --vae PATH --clip_l PATH --t5xxl PATH "
        "--prompt TEXT --seed N --cfg-scale F --width N --height N --steps N "
        "[--warmup-runs N] [--measured-runs N]\n");
    std::exit(1);
}

static double elapsed_ms(
    const std::chrono::steady_clock::time_point& start,
    const std::chrono::steady_clock::time_point& end
) {
    return std::chrono::duration<double, std::milli>(end - start).count();
}

static void free_generated_images(sd_image_t* images, int count) {
    if (images == nullptr) {
        return;
    }
    for (int i = 0; i < count; ++i) {
        std::free(images[i].data);
    }
    std::free(images);
}

static bool require_value(int argc, int index) {
    return index + 1 < argc;
}

static Args parse_args(int argc, const char** argv) {
    Args args;
    for (int i = 1; i < argc; ++i) {
        const char* arg = argv[i];
        if (std::strcmp(arg, "--diffusion-model") == 0) {
            if (!require_value(argc, i)) usage();
            args.diffusion_model = argv[++i];
        } else if (std::strcmp(arg, "--vae") == 0) {
            if (!require_value(argc, i)) usage();
            args.vae = argv[++i];
        } else if (std::strcmp(arg, "--clip_l") == 0) {
            if (!require_value(argc, i)) usage();
            args.clip_l = argv[++i];
        } else if (std::strcmp(arg, "--t5xxl") == 0) {
            if (!require_value(argc, i)) usage();
            args.t5xxl = argv[++i];
        } else if (std::strcmp(arg, "--prompt") == 0) {
            if (!require_value(argc, i)) usage();
            args.prompt = argv[++i];
        } else if (std::strcmp(arg, "--seed") == 0) {
            if (!require_value(argc, i)) usage();
            args.seed = std::strtoll(argv[++i], nullptr, 10);
        } else if (std::strcmp(arg, "--cfg-scale") == 0) {
            if (!require_value(argc, i)) usage();
            args.cfg_scale = std::strtof(argv[++i], nullptr);
        } else if (std::strcmp(arg, "--width") == 0) {
            if (!require_value(argc, i)) usage();
            args.width = std::atoi(argv[++i]);
        } else if (std::strcmp(arg, "--height") == 0) {
            if (!require_value(argc, i)) usage();
            args.height = std::atoi(argv[++i]);
        } else if (std::strcmp(arg, "--steps") == 0) {
            if (!require_value(argc, i)) usage();
            args.steps = std::atoi(argv[++i]);
        } else if (std::strcmp(arg, "--warmup-runs") == 0) {
            if (!require_value(argc, i)) usage();
            args.warmup_runs = std::atoi(argv[++i]);
        } else if (std::strcmp(arg, "--measured-runs") == 0) {
            if (!require_value(argc, i)) usage();
            args.measured_runs = std::atoi(argv[++i]);
        } else if (
            std::strcmp(arg, "-h") == 0 || std::strcmp(arg, "--help") == 0
        ) {
            usage();
        } else {
            std::fprintf(stderr, "unknown option: %s\n", arg);
            usage();
        }
    }

    if (args.diffusion_model.empty() || args.vae.empty() || args.clip_l.empty() ||
        args.t5xxl.empty() || args.prompt.empty() || args.width <= 0 ||
        args.height <= 0 || args.steps <= 0 || args.warmup_runs < 0 ||
        args.measured_runs <= 0) {
        usage();
    }

    return args;
}

int main(int argc, const char** argv) {
    const Args args = parse_args(argc, argv);

    const auto load_start = std::chrono::steady_clock::now();

    sd_ctx_params_t ctx_params;
    sd_ctx_params_init(&ctx_params);
    ctx_params.vae_decode_only = false;
    ctx_params.free_params_immediately = false;
    ctx_params.diffusion_model_path = args.diffusion_model.c_str();
    ctx_params.vae_path = args.vae.c_str();
    ctx_params.clip_l_path = args.clip_l.c_str();
    ctx_params.t5xxl_path = args.t5xxl.c_str();

    sd_ctx_t* ctx = new_sd_ctx(&ctx_params);
    if (ctx == nullptr) {
        std::fprintf(stderr, "new_sd_ctx failed\n");
        return 1;
    }

    sd_img_gen_params_t gen_params;
    sd_img_gen_params_init(&gen_params);
    gen_params.prompt = args.prompt.c_str();
    gen_params.width = args.width;
    gen_params.height = args.height;
    gen_params.seed = args.seed;
    gen_params.batch_count = 1;
    gen_params.sample_params.sample_steps = args.steps;
    gen_params.sample_params.guidance.txt_cfg = args.cfg_scale;

    if (gen_params.sample_params.sample_method == SAMPLE_METHOD_COUNT) {
        gen_params.sample_params.sample_method = sd_get_default_sample_method(ctx);
    }
    if (gen_params.sample_params.scheduler == SCHEDULER_COUNT) {
        gen_params.sample_params.scheduler = sd_get_default_scheduler(
            ctx,
            gen_params.sample_params.sample_method
        );
    }

    const auto load_end = std::chrono::steady_clock::now();

    std::printf("prompt: %s\n", args.prompt.c_str());
    std::printf(
        "size: %dx%d steps=%d\n",
        args.width,
        args.height,
        args.steps
    );
    std::printf(
        "sampler: %s scheduler=%s\n",
        sd_sample_method_name(gen_params.sample_params.sample_method),
        sd_scheduler_name(gen_params.sample_params.scheduler)
    );
    std::printf("load.ctx_init_ms=%.3f\n", elapsed_ms(load_start, load_end));

    auto run_once = [&](int run_index) -> double {
        gen_params.seed = args.seed + run_index;
        const auto start = std::chrono::steady_clock::now();
        sd_image_t* images = generate_image(ctx, &gen_params);
        const auto end = std::chrono::steady_clock::now();
        if (images == nullptr) {
            std::fprintf(stderr, "generate_image returned null\n");
            free_sd_ctx(ctx);
            std::exit(1);
        }
        free_generated_images(images, gen_params.batch_count);
        return elapsed_ms(start, end);
    };

    for (int i = 0; i < args.warmup_runs; ++i) {
        const double ms = run_once(i);
        std::printf("warmup.run_%d.total_ms=%.3f\n", i + 1, ms);
    }

    double total_ms = 0.0;
    double best_ms = std::numeric_limits<double>::infinity();
    double worst_ms = 0.0;
    for (int i = 0; i < args.measured_runs; ++i) {
        const double ms = run_once(args.warmup_runs + i);
        std::printf("measured.run_%d.total_ms=%.3f\n", i + 1, ms);
        total_ms += ms;
        best_ms = std::min(best_ms, ms);
        worst_ms = std::max(worst_ms, ms);
    }

    std::printf(
        "measured.summary.total_ms.mean=%.3f\n",
        total_ms / args.measured_runs
    );
    std::printf("measured.summary.total_ms.best=%.3f\n", best_ms);
    std::printf("measured.summary.total_ms.worst=%.3f\n", worst_ms);

    free_sd_ctx(ctx);
    return 0;
}
