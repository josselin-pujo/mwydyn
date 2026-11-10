/* Freestanding shims for the wasm32-unknown-unknown build (round
 * 43). Compiled ONLY for the wasm target — native builds link the
 * real libc. The heap (malloc family) and libm live in src/alloc.rs
 * (Rust); these are the pieces va_list or byte-level loops want C
 * for. */

#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <ctype.h>
#include <time.h>
#include <sys/time.h>
#include <errno.h>
#include <pthread.h>
#include <stdlib.h>

/* ---- errno ---- */

static int mwjs_errno_value = 0;
int *__mwjs_errno(void) { return &mwjs_errno_value; }

/* ---- string ---- */

void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = dest;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dest;
}

void *memmove(void *dest, const void *src, size_t n) {
    unsigned char *d = dest;
    const unsigned char *s = src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        d += n; s += n;
        while (n--) *--d = *--s;
    }
    return dest;
}

void *memset(void *s, int c, size_t n) {
    unsigned char *p = s;
    while (n--) *p++ = (unsigned char)c;
    return s;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = a, *y = b;
    while (n--) {
        if (*x != *y) return *x - *y;
        x++; y++;
    }
    return 0;
}

size_t strlen(const char *s) {
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

int strcmp(const char *s1, const char *s2) {
    while (*s1 && *s1 == *s2) { s1++; s2++; }
    return (unsigned char)*s1 - (unsigned char)*s2;
}

int strncmp(const char *s1, const char *s2, size_t n) {
    while (n && *s1 && *s1 == *s2) { s1++; s2++; n--; }
    return n ? (unsigned char)*s1 - (unsigned char)*s2 : 0;
}

char *strncpy(char *dest, const char *src, size_t n) {
    char *d = dest;
    while (n && *src) { *d++ = *src++; n--; }
    while (n--) *d++ = '\0';
    return dest;
}

char *strchr(const char *s, int c) {
    for (; *s; s++)
        if (*s == (char)c) return (char *)s;
    return c == '\0' ? (char *)s : NULL;
}

char *strstr(const char *hay, const char *needle) {
    if (!*needle) return (char *)hay;
    for (; *hay; hay++) {
        const char *h = hay, *n = needle;
        while (*h && *n && *h == *n) { h++; n++; }
        if (!*n) return (char *)hay;
    }
    return NULL;
}

/* ---- ctype ---- */

int isalnum(int c) { return isdigit(c) || isalpha(c); }
int isalpha(int c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'); }
int isdigit(int c) { return c >= '0' && c <= '9'; }
int isspace(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int toupper(int c) { return islower(c) ? c - 'a' + 'A' : c; }
int tolower(int c) { return isupper(c) ? c - 'A' + 'a' : c; }

/* ---- the frozen clock (§6: no clock — Date.now() is epoch 0) ---- */

time_t time(time_t *t) { if (t) *t = 0; return 0; }
clock_t clock(void) { return 0; }
int gettimeofday(struct timeval *tv, void *tz) {
    (void)tz;
    if (tv) { tv->tv_sec = 0; tv->tv_usec = 0; }
    return 0;
}

/* ---- the printf subset quickjs actually uses ----
 * Conversions: %c %s %d %i %u %x %X %p %% with flags '-' and '0',
 * width (incl. '*'), precision (incl. '.*'), length modifiers
 * (l/ll/z/h/j/t — ll/z/j/t select 64-bit reads). %f/%g are NOT
 * needed: quickjs formats floats through its own dtoa. An unknown
 * conversion is emitted literally — loud in output, never a crash. */

static void mw_putc(char *str, size_t size, size_t *len, char c) {
    if (*len + 1 < size) str[*len] = c;
    (*len)++;
}

static void mw_puts(char *str, size_t size, size_t *len, const char *s) {
    while (*s) mw_putc(str, size, len, *s++);
}

static void mw_pad(char *str, size_t size, size_t *len, char c, int n) {
    for (int i = 0; i < n; i++) mw_putc(str, size, len, c);
}

int vsnprintf(char *str, size_t size, const char *fmt, va_list ap) {
    size_t len = 0;
    for (const char *p = fmt; *p; p++) {
        if (*p != '%') {
            mw_putc(str, size, &len, *p);
            continue;
        }
        p++;
        int left = 0, zero = 0;
        while (*p == '-' || *p == '0') {
            if (*p == '-') left = 1;
            else zero = 1;
            p++;
        }
        int width = 0;
        if (*p == '*') { width = va_arg(ap, int); p++; }
        else while (*p >= '0' && *p <= '9') { width = width * 10 + (*p - '0'); p++; }
        int precision = -1;
        if (*p == '.') {
            p++;
            precision = 0;
            if (*p == '*') { precision = va_arg(ap, int); p++; }
            else while (*p >= '0' && *p <= '9') { precision = precision * 10 + (*p - '0'); p++; }
        }
        int lcount = 0;
        while (*p == 'l' || *p == 'z' || *p == 'h' || *p == 'j' || *p == 't') {
            if (*p == 'l') lcount++;
            if (*p == 'z' || *p == 'j' || *p == 't') lcount = 2;
            p++;
        }
        int big = lcount >= 2;
        char conv = *p;
        char tmp[32];
        switch (conv) {
        case '%':
            mw_putc(str, size, &len, '%');
            break;
        case 'c': {
            char c = (char)va_arg(ap, int);
            int pad = width > 1 ? width - 1 : 0;
            if (!left) mw_pad(str, size, &len, zero ? '0' : ' ', pad);
            mw_putc(str, size, &len, c);
            if (left) mw_pad(str, size, &len, ' ', pad);
            break;
        }
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            int slen = 0;
            while (s[slen] && (precision < 0 || slen < precision)) slen++;
            int pad = width > slen ? width - slen : 0;
            if (!left) mw_pad(str, size, &len, zero ? '0' : ' ', pad);
            for (int i = 0; i < slen; i++) mw_putc(str, size, &len, s[i]);
            if (left) mw_pad(str, size, &len, ' ', pad);
            break;
        }
        case 'd':
        case 'i': {
            long long v = big ? va_arg(ap, long long) : (long long)va_arg(ap, int);
            unsigned long long mag =
                v < 0 ? (unsigned long long)-(v + 1) + 1 : (unsigned long long)v;
            int n = 0;
            do { tmp[n++] = (char)('0' + (char)(mag % 10)); mag /= 10; } while (mag);
            int digits = n + (v < 0 ? 1 : 0);
            int pad = width > digits ? width - digits : 0;
            if (!left) mw_pad(str, size, &len, zero ? '0' : ' ', pad);
            if (v < 0) mw_putc(str, size, &len, '-');
            while (n--) mw_putc(str, size, &len, tmp[n]);
            if (left) mw_pad(str, size, &len, ' ', pad);
            break;
        }
        case 'u':
        case 'x':
        case 'X': {
            unsigned long long v = big ? va_arg(ap, unsigned long long)
                                       : (unsigned long long)va_arg(ap, unsigned int);
            int base = conv == 'u' ? 10 : 16;
            const char *digits = conv == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
            int n = 0;
            do { tmp[n++] = digits[v % (unsigned)base]; v /= (unsigned)base; } while (v);
            int pad = width > n ? width - n : 0;
            if (!left) mw_pad(str, size, &len, zero ? '0' : ' ', pad);
            while (n--) mw_putc(str, size, &len, tmp[n]);
            if (left) mw_pad(str, size, &len, ' ', pad);
            break;
        }
        case 'p': {
            uintptr_t v = (uintptr_t)va_arg(ap, void *);
            int n = 0;
            do { tmp[n++] = "0123456789abcdef"[v % 16]; v /= 16; } while (v);
            mw_puts(str, size, &len, "0x");
            while (n--) mw_putc(str, size, &len, tmp[n]);
            break;
        }
        default:
            mw_putc(str, size, &len, '%');
            if (conv) mw_putc(str, size, &len, conv);
            break;
        }
    }
    if (size) str[len < size ? len : size - 1] = '\0';
    return (int)len;
}

int snprintf(char *str, size_t size, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(str, size, fmt, ap);
    va_end(ap);
    return r;
}

/* The sink: freestanding guests have no stdout. Format-and-drop
 * keeps the engine's debug paths functional; a telemetry hook is
 * the eventual consumer. */
static char mwjs_sink[512];
int printf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(mwjs_sink, sizeof(mwjs_sink), fmt, ap);
    va_end(ap);
    return r;
}
int fprintf(FILE *f, const char *fmt, ...) {
    (void)f;
    va_list ap;
    va_start(ap, fmt);
    int r = vsnprintf(mwjs_sink, sizeof(mwjs_sink), fmt, ap);
    va_end(ap);
    return r;
}
int vfprintf(FILE *f, const char *fmt, va_list ap) {
    (void)f;
    return vsnprintf(mwjs_sink, sizeof(mwjs_sink), fmt, ap);
}
int fputs(const char *s, FILE *f) { (void)f; (void)s; return 0; }
int fputc(int c, FILE *f) { (void)f; (void)c; return c; }
int fflush(FILE *f) { (void)f; return 0; }
int puts(const char *s) { (void)s; return 0; }
size_t fwrite(const void *p, size_t sz, size_t n, FILE *f) { (void)p; (void)sz; (void)n; (void)f; return n; }
void abort(void) {
    /* trap = strike: the byzantine law (§9.1) — an aborting engine
     * is a crashing guest, supervision's problem, never a swallow */
    __builtin_trap();
}
void exit(int status) { (void)status; __builtin_trap(); }

char *strrchr(const char *s, int c) {
    const char *last = NULL;
    for (; *s; s++)
        if (*s == (char)c) last = s;
    return (char *)last;
}

int putchar(int c) { return c; }

int clock_gettime(int clk, struct timespec *tp) {
    (void)clk;
    if (tp) { tp->tv_sec = 0; tp->tv_nsec = 0; }
    return 0;
}

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }

