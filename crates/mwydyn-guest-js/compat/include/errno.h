#ifndef _MWJS_ERRNO_H
#define _MWJS_ERRNO_H

int *__mwjs_errno(void);
#define errno (*__mwjs_errno())

#define EINVAL 22
#define ENOMEM 12
#define ERANGE 34
#define EDOM 33
#define ETIMEDOUT 138
#define EPERM 1
#define EINTR 4
#define EAGAIN 11
#define ENOTDIR 20
#define EISDIR 21

#endif
