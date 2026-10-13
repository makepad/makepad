#include "launch_profile.cuh"
#include <new>
#include <vector>

thread_local bool makepad_cuda_launch_profile_active = false;

struct LaunchEvent {
    cudaEvent_t start = nullptr, stop = nullptr;
    MakepadKernelTiming timing = {};
};

static thread_local std::vector<LaunchEvent> events;
static thread_local size_t used = 0;
static thread_local cudaEvent_t step_start = nullptr, step_stop = nullptr;
static thread_local cudaStream_t profile_stream = nullptr;
static thread_local cudaError_t profile_error = cudaSuccess;

static void remember_error(cudaError_t status) {
    if (profile_error == cudaSuccess) profile_error = status;
}

extern "C" void makepad_cuda_launch_profile_clear() {
    makepad_cuda_launch_profile_active = false;
    for (auto &event : events) {
        if (event.start) cudaEventDestroy(event.start);
        if (event.stop) cudaEventDestroy(event.stop);
    }
    events.clear();
    if (step_start) cudaEventDestroy(step_start);
    if (step_stop) cudaEventDestroy(step_stop);
    step_start = step_stop = nullptr;
    used = 0;
}

static bool grow_events(size_t count) {
    const size_t old = events.size();
    try {
        events.resize(count);
    } catch (const std::bad_alloc &) {
        remember_error(cudaErrorMemoryAllocation);
        return false;
    }
    for (size_t i = old; i < count; ++i) {
        remember_error(cudaEventCreate(&events[i].start));
        remember_error(cudaEventCreate(&events[i].stop));
        if (profile_error != cudaSuccess) return false;
    }
    return true;
}

extern "C" cudaError_t makepad_cuda_launch_profile_begin(uint32_t reserve, cudaStream_t stream) {
    if (makepad_cuda_launch_profile_active) return cudaErrorInvalidValue;
    makepad_cuda_launch_profile_clear();
    profile_error = cudaSuccess;
    profile_stream = stream;
    // This is an allocation hint, never a launch limit. Keep event creation
    // out of the measured row; unexpected extra launches grow the storage.
    if (!grow_events(reserve)) return profile_error;
    remember_error(cudaEventCreate(&step_start));
    remember_error(cudaEventCreate(&step_stop));
    remember_error(cudaStreamSynchronize(stream));
    if (profile_error != cudaSuccess) return profile_error;
    makepad_cuda_launch_profile_active = profile_error == cudaSuccess;
    return profile_error;
}

// Explicit event nodes survive capture as timestamps readable after replay.
// Recording around uncaptured <<<launch>>> calls also times any host/driver
// stall between the two submissions, even though no kernel is executing.
extern "C" cudaError_t makepad_cuda_launch_profile_capture_start() {
    if (!makepad_cuda_launch_profile_active) return cudaErrorInvalidValue;
    remember_error(cudaEventRecordWithFlags(step_start, profile_stream, cudaEventRecordExternal));
    return profile_error;
}

extern "C" cudaError_t makepad_cuda_launch_profile_capture_stop() {
    if (!makepad_cuda_launch_profile_active) return cudaErrorInvalidValue;
    remember_error(cudaEventRecordWithFlags(step_stop, profile_stream, cudaEventRecordExternal));
    return profile_error;
}

size_t makepad_cuda_launch_profile_start(const char *name, cudaStream_t stream,
                                        uint32_t rows, uint32_t cols) {
    if (stream != profile_stream) {
        remember_error(cudaErrorInvalidValue);
        return SIZE_MAX;
    }
    if (profile_error != cudaSuccess) return SIZE_MAX;
    if (used == events.size() && !grow_events(used ? used * 2 : 1)) return SIZE_MAX;
    const size_t slot = used++;
    events[slot].timing = {name, rows, cols, 0.0};
    remember_error(cudaEventRecordWithFlags(events[slot].start, stream, cudaEventRecordExternal));
    return slot;
}

void makepad_cuda_launch_profile_stop(size_t slot, cudaStream_t stream) {
    remember_error(cudaEventRecordWithFlags(events[slot].stop, stream, cudaEventRecordExternal));
}

extern "C" cudaError_t makepad_cuda_launch_profile_end(double *span_us, size_t *count) {
    if (!makepad_cuda_launch_profile_active || !span_us || !count) return cudaErrorInvalidValue;
    makepad_cuda_launch_profile_active = false;
    remember_error(cudaEventSynchronize(step_stop));
    if (profile_error != cudaSuccess) return profile_error;
    float ms = 0.0f;
    remember_error(cudaEventElapsedTime(&ms, step_start, step_stop));
    *span_us = (double)ms * 1000.0;
    *count = used;
    for (size_t i = 0; i < used; ++i) {
        remember_error(cudaEventElapsedTime(&ms, events[i].start, events[i].stop));
        events[i].timing.microseconds = (double)ms * 1000.0;
    }
    return profile_error;
}

extern "C" cudaError_t makepad_cuda_launch_profile_read(size_t index, MakepadKernelTiming *out) {
    if (makepad_cuda_launch_profile_active || index >= used || !out) return cudaErrorInvalidValue;
    *out = events[index].timing;
    return cudaSuccess;
}
