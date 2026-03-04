/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2007 Bo Berry
 *  Copyright (c) 2012 Jonathan Toppins <jtoppins@users.sourceforge.net>
 *  Copyright (c) 2007-2013 by Cisco Systems, Inc
 */

#ifndef __SAFE_TYPES_H__
#define __SAFE_TYPES_H__

#ifdef __KERNEL__
/* linux kernel environment */

#include <linux/stddef.h>
#include <linux/types.h>
#include <linux/errno.h>

/* errno_t isn't defined in the kernel */
typedef int errno_t;

#else

#include <stdio.h>

/* For systems without sys/types.h, prefer to get size_t from stdlib.h */
/* some armcc environments don't have a sys/types.h in the environment */
#ifdef _USE_STDLIB
#include <stdlib.h>
#include <ctype.h>
#else
#include <sys/types.h>
#endif

#include <inttypes.h>
#include <stdint.h>
#include <errno.h>

typedef int errno_t;

#include <stdbool.h>

#endif /* __KERNEL__ */
#endif /* __SAFE_TYPES_H__ */
