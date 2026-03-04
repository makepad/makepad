/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifdef _WIN32
#include <sys/timeb.h>
#include <windows.h>
#elif !defined(__USE_POSIX199309)
#define __USE_POSIX199309
#endif

#include <time.h>

#if defined(__MAC_OS_X_VERSION_MIN_REQUIRED) && (__MAC_OS_X_VERSION_MIN_REQUIRED < __MAC_10_12)
#define OLD_MACOS
#endif

#if !defined(CLOCK_MONOTONIC) && !defined(_WIN32) || defined(OLD_MACOS)
#include <sys/time.h>
#endif

#include "svt_time.h"

double svt_av1_compute_overall_elapsed_time_ms(const uint64_t start_seconds, const uint64_t start_useconds,
                                               const uint64_t finish_seconds, const uint64_t finish_useconds) {
    const int64_t s_diff = (int64_t)finish_seconds - (int64_t)start_seconds,
                  u_diff = (int64_t)finish_useconds - (int64_t)start_useconds;
    return (double)s_diff * 1000.0 + (double)u_diff / 1000.0 + 0.5;
}

void svt_av1_get_time(uint64_t* const seconds, uint64_t* const useconds) {
#ifdef _WIN32
    struct _timeb curr_time;
    _ftime_s(&curr_time);
    *seconds  = curr_time.time;
    *useconds = curr_time.millitm * 1000;
#elif defined(CLOCK_MONOTONIC) && !defined(OLD_MACOS)
    struct timespec curr_time;
    clock_gettime(CLOCK_MONOTONIC, &curr_time);
    *seconds  = curr_time.tv_sec;
    *useconds = curr_time.tv_nsec / 1000;
#else
    struct timeval curr_time;
    gettimeofday(&curr_time, NULL);
    *seconds  = curr_time.tv_sec;
    *useconds = curr_time.tv_usec;
#endif
}
