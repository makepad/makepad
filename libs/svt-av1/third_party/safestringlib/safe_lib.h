/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2008 Bo Berry
 *  Copyright (c) 2012 Jonathan Toppins <jtoppins@users.sourceforge.net>
 *  Copyright (c) 2008-2013 by Cisco Systems, Inc
 *  Copyright (c) 2021-2022 by Intel Corp
 */

#ifndef __SAFE_LIB_H__
#define __SAFE_LIB_H__

#ifdef __cplusplus
extern "C" {
#endif

/* Define safe_lib version number */
#define SAFEC_VERSION_MAJOR 1
#define SAFEC_VERSION_MINOR 2
#define SAFEC_VERSION_PATCH 0
#define SAFEC_VERSION_STRING "1.2.0"

#define SAFEC_VERSION_NUM(a, b, c) (((a) << 16L) | ((b) << 8) | (c))
#define SAFEC_VERSION SAFEC_VERSION_NUM(SAFEC_VERSION_MAJOR, SAFEC_VERSION_MINOR, SAFEC_VERSION_PATCH)

#include "safe_types.h"
#include "safe_lib_errno.h"

/* C11 appendix K types - specific for bounds checking */
typedef size_t rsize_t;

/*
 * This is the original library decision:
 * We depart from the standard and allow memory and string operations to
 * have different max sizes. See the respective safe_mem_lib.h or
 * safe_str_lib.h files.
 */
/* #define RSIZE_MAX (~(rsize_t)0)  - leave here for completeness */

#ifndef RSIZE_MAX
/* Bring back the standard */
#define RSIZE_MAX (256UL << 20) /* 256MB */
#endif

typedef void (*constraint_handler_t)(const char * /* msg */, void * /* ptr */, errno_t /* error */);

extern void abort_handler_s(const char *msg, void *ptr, errno_t error);
extern void ignore_handler_s(const char *msg, void *ptr, errno_t error);

#define sl_default_handler ignore_handler_s

#ifdef SAFE_MEM_LIB_NEEDED
#include "safe_mem_lib.h"
#endif
#include "safe_str_lib.h"

#ifdef __cplusplus
}
#endif
#endif /* __SAFE_LIB_H__ */
