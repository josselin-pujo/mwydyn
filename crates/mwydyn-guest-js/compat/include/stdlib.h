/* Freestanding libc surface for wasm32-unknown-unknown (round 43).
 * Declarations only — implementations live in compat/shims.c (C) and
 * src/alloc.rs (Rust, the unified heap). */

#ifndef _MWJS_STDLIB_H
#define _MWJS_STDLIB_H

#include <stddef.h>

void *malloc(size_t size);
void *calloc(size_t nmemb, size_t size);
void *realloc(void *ptr, size_t size);
void free(void *ptr);
void abort(void);
void exit(int status);
int atoi(const char *nptr);
int abs(int x);
long labs(long x);
long strtol(const char *nptr, char **endptr, int base);
unsigned long strtoul(const char *nptr, char **endptr, int base);
double strtod(const char *nptr, char **endptr);

#ifndef alloca
#define alloca(n) __builtin_alloca(n)
#endif

#define EXIT_FAILURE 1
#define EXIT_SUCCESS 0

#endif
