#ifndef _MWJS_STDIO_H
#define _MWJS_STDIO_H

#include <stddef.h>
#include <stdarg.h>

/* There is no stdout on a freestanding guest: the printf family
 * formats into buffers (snprintf — used by real engine paths like
 * JSON \u escaping) or lands in the sink (a no-op v0; a telemetry
 * hook is the eventual consumer). FILE is an opaque dummy. */
typedef struct _mwjs_file FILE;
#define stdout ((FILE *)0)
#define stderr ((FILE *)1)
#define EOF (-1)

int printf(const char *fmt, ...);
int fprintf(FILE *f, const char *fmt, ...);
int snprintf(char *str, size_t size, const char *fmt, ...);
int vsnprintf(char *str, size_t size, const char *fmt, va_list ap);
int fputs(const char *s, FILE *f);
int fputc(int c, FILE *f);
int fflush(FILE *f);
int putchar(int c);
int vfprintf(FILE *f, const char *fmt, va_list ap);

#endif
