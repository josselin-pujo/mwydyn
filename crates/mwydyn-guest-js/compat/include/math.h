#ifndef _MWJS_MATH_H
#define _MWJS_MATH_H

/* Implementations in src/alloc.rs (Rust f64 methods — exact). */
double floor(double x);
double ceil(double x);
double round(double x);
double trunc(double x);
double log2(double x);
double fmod(double x, double y);
double fabs(double x);
double sqrt(double x);
double pow(double x, double y);
double modf(double x, double *iptr);
double ldexp(double x, int exp);
double scalbn(double x, int exp);
long lrint(double x);
double hypot(double x, double y);
double acos(double x);
double asin(double x);
double atan(double x);
double atan2(double y, double x);
double cos(double x);
double sin(double x);
double tan(double x);
double cosh(double x);
double sinh(double x);
double tanh(double x);
double exp(double x);
double log(double x);
double log10(double x);
double log1p(double x);
double expm1(double x);
double cbrt(double x);
double acosh(double x);
double asinh(double x);
double atanh(double x);
double copysign(double x, double y);
double frexp(double x, int *exp);

#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x) __builtin_signbit(x)

#ifndef HUGE_VAL
#define HUGE_VAL (__builtin_huge_val())
#endif
#ifndef INFINITY
#define INFINITY (__builtin_inff())
#endif
#ifndef NAN
#define NAN (__builtin_nanf(""))
#endif

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif
#ifndef M_PI_2
#define M_PI_2 1.57079632679489661923
#endif

#endif
