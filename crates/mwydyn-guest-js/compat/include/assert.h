#ifndef _MWJS_ASSERT_H
#define _MWJS_ASSERT_H

/* assert failure = trap = strike (the byzantine law, §9.1). */
#ifndef NDEBUG
#define assert(x) ((void)((x) || (__builtin_trap(), 0)))
#else
#define assert(x) ((void)0)
#endif

#endif
