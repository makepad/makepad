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
//for getenv on windows
#if defined(_WIN32) && !defined(_CRT_SECURE_NO_WARNINGS)
#define _CRT_SECURE_NO_WARNINGS
#endif
#include "svt_log.h"
#include "svt_threads.h"
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>

#if !CONFIG_LOG_QUIET

static const char* log_level_str(SvtAv1LogLevel level) {
    switch (level) {
    case SVT_AV1_LOG_FATAL:
        return "fatal";
    case SVT_AV1_LOG_ERROR:
        return "error";
    case SVT_AV1_LOG_WARN:
        return "warn";
    case SVT_AV1_LOG_INFO:
        return "info";
    case SVT_AV1_LOG_DEBUG:
        return "debug";
    default:
        return "unknown";
    }
}

struct DefaultCtx {
    SvtAv1LogLevel level;
    FILE*          file;
};

/**
 * @brief Global logger structure
 *
 * Handles both default cases and custom loggers.
 */
static struct Logger {
    SvtAv1LogCallback fn;
    // If fn == default_logger, ctx is DefaultCtx*
    void* ctx;
}* g_logger;

static void default_logger(void* context, SvtAv1LogLevel level, const char* tag, const char* fmt, va_list args) {
    struct DefaultCtx* logger = context;
    if (level > logger->level) {
        return;
    }
    if (tag) {
        fprintf(logger->file, "%s[%s]: ", tag, log_level_str(level));
    }
    vfprintf(logger->file, fmt, args);
    fflush(logger->file);
}

static void logger_cleanup(void) {
    if (g_logger->fn == default_logger) {
        struct DefaultCtx* ctx = g_logger->ctx;
        if (ctx->file && ctx->file != stderr) {
            fclose(ctx->file);
        }
        free(ctx);
    }
    free(g_logger);
    g_logger = NULL;
}

// file scoped variable just to handle the init for a custom logger.
// It's only ever read once.
// Any writes after being read are ignored.
static struct Logger custom_logger_input;

static ONCE_ROUTINE(logger_create) {
    g_logger = calloc(1, sizeof(*g_logger));
    if (!g_logger) {
        // Not sure what else to do here...
        ONCE_ROUTINE_EPILOG;
    }
    struct Logger custom = custom_logger_input;

    if (custom.fn && custom.fn != default_logger) {
        // A custom logger was requested before initialization
        *g_logger = custom;
        atexit(logger_cleanup);
        ONCE_ROUTINE_EPILOG;
    }

    // Default logger
    struct DefaultCtx* ctx = calloc(1, sizeof(*ctx));
    if (!ctx) {
        free(g_logger);
        g_logger = NULL;
        ONCE_ROUTINE_EPILOG;
    }

    const char* log = getenv("SVT_LOG");
    ctx->level      = SVT_AV1_LOG_INFO;
    if (log) {
        const int new_level = atoi(log);
        if (new_level >= SVT_AV1_LOG_ALL && new_level <= SVT_AV1_LOG_DEBUG) {
            ctx->level = (SvtAv1LogLevel)new_level;
        }
    }

    const char* file = getenv("SVT_LOG_FILE");
    if (file) {
        FOPEN(ctx->file, file, "w+");
    }
    // If the file couldn't be opened, fall back to stderr
    if (!ctx->file) {
        ctx->file = stderr;
    }

    g_logger->fn  = default_logger;
    g_logger->ctx = ctx;
    atexit(logger_cleanup);
    ONCE_ROUTINE_EPILOG;
}

DEFINE_ONCE(g_logger_once);

static struct Logger* get_logger() {
    svt_run_once(&g_logger_once, logger_create);
    return g_logger;
}

void svt_aom_log_set_callback(SvtAv1LogCallback callback, void* context) {
    // We only want to allow using a custom context if a custom callback is provided.
    // Otherwise the context for the default logger will be incorrect.
    // This will still allow passing NULL for context in case the callback doesn't use it.
    // Due to how the rest of the code is structured, this only takes effect once,
    // and only if done before any logging happens.
    if (callback) {
        custom_logger_input.fn  = callback;
        custom_logger_input.ctx = context;
    }
}

void svt_log(SvtAv1LogLevel level, const char* tag, const char* format, ...) {
    struct Logger* logger = get_logger();
    if (!logger) {
        return;
    }
    va_list args;
    va_start(args, format);
    logger->fn(logger->ctx, level, tag, format, args);
    va_end(args);
}

#endif //CONFIG_LOG_QUIET
