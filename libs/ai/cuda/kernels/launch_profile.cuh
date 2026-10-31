#pragma once
#include <cuda_runtime.h>
#include <stddef.h>
#include <stdint.h>

// Opt-in cudaEvent graph nodes, read after one instrumented replay. Scopes
// must contain exactly one launch. Rows/cols describe the op, not its grid.
extern thread_local bool makepad_cuda_launch_profile_active;
size_t makepad_cuda_launch_profile_start(const char *name, cudaStream_t stream,
                                        uint32_t rows, uint32_t cols);
void makepad_cuda_launch_profile_stop(size_t slot, cudaStream_t stream);

class MakepadLaunchTiming {
    size_t slot = SIZE_MAX;
    cudaStream_t stream;
public:
    MakepadLaunchTiming(const char *name, cudaStream_t stream, uint32_t rows, uint32_t cols)
        : stream(stream) {
        if (makepad_cuda_launch_profile_active) {
            slot = makepad_cuda_launch_profile_start(name, stream, rows, cols);
        }
    }
    ~MakepadLaunchTiming() {
        if (slot != SIZE_MAX) makepad_cuda_launch_profile_stop(slot, stream);
    }
};

struct MakepadKernelTiming {
    const char *name;
    uint32_t rows, cols;
    double microseconds;
};
