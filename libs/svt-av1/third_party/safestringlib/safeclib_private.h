/*  SPDX-License-Identifier: MIT */
/*
 *  Copyright (c) 2012 Jonathan Toppins <jtoppins@users.sourceforge.net>
 *  Copyright (c) 2012-2013 Cisco Systems, Inc
 *  Copyright (c) 2022 Intel Corp
 */

#ifndef __SAFECLIB_PRIVATE_H__
#define __SAFECLIB_PRIVATE_H__

#ifdef __KERNEL__
/* linux kernel environment */

#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/ctype.h>

#define RCNEGATE(x) (-(x))

#define slprintf(...) printk(KERN_EMERG __VA_ARGS__)
#define slabort()
#ifdef DEBUG
#define sldebug_printf(...) printk(KERN_DEBUG __VA_ARGS__)
#endif

#else /* !__KERNEL__ */

#if HAVE_CONFIG_H
#include "config.h"
#endif

#include <stdio.h>
#ifdef STDC_HEADERS
#include <ctype.h>
#include <stdlib.h>
#include <stddef.h>
#else
#ifdef HAVE_STDLIB_H
#include <stdlib.h>
#endif
#endif
#ifdef HAVE_STRING_H
#if !defined STDC_HEADERS && defined HAVE_MEMORY_H
#include <memory.h>
#endif
#include <string.h>
#endif
#ifdef HAVE_LIMITS_H
#include <limits.h>
#endif

#define EXPORT_SYMBOL(sym)
#define RCNEGATE(x) (x)

#define slprintf(...) fprintf(stderr, __VA_ARGS__)
#define slabort() abort()
#ifdef DEBUG
#define sldebug_printf(...) printf(__VA_ARGS__)
#endif

#endif /* __KERNEL__ */

#define UNUSED(x) (void)(x)

#ifndef sldebug_printf
#define sldebug_printf(...)
#endif

#include "safe_lib.h"

#endif /* __SAFECLIB_PRIVATE_H__ */
