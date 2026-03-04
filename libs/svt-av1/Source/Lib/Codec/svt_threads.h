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

#ifndef EbThreads_h
#define EbThreads_h

#include "definitions.h"

#ifdef _WIN32
#include <windows.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif
// Create wrapper functions that hide thread calls,
// semaphores, mutex, etc. These wrappers also hide
// platform specific implementations of these objects.

/**************************************
     * Threads
     **************************************/
EbHandle svt_create_thread(void* thread_function(void*), void* thread_context);

EbErrorType svt_destroy_thread(EbHandle thread_handle);

/**************************************
     * Semaphores
     **************************************/
EbHandle svt_create_semaphore(uint32_t initial_count, uint32_t max_count);

EbErrorType svt_post_semaphore(EbHandle semaphore_handle);

EbErrorType svt_block_on_semaphore(EbHandle semaphore_handle);

EbErrorType svt_destroy_semaphore(EbHandle semaphore_handle);

/**************************************
     * Mutex
     **************************************/
EbHandle    svt_create_mutex(void);
EbErrorType svt_release_mutex(EbHandle mutex_handle);
EbErrorType svt_block_on_mutex(EbHandle mutex_handle);
EbErrorType svt_destroy_mutex(EbHandle mutex_handle);
#ifndef _WIN32
#ifndef __USE_GNU
#define __USE_GNU
#endif
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <sched.h>
#include <pthread.h>
#endif
#define EB_CREATE_THREAD(pointer, thread_function, thread_context)    \
    do {                                                              \
        pointer = svt_create_thread(thread_function, thread_context); \
        EB_ADD_MEM(pointer, 1, EB_THREAD);                            \
    } while (0)
#define EB_DESTROY_THREAD(pointer)                   \
    do {                                             \
        if (pointer) {                               \
            svt_destroy_thread(pointer);             \
            EB_REMOVE_MEM_ENTRY(pointer, EB_THREAD); \
            pointer = NULL;                          \
        }                                            \
    } while (0);

#define EB_CREATE_THREAD_ARRAY(pa, count, thread_function, thread_contexts) \
    do {                                                                    \
        EB_ALLOC_PTR_ARRAY(pa, count);                                      \
        for (uint32_t i = 0; i < count; i++)                                \
            EB_CREATE_THREAD(pa[i], thread_function, thread_contexts[i]);   \
    } while (0)

#define EB_DESTROY_THREAD_ARRAY(pa, count)       \
    do {                                         \
        if (pa) {                                \
            for (uint32_t i = 0; i < count; i++) \
                EB_DESTROY_THREAD(pa[i]);        \
            EB_FREE_PTR_ARRAY(pa, count);        \
        }                                        \
    } while (0)

void svt_aom_atomic_set_u32(AtomicVarU32* var, uint32_t in);

/*
 Condition variable
*/
typedef struct CondVar {
    int32_t val;
#ifdef _WIN32
    CRITICAL_SECTION   cs;
    CONDITION_VARIABLE cv;
#else
    pthread_mutex_t m_mutex;
    pthread_cond_t  m_cond;
#endif
} CondVar;

EbErrorType svt_set_cond_var(CondVar* cond_var, int32_t newval);
EbErrorType svt_wait_cond_var(CondVar* cond_var, int32_t input);
EbErrorType svt_create_cond_var(CondVar* cond_var);

// once related functions and macros
#ifdef _WIN32
typedef INIT_ONCE OnceType;
#define ONCE_INIT INIT_ONCE_STATIC_INIT
#define ONCE_ROUTINE(name) BOOL CALLBACK name(PINIT_ONCE InitOnce, PVOID Parameter, PVOID* lpContext)
#define ONCE_ROUTINE_EPILOG \
    do {                    \
        return TRUE;        \
    } while (0)
typedef PINIT_ONCE_FN OnceFn;
#else
typedef pthread_once_t OnceType;
#define ONCE_INIT PTHREAD_ONCE_INIT
#define ONCE_ROUTINE(name) void name(void)
#define ONCE_ROUTINE_EPILOG \
    do {                    \
        return;             \
    } while (0)
typedef void (*OnceFn)(void);
#endif
#define DEFINE_ONCE(once_control) static OnceType once_control = ONCE_INIT

// Macro to define a lazily-initialized mutex with once control
// Usage: DEFINE_ONCE_MUTEX(my_mutex)
// Then call: RUN_ONCE_MUTEX(my_mutex) before using svt_block_on_mutex(my_mutex)
#define DEFINE_ONCE_MUTEX(mutex_name)    \
    static EbHandle mutex_name = NULL;   \
    ONCE_ROUTINE(init_##mutex_name) {    \
        mutex_name = svt_create_mutex(); \
        ONCE_ROUTINE_EPILOG;             \
    }                                    \
    DEFINE_ONCE(mutex_name##_once)

#define RUN_ONCE_MUTEX(mutex_name) svt_run_once(&mutex_name##_once, init_##mutex_name)

void svt_run_once(OnceType* once_control, OnceFn init_routine);

#ifdef __cplusplus
}
#endif
#endif // EbThreads_h
