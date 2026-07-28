// Oracle: run llama.cpp's clip.cpp vision encoder on an image and dump
// (a) the preprocessed f32 image tensor and (b) the projected output
// embeddings, for validation of the makepad-llama vision implementation.
//
// Build + usage: see build.sh in this directory.
//
// Output format (little-endian):
//   preproc dump:  u32 nx, u32 ny, then nx*ny*3 f32 (RGBRGB planar per clip's layout, raw buf)
//   embd dump:     u32 n_tokens, u32 n_embd, u32 nx, u32 ny (preprocessed), then n_tokens*n_embd f32

#include "clip.h"
#include "clip-impl.h"
#include "mtmd-image.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static std::vector<unsigned char> read_ppm(const char * path, int & w, int & h) {
    FILE * f = fopen(path, "rb");
    if (!f) { fprintf(stderr, "cannot open %s\n", path); exit(1); }
    char magic[3] = {0};
    int maxval = 0;
    if (fscanf(f, "%2s", magic) != 1 || strcmp(magic, "P6") != 0) {
        fprintf(stderr, "not a P6 ppm: %s\n", path); exit(1);
    }
    // skip whitespace/comments
    auto next_int = [&]() {
        int c;
        do {
            c = fgetc(f);
            if (c == '#') { while (c != '\n' && c != EOF) c = fgetc(f); }
        } while (c == ' ' || c == '\n' || c == '\r' || c == '\t');
        ungetc(c, f);
        int v; fscanf(f, "%d", &v); return v;
    };
    w = next_int(); h = next_int(); maxval = next_int();
    fgetc(f); // single whitespace after maxval
    if (maxval != 255) { fprintf(stderr, "maxval must be 255\n"); exit(1); }
    std::vector<unsigned char> rgb((size_t)w * h * 3);
    if (fread(rgb.data(), 1, rgb.size(), f) != rgb.size()) {
        fprintf(stderr, "short read on %s\n", path); exit(1);
    }
    fclose(f);
    return rgb;
}

int main(int argc, char ** argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: %s <mmproj.gguf> <image.ppm> <out_prefix> [--cpu] [--max-tokens N]\n", argv[0]);
        return 1;
    }
    const char * mmproj_path = argv[1];
    const char * image_path  = argv[2];
    std::string  out_prefix  = argv[3];
    bool use_gpu = true;
    int max_tokens = 0;
    for (int i = 4; i < argc; i++) {
        if (strcmp(argv[i], "--cpu") == 0) use_gpu = false;
        if (strcmp(argv[i], "--max-tokens") == 0 && i + 1 < argc) max_tokens = atoi(argv[++i]);
    }

    clip_context_params cparams = {};
    cparams.use_gpu = use_gpu;
    cparams.flash_attn_type = CLIP_FLASH_ATTN_TYPE_DISABLED; // deterministic reference path
    if (max_tokens > 0) cparams.image_max_tokens = max_tokens;

    clip_init_result res = clip_init(mmproj_path, cparams);
    if (!res.ctx_v) { fprintf(stderr, "clip_init failed\n"); return 1; }
    clip_ctx * ctx = res.ctx_v;

    int w = 0, h = 0;
    std::vector<unsigned char> rgb = read_ppm(image_path, w, h);
    fprintf(stderr, "input image: %d x %d\n", w, h);

    clip_image_u8 * img_u8 = clip_image_u8_init();
    clip_build_img_from_pixels(rgb.data(), w, h, img_u8);

    clip_image_f32_batch batch;
    mtmd_image_preprocessor_dyn_size preproc(ctx);
    if (!preproc.preprocess(*img_u8, batch)) {
        fprintf(stderr, "preprocess failed\n"); return 1;
    }
    if (batch.entries.size() != 1) {
        fprintf(stderr, "expected 1 preprocessed image, got %zu\n", batch.entries.size()); return 1;
    }
    clip_image_f32 * img = batch.entries[0].get();
    fprintf(stderr, "preprocessed: %d x %d (buf %zu floats)\n", img->nx, img->ny, img->buf.size());

    // dump preprocessed tensor
    {
        std::string p = out_prefix + ".preproc.bin";
        FILE * f = fopen(p.c_str(), "wb");
        uint32_t nx = img->nx, ny = img->ny;
        fwrite(&nx, 4, 1, f); fwrite(&ny, 4, 1, f);
        fwrite(img->buf.data(), 4, img->buf.size(), f);
        fclose(f);
        fprintf(stderr, "wrote %s\n", p.c_str());
    }

    const int n_tokens = clip_n_output_tokens(ctx, img);
    const int n_embd   = clip_n_mmproj_embd(ctx);
    const int tx = clip_n_output_tokens_x(ctx, img);
    const int ty = clip_n_output_tokens_y(ctx, img);
    fprintf(stderr, "output tokens: %d (%d x %d), embd %d\n", n_tokens, tx, ty, n_embd);

    std::vector<float> embd((size_t)n_tokens * n_embd);
    if (!clip_image_encode(ctx, 4, img, embd.data())) {
        fprintf(stderr, "encode failed\n"); return 1;
    }

    {
        std::string p = out_prefix + ".embd.bin";
        FILE * f = fopen(p.c_str(), "wb");
        uint32_t vals[4] = { (uint32_t)n_tokens, (uint32_t)n_embd, (uint32_t)img->nx, (uint32_t)img->ny };
        fwrite(vals, 4, 4, f);
        fwrite(embd.data(), 4, embd.size(), f);
        fclose(f);
        fprintf(stderr, "wrote %s\n", p.c_str());
    }

    // quick stats so runs are comparable at a glance
    double sum = 0, sum2 = 0;
    for (float v : embd) { sum += v; sum2 += (double)v * v; }
    fprintf(stderr, "embd mean %.6f rms %.6f first8:", sum / embd.size(),
            sqrt(sum2 / embd.size()));
    for (int i = 0; i < 8; i++) fprintf(stderr, " %.5f", embd[i]);
    fprintf(stderr, "\n");

    clip_image_u8_free(img_u8);
    clip_free(ctx);
    return 0;
}
