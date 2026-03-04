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
#ifndef EbMalloc_h
#define EbMalloc_h
#include <stdlib.h>
#include <stdio.h>
#include <stdint.h>

#include "definitions.h"

#ifndef NDEBUG
#define DEBUG_MEMORY_USAGE
#endif

//-------aom memory stuff

#define ADDRESS_STORAGE_SIZE sizeof(size_t)
#define DEFAULT_ALIGNMENT (2 * sizeof(void*))
#define AOM_MAX_ALLOCABLE_MEMORY 8589934592 // 8 GB
/*returns an addr aligned to the byte boundary specified by align*/
#define align_addr(addr, align) (void*)(((size_t)(addr) + ((align) - 1)) & ~(size_t)((align) - 1))

// Returns 0 in case of overflow of nmemb * size.
static inline int32_t check_size_argument_overflow(uint64_t nmemb, uint64_t size) {
    const uint64_t total_size = nmemb * size;
    if (nmemb == 0) {
        return 1;
    }
    if (size > AOM_MAX_ALLOCABLE_MEMORY / nmemb) {
        return 0;
    }
    if (total_size != (size_t)total_size) {
        return 0;
    }
    return 1;
}

static inline size_t get_aligned_malloc_size(size_t size, size_t align) {
    return size + align - 1 + ADDRESS_STORAGE_SIZE;
}

static inline size_t* get_malloc_address_location(void* const mem) {
    return ((size_t*)mem) - 1;
}

static inline void set_actual_malloc_address(void* const mem, const void* const malloc_addr) {
    size_t* const malloc_addr_location = get_malloc_address_location(mem);
    *malloc_addr_location              = (size_t)malloc_addr;
}

static inline void* get_actual_malloc_address(void* const mem) {
    const size_t* const malloc_addr_location = get_malloc_address_location(mem);
    return (void*)(*malloc_addr_location);
}

static inline void* svt_aom_memalign(size_t align, size_t size) {
    void*        x            = NULL;
    const size_t aligned_size = get_aligned_malloc_size(size, align);
#if defined(AOM_MAX_ALLOCABLE_MEMORY)
    if (!check_size_argument_overflow(1, aligned_size)) {
        return NULL;
    }
#endif
    void* const addr = malloc(aligned_size);
    if (addr) {
        x = align_addr((uint8_t*)addr + ADDRESS_STORAGE_SIZE, align);
        set_actual_malloc_address(x, addr);
    }
    return x;
}

static inline void* svt_aom_malloc(size_t size) {
    return svt_aom_memalign(DEFAULT_ALIGNMENT, size);
}

static inline void svt_aom_free(void* memblk) {
    if (memblk) {
        void* addr = get_actual_malloc_address(memblk);
        free(addr);
    }
}

static inline void* svt_aom_memset16(void* dest, int32_t val, size_t length) {
    size_t    i;
    uint16_t* dest16 = (uint16_t*)dest;
    for (i = 0; i < length; i++) {
        *dest16++ = (uint16_t)val;
    }
    return dest;
}

//-------------------------------
void svt_print_alloc_fail_impl(const char* file, int line);

#ifdef DEBUG_MEMORY_USAGE
void svt_print_memory_usage(void);
void svt_increase_component_count(void);
void svt_decrease_component_count(void);
void svt_add_mem_entry_impl(void* ptr, EbPtrType type, size_t count, const char* file, uint32_t line);
void svt_remove_mem_entry(void* ptr, EbPtrType type);

#define EB_ADD_MEM_ENTRY(p, type, count) svt_add_mem_entry_impl(p, type, count, __FILE__, EB_LINE_NUM)
#define EB_REMOVE_MEM_ENTRY(p, type) svt_remove_mem_entry(p, type);

#else
#define svt_print_memory_usage() \
    do {                         \
    } while (0)
#define svt_increase_component_count() \
    do {                               \
    } while (0)
#define svt_decrease_component_count() \
    do {                               \
    } while (0)