long lrint(double x) {
    return (long)(x < 0 ? x - 0.5 : x + 0.5);
}

double strtod(const char *nptr, char **endptr) {
    /* minimal decimal parse — quickjs's own js_atod is the real
     * parser; this covers stray libc call sites */
    const char *p = nptr;
    int neg = 0;
    while (*p == ' ' || (*p >= '\011' && *p <= '\015')) p++;
    if (*p == '+' || *p == '-') neg = (*p++ == '-');
    double v = 0;
    int any = 0;
    while (*p >= '0' && *p <= '9') { v = v * 10 + (*p - '0'); p++; any = 1; }
    if (*p == '.') {
        p++;
        double scale = 0.1;
        while (*p >= '0' && *p <= '9') { v += (*p - '0') * scale; scale *= 0.1; p++; any = 1; }
    }
    if (*p == 'e' || *p == 'E') {
        p++;
        int eneg = 0;
        if (*p == '+' || *p == '-') eneg = (*p++ == '-');
        int e = 0, eany = 0;
        while (*p >= '0' && *p <= '9') { e = e * 10 + (*p - '0'); p++; eany = 1; }
        if (eany) {
            double m = 1.0;
            for (int i = 0; i < e; i++) m *= 10.0;
            v = eneg ? v / m : v * m;
        }
    }
    if (endptr) *endptr = (char *)(any ? p : nptr);
    return neg ? -v : v;
}

