#ifndef _MWJS_SYS_TIME_H
#define _MWJS_SYS_TIME_H

/* Frozen at epoch — see time.h. */

struct timeval {
    long tv_sec;
    long tv_usec;
};

struct timezone;
int gettimeofday(struct timeval *tv, void *tz);

#endif
