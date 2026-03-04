/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2008 Bo Berry
 *  Copyright (c) 2008-2011 Cisco Systems
 */

#include "safeclib_private.h"
#include "safe_str_constraint.h"
#include "safe_str_lib.h"

/**
 * NAME
 *    strnlen_s
 *
 * SYNOPSIS
 *    #include "safe_str_lib.h"
 *    rsize_t
 *    strnlen_s(const char *dest, rsize_t dmax)
 *
 * DESCRIPTION
 *    The strnlen_s function computes the length of the string pointed
 *    to by dest.
 *
 * SPECIFIED IN
 *    ISO/IEC TR 24731-1, Programming languages, environments
 *    and system software interfaces, Extensions to the C Library,
 *    Part I: Bounds-checking interfaces
 *
 * INPUT PARAMETERS
 *    dest      pointer to string
 *
 *    dmax      restricted maximum length.
 *
 * OUTPUT PARAMETERS
 *    none
 *
 * RUNTIME CONSTRAINTS
 *    dest shall not be a null pointer
 *    dmax shall not be greater than RSIZE_MAX_STR
 *    dmax shall not equal zero
 *
 * RETURN VALUE
 *    The function returns the string length, excluding  the terminating
 *    null character.  If dest is NULL, then strnlen_s returns 0.
 *
 *    Otherwise, the strnlen_s function returns the number of characters
 *    that precede the terminating null character. If there is no null
 *    character in the first dmax characters of dest then strnlen_s returns
 *    dmax. At most the first dmax characters of dest are accessed
 *    by strnlen_s.
 *
 * ALSO SEE
 *    strnterminate_s()
 *
 */
rsize_t strnlen_s(const char *dest, rsize_t dmax) {
    rsize_t count;

    if (dest == NULL) {
        return RCNEGATE(0);
    }

    if (dmax == 0) {
        invoke_safe_str_constraint_handler("strnlen_s: dmax is 0", NULL, ESZEROL);
        return RCNEGATE(0);
    }

    if (dmax > RSIZE_MAX_STR) {
        invoke_safe_str_constraint_handler("strnlen_s: dmax exceeds max", NULL, ESLEMAX);
        return RCNEGATE(0);
    }

    count = 0;
    while (dmax && *dest) {
        count++;
        dmax--;
        dest++;
    }

    return RCNEGATE(count);
}
EXPORT_SYMBOL(strnlen_s)
