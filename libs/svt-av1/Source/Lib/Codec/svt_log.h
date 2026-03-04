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
#ifndef EbLog_h
#define EbLog_h

#include "EbConfigMacros.h"
#include "EbSvtAv1Enc.h"

#ifndef LOG_TAG
#define LOG_TAG "Svt"
#endif

#if !CONFIG_LOG_QUIET

//SVT_LOG will not output the prefix. you can contorl the output style.
#define SVT_LOG(format, ...) svt_log(SVT_AV1_LOG_ALL, NULL, format, ##__VA_ARGS__)

#define SVT_DEBUG(format, ...) svt_log(SVT_AV1_LOG_DEBUG, LOG_TAG, format, ##__VA_ARGS__)
#define SVT_INFO(format, ...) svt_log(SVT_AV1_LOG_INFO, LOG_TAG, format, ##__VA_ARGS__)
#define SVT_WARN(format, ...) svt_log(SVT_AV1_LOG_WARN, LOG_TAG, format, ##__VA_ARGS__)
#define SVT_ERROR(format, ...) svt_log(SVT_AV1_LOG_ERROR, LOG_TAG, format, ##__VA_ARGS__)
#define SVT_FATAL(format, ...) svt_log(SVT_AV1_LOG_FATAL, LOG_TAG, format, ##__VA_ARGS__)

void svt_log(SvtAv1LogLevel level, const char* tag, const char* format, ...);
void svt_aom_log_set_callback(SvtAv1LogCallback callback, void* context);

#else

// set of macros to silence unused arguments in below logging macros
// currently handle 0 to 7 arguments
#define CALL_MACRO_0()
#define CALL_MACRO_1(arg1) ((void)(arg1))
#define CALL_MACRO_2(arg1, arg2) \
    CALL_MACRO_1(arg1);          \
    CALL_MACRO_1(arg2)
#define CALL_MACRO_3(arg1, arg2, arg3) \
    CALL_MACRO_2(arg1, arg2);          \
    CALL_MACRO_1(arg3)
#define CALL_MACRO_4(arg1, arg2, arg3, arg4) \
    CALL_MACRO_3(arg1, arg2, arg3);          \
    CALL_MACRO_1(arg4)
#define CALL_MACRO_5(arg1, arg2, arg3, arg4, arg5) \
    CALL_MACRO_4(arg1, arg2, arg3, arg4);          \
    CALL_MACRO_1(arg4)
#define CALL_MACRO_6(arg1, arg2, arg3, arg4, arg5, arg6) \
    CALL_MACRO_5(arg1, arg2, arg3, arg4, arg5);          \
    CALL_MACRO_1(arg4)
#define CALL_MACRO_7(arg1, arg2, arg3, arg4, arg5, arg6, arg7) \
    CALL_MACRO_6(arg1, arg2, arg3, arg4, arg5, arg6);          \
    CALL_MACRO_1(arg4)
// ... add more if needed, and adjust below macros

//trick: to support zero param constructor
#define LOG_VA_ARGS(...) , ##__VA_ARGS__

#define GET_MACRO_NAME(_0, _1, _2, _3, _4, _5, _6, _7, NAME, ...) NAME
#define GET_MACRO_NAME2(...) GET_MACRO_NAME(__VA_ARGS__)
#define MAYBE_UNUSED_ARGS(...)                  \
    GET_MACRO_NAME2(x LOG_VA_ARGS(__VA_ARGS__), \
                    CALL_MACRO_7,               \
                    CALL_MACRO_6,               \
                    CALL_MACRO_5,               \
                    CALL_MACRO_4,               \
                    CALL_MACRO_3,               \
                    CALL_MACRO_2,               \
                    CALL_MACRO_1,               \
                    CALL_MACRO_0)(__VA_ARGS__)

#define SVT_LOG(format, ...)            \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)
#define SVT_DEBUG(format, ...)          \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)
#define SVT_INFO(format, ...)           \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)
#define SVT_WARN(format, ...)           \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)
#define SVT_ERROR(format, ...)          \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)
#define SVT_FATAL(format, ...)          \
    do {                                \
        MAYBE_UNUSED_ARGS(__VA_ARGS__); \
    } while (0)

#endif //CONFIG_LOG_QUIET

#endif //EbLog_h
