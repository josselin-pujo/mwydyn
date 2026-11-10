#ifndef _MWJS_TIME_H
#define _MWJS_TIME_H

/* NO CLOCK (§6 clock obliviousness — the ticker is the only
 * wall-clock reader). The freestanding clock is FROZEN AT EPOCH:
 * Date.now() returns 0. This is aligned, not a stub of shame. */

typedef long long time_t;
typedef long clock_t;

/* C11 6.2.1: a tag first declared inside a prototype is
 * PROTOTYPE-scoped — a later file-scope definition would be a
 * second, incompatible type (found the hard way). Forward-declare
 * before any prototype mentions them. */
struct timespec;
struct tm;

time_t time(time_t *t);
clock_t clock(void);
#define CLOCK_MONOTONIC 1
#define CLOCK_REALTIME 0
int clock_gettime(int clk, struct timespec *tp);

struct tm {
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year;
    int tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;
};
struct tm *localtime_r(const time_t *t, struct tm *tm);
struct tm *gmtime_r(const time_t *t, struct tm *tm);
time_t mktime(struct tm *tm);

struct timespec {
    long tv_sec;
    long tv_nsec;
};
#define CLOCKS_PER_SEC 1000000

#endif
