/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2008 Bo Berry
 *  Copyright (c) 2008-2011 Cisco Systems
 *  Copyright (c) 2022 Intel Corp
 */

#ifndef __SAFE_STR_CONSTRAINT_H__
#define __SAFE_STR_CONSTRAINT_H__

#include "safeclib_private.h"

/*
 * Function used by the libraries to invoke the registered
 * runtime-constraint handler. Always needed.
 */
extern void invoke_safe_str_constraint_handler(const char *msg, void *ptr, errno_t error);

/*
 * Safe C Lib internal string routine to consolidate error handling
 */
static inline void handle_error(char *orig_dest, rsize_t orig_dmax, char *err_msg, errno_t err_code) {
#ifdef SAFECLIB_STR_NULL_SLACK
    /* null string to eliminate partial copy */
    while (orig_dmax) {
        *orig_dest = '\0';
        orig_dmax--;
        orig_dest++;
    }
#else
    *orig_dest = '\0';
#endif

    invoke_safe_str_constraint_handler(err_msg, NULL, err_code);
    return;
}

static inline void handle_wc_error(wchar_t *orig_dest, rsize_t orig_dmax, char *err_msg, errno_t err_code) {
#ifdef SAFECLIB_STR_NULL_SLACK
    /* null string to eliminate partial copy */
    while (orig_dmax) {
        *orig_dest = L'\0';
        orig_dmax--;
        orig_dest++;
    }
#else
    *orig_dest = L'\0';
#endif

    invoke_safe_str_constraint_handler(err_msg, NULL, err_code);
    return;
}

#endif /* __SAFE_STR_CONSTRAINT_H__ */