#define EB_ADD_MEM_ENTRY(p, type, count) \
    do {                                 \
    } while (0)
#define EB_REMOVE_MEM_ENTRY(p, type) \
    do {                             \
    } while (0)

#endif //DEBUG_MEMORY_USAGE

#define EB_NO_THROW_ADD_MEM(p, size, type)                    \
    do {                                                      \
        if (!p)                                               \
            svt_print_alloc_fail_impl(__FILE__, EB_LINE_NUM); \
        else                                                  \
            EB_ADD_MEM_ENTRY(p, type, size);                  \
    } while (0)

#define EB_CHECK_MEM(p)                           \
    do {                                          \
        if (!p)                                   \
            return EB_ErrorInsufficientResources; \
    } while (0)

#define EB_ADD_MEM(p, size, type)           \
    do {                                    \
        EB_NO_THROW_ADD_MEM(p, size, type); \
        EB_CHECK_MEM(p);                    \
    } while (0)

#define EB_NO_THROW_MALLOC(pointer, size)                \
    do {                                                 \
        void* malloced_p = malloc(size);                 \
        EB_NO_THROW_ADD_MEM(malloced_p, size, EB_N_PTR); \
        pointer = malloced_p;                            \
    } while (0)

#define EB_MALLOC(pointer, size)           \
    do {                                   \
        EB_NO_THROW_MALLOC(pointer, size); \
        EB_CHECK_MEM(pointer);             \
    } while (0)

#define EB_MALLOC_NO_CHECK(pointer, size)  \
    do {                                   \
        EB_NO_THROW_MALLOC(pointer, size); \
    } while (0)

#define EB_MALLOC_OBJECT(pointer)                        \
    do {                                                 \
        EB_NO_THROW_MALLOC(pointer, sizeof(*(pointer))); \
        EB_CHECK_MEM(pointer);                           \
    } while (0)

#define EB_MALLOC_OBJECT_NO_CHECK(pointer)               \
    do {                                                 \
        EB_NO_THROW_MALLOC(pointer, sizeof(*(pointer))); \
    } while (0)

#define EB_NO_THROW_CALLOC(pointer, count, size)              \
    do {                                                      \
        pointer = calloc(count, size);                        \
        EB_NO_THROW_ADD_MEM(pointer, count * size, EB_C_PTR); \
    } while (0)

#define EB_CALLOC(pointer, count, size)           \
    do {                                          \
        EB_NO_THROW_CALLOC(pointer, count, size); \
        EB_CHECK_MEM(pointer);                    \
    } while (0)

#define EB_CALLOC_NO_CHECK(pointer, count, size)  \
    do {                                          \
        EB_NO_THROW_CALLOC(pointer, count, size); \
    } while (0)

#define EB_FREE(pointer)                        \
    do {                                        \
        EB_REMOVE_MEM_ENTRY(pointer, EB_N_PTR); \
        free(pointer);                          \
        pointer = NULL;                         \
    } while (0)

#define EB_FREE_ARRAY(pa) EB_FREE(pa);

#define EB_MALLOC_ARRAY(pa, count)              \
    do {                                        \
        EB_MALLOC(pa, sizeof(*(pa)) * (count)); \
    } while (0)

#define EB_MALLOC_ARRAY_NO_CHECK(pa, count)              \
    do {                                                 \
        EB_MALLOC_NO_CHECK(pa, sizeof(*(pa)) * (count)); \
    } while (0)

#define EB_REALLOC_ARRAY(pa, count)            \
    do {                                       \
        size_t s_ra = sizeof(*(pa)) * (count); \
        void*  p_ra = realloc(pa, s_ra);       \
        if (p_ra) {                            \
            EB_REMOVE_MEM_ENTRY(pa, EB_N_PTR); \
            EB_ADD_MEM(p_ra, s_ra, EB_N_PTR);  \
        } else {                               \
            EB_FREE(pa);                       \
        }                                      \
        pa = p_ra;                             \
    } while (0)