/* ---- pthread stubs (single-threaded; success is correct) ---- */

int pthread_once(pthread_once_t *guard, void (*cb)(void)) {
    if (*guard == 0) { *guard = 1; cb(); }
    return 0;
}
int pthread_mutex_init(pthread_mutex_t *m, const void *attr) { (void)m; (void)attr; return 0; }
int pthread_mutex_destroy(pthread_mutex_t *m) { (void)m; return 0; }
int pthread_mutex_lock(pthread_mutex_t *m) { (void)m; return 0; }
int pthread_mutex_unlock(pthread_mutex_t *m) { (void)m; return 0; }
int pthread_cond_init(pthread_cond_t *c, const void *attr) { (void)c; (void)attr; return 0; }
int pthread_cond_destroy(pthread_cond_t *c) { (void)c; return 0; }
int pthread_cond_signal(pthread_cond_t *c) { (void)c; return 0; }
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) { (void)c; (void)m; return 0; }
int pthread_condattr_init(pthread_condattr_t *a) { (void)a; return 0; }
int pthread_condattr_destroy(pthread_condattr_t *a) { (void)a; return 0; }
int pthread_condattr_setclock(pthread_condattr_t *a, int clk) { (void)a; (void)clk; return 0; }
pthread_t pthread_self(void) { return 1; }
int pthread_cond_broadcast(pthread_cond_t *c) { (void)c; return 0; }
int pthread_attr_init(pthread_attr_t *a) { (void)a; return 0; }
int pthread_attr_destroy(pthread_attr_t *a) { (void)a; return 0; }
int pthread_attr_setstacksize(pthread_attr_t *a, size_t sz) { (void)a; (void)sz; return 0; }
int pthread_create(pthread_t *t, const pthread_attr_t *a, void *(*fn)(void *), void *arg) {
    (void)t; (void)a; (void)fn; (void)arg;
    abort(); /* threads do not exist on a freestanding guest */
}
int pthread_join(pthread_t t, void **res) { (void)t; (void)res; return 0; }
int pthread_detach(pthread_t t) { (void)t; return 0; }
int pthread_attr_setdetachstate(pthread_attr_t *a, int state) { (void)a; (void)state; return 0; }
void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = s;
    for (; n--; p++)
        if (*p == (unsigned char)c) return (void *)p;
    return NULL;
}
static struct tm mwjs_tm; /* the frozen epoch, decomposed */
struct tm *localtime_r(const time_t *t, struct tm *tm) {
    (void)t;
    if (!tm) tm = &mwjs_tm;
    tm->tm_sec = tm->tm_min = tm->tm_hour = 0;
    tm->tm_mday = 1; tm->tm_mon = 0; tm->tm_year = 70;
    tm->tm_wday = 4; tm->tm_yday = 0; tm->tm_isdst = 0;
    return tm;
}
struct tm *gmtime_r(const time_t *t, struct tm *tm) { return localtime_r(t, tm); }
time_t mktime(struct tm *tm) { (void)tm; return 0; }
int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *t) {
    (void)c; (void)m; (void)t; return 0;
}
