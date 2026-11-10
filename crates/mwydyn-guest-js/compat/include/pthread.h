/* Single-threaded wasm guest: pthread as no-op types and
 * success-returning stubs (declared here; implemented in
 * compat/shims.c). The engine's thread helpers are inline-unused
 * in the core; if any path links, no-op success is correct. */
#ifndef _MWJS_PTHREAD_H
#define _MWJS_PTHREAD_H

#include <time.h> /* struct timespec for the cond prototypes */

typedef int pthread_once_t;
typedef int pthread_mutex_t;
typedef int pthread_cond_t;
typedef unsigned long pthread_t;
typedef int pthread_condattr_t;
typedef int pthread_attr_t;

#define PTHREAD_ONCE_INIT 0
#define PTHREAD_MUTEX_INITIALIZER 0

int pthread_once(pthread_once_t *guard, void (*cb)(void));
int pthread_mutex_init(pthread_mutex_t *m, const void *attr);
int pthread_mutex_destroy(pthread_mutex_t *m);
int pthread_mutex_lock(pthread_mutex_t *m);
int pthread_mutex_unlock(pthread_mutex_t *m);
int pthread_cond_init(pthread_cond_t *c, const void *attr);
int pthread_cond_destroy(pthread_cond_t *c);
int pthread_cond_signal(pthread_cond_t *c);
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m);
int pthread_cond_broadcast(pthread_cond_t *c);
int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *t);
int pthread_condattr_init(pthread_condattr_t *a);
int pthread_condattr_destroy(pthread_condattr_t *a);
int pthread_condattr_setclock(pthread_condattr_t *a, int clk);
pthread_t pthread_self(void);
int pthread_attr_init(pthread_attr_t *a);
int pthread_attr_destroy(pthread_attr_t *a);
int pthread_attr_setstacksize(pthread_attr_t *a, size_t sz);
int pthread_create(pthread_t *t, const pthread_attr_t *a,
                   void *(*fn)(void *), void *arg);
int pthread_join(pthread_t t, void **res);
int pthread_detach(pthread_t t);
int pthread_attr_setdetachstate(pthread_attr_t *a, int state);
#define PTHREAD_CREATE_DETACHED 1
#define PTHREAD_CREATE_JOINABLE 0

#endif