#define EB_REALLOC_ARRAY_NO_CHECK(pa, count)           \
    do {                                               \
        size_t s_ra = sizeof(*(pa)) * (count);         \
        void*  p_ra = realloc(pa, s_ra);               \
        if (p_ra) {                                    \
            EB_REMOVE_MEM_ENTRY(pa, EB_N_PTR);         \
            EB_NO_THROW_ADD_MEM(p_ra, s_ra, EB_N_PTR); \
        } else {                                       \
            EB_FREE(pa);                               \
        }                                              \
        pa = p_ra;                                     \
    } while (0)

#define EB_CALLOC_ARRAY(pa, count)           \
    do {                                     \
        EB_CALLOC(pa, count, sizeof(*(pa))); \
    } while (0)

#define EB_CALLOC_ARRAY_NO_CHECK(pa, count)           \
    do {                                              \
        EB_CALLOC_NO_CHECK(pa, count, sizeof(*(pa))); \
    } while (0)

#define EB_ALLOC_PTR_ARRAY(pa, count)        \
    do {                                     \
        EB_CALLOC(pa, count, sizeof(*(pa))); \
    } while (0)

#define EB_FREE_PTR_ARRAY(pa, count)           \
    do {                                       \
        if (pa) {                              \
            for (size_t i = 0; i < count; i++) \
                EB_FREE(pa[i]);                \
            EB_FREE(pa);                       \
        }                                      \
    } while (0)

#define EB_MALLOC_2D(p2d, width, height)             \
    do {                                             \
        EB_MALLOC_ARRAY(p2d, width);                 \
        EB_MALLOC_ARRAY(p2d[0], (width) * (height)); \
        for (size_t w = 1; w < (width); w++)         \
            p2d[w] = p2d[0] + w * (height);          \
    } while (0)

#define EB_CALLOC_2D(p2d, width, height)             \
    do {                                             \
        EB_MALLOC_ARRAY(p2d, width);                 \
        EB_CALLOC_ARRAY(p2d[0], (width) * (height)); \
        for (size_t w = 1; w < (width); w++)         \
            p2d[w] = p2d[0] + w * (height);          \
    } while (0)

#define EB_FREE_2D(p2d)            \
    do {                           \
        if (p2d)                   \
            EB_FREE_ARRAY(p2d[0]); \
        EB_FREE_ARRAY(p2d);        \
    } while (0)

#ifdef _WIN32
#define EB_MALLOC_ALIGNED(pointer, size)          \
    do {                                          \
        pointer = _aligned_malloc(size, ALVALUE); \
        EB_ADD_MEM(pointer, size, EB_A_PTR);      \
    } while (0)

#define EB_FREE_ALIGNED(pointer)                \
    do {                                        \
        EB_REMOVE_MEM_ENTRY(pointer, EB_A_PTR); \
        _aligned_free(pointer);                 \
        pointer = NULL;                         \
    } while (0)
#else
#define EB_MALLOC_ALIGNED(pointer, size)                            \
    do {                                                            \
        if (posix_memalign((void**)&(pointer), ALVALUE, size) != 0) \
            return EB_ErrorInsufficientResources;                   \
        EB_ADD_MEM(pointer, size, EB_A_PTR);                        \
    } while (0)

#define EB_FREE_ALIGNED(pointer)                \
    do {                                        \
        EB_REMOVE_MEM_ENTRY(pointer, EB_A_PTR); \
        free(pointer);                          \
        pointer = NULL;                         \
    } while (0)
#endif

#define EB_MALLOC_ALIGNED_ARRAY(pa, count) EB_MALLOC_ALIGNED(pa, sizeof(*(pa)) * (count))

#define EB_CALLOC_ALIGNED_ARRAY(pa, count)              \
    do {                                                \
        EB_MALLOC_ALIGNED(pa, sizeof(*(pa)) * (count)); \
        memset(pa, 0, sizeof(*(pa)) * (count));         \
    } while (0)

#define EB_FREE_ALIGNED_ARRAY(pa) EB_FREE_ALIGNED(pa)

#endif //EbMalloc_h
