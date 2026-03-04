/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2012 Jonathan Toppins <jtoppins@users.sourceforge.net>
 *  Copyright (c) 2012 Cisco Systems
 *  Copyright (c) 2018-2022 Intel Corp
 */

#include "safeclib_private.h"

/**
 * NAME
 *    ignore_handler_s
 *
 * SYNOPSIS
 *    #include "safe_lib.h"
 *    void ignore_handler_s(const char *msg, void *ptr, errno_t error)
 *
 * DESCRIPTION
 *    This function simply returns to the caller.
 *
 * SPECIFIED IN
 *    ISO/IEC JTC1 SC22 WG14 N1172, Programming languages, environments
 *    and system software interfaces, Extensions to the C Library,
 *    Part I: Bounds-checking interfaces
 *
 * INPUT PARAMETERS
 *    msg       Pointer to the message describing the error
 *
 *    ptr       Pointer to aassociated data.  Can be NULL.
 *
 *    error     The error code encountered.
 *
 * RETURN VALUE
 *    Returns no value.
 *
 * ALSO SEE
 *    abort_handler_s()
 *
 */

void ignore_handler_s(const char *msg, void *ptr, errno_t error) {
    UNUSED(ptr);
    UNUSED(msg);
    UNUSED(error);

    sldebug_printf("IGNORE CONSTRAINT HANDLER: (%u) %s\n", error, (msg) ? msg : "Null message");
}
EXPORT_SYMBOL(ignore_handler_s)
