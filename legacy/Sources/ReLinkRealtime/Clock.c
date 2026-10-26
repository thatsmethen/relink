#include "Clock.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <math.h>
_Static_assert(ATOMIC_LLONG_LOCK_FREE == 2, "Clock telemetry requires lock-free atomics");

struct RLClock {
    double nominalRate, hostFrequency, anchorSample, lastSample, factor;
    uint64_t anchorHost, lastHost;
    uint32_t lastFrames;
    bool anchored, valid;
    _Atomic uint64_t discontinuities;
    _Atomic float publishedFactor;
};
RLClock *rl_clock_create(double rate, double frequency) {
    if (!isfinite(rate) || rate < 8000 || rate > 384000 || !isfinite(frequency) || frequency <= 0) return NULL;
    RLClock *c = calloc(1, sizeof(*c));
    if (c && !atomic_is_lock_free(&c->publishedFactor)) { free(c); return NULL; }
    if (c) { c->nominalRate = rate; c->hostFrequency = frequency; c->factor = 1; }
    return c;
}
void rl_clock_destroy(RLClock *c) { free(c); }
void rl_clock_reset(RLClock *c) {
    if (!c) return;
    c->anchored = false; c->valid = false; c->factor = 1;
    atomic_store(&c->discontinuities, 0);
    atomic_store(&c->publishedFactor, 0);
}
static float invalid(RLClock *c, bool failure) {
    c->anchored = false; c->valid = false;
    atomic_store_explicit(&c->publishedFactor, 0, memory_order_relaxed);
    if (failure) atomic_fetch_add_explicit(&c->discontinuities, 1, memory_order_relaxed);
    return 0;
}
float rl_clock_observe(RLClock *c, const AudioTimeStamp *t, uint32_t frames, bool relaxed) {
    if (!c || atomic_load_explicit(&c->discontinuities, memory_order_relaxed)) return 0;
    if (!t || !frames || (t->mFlags & (kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid)) !=
        (kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid)) return invalid(c, false);
    if (!isfinite(t->mSampleTime)) return invalid(c, true);
    if (c->anchored) {
        double advance = t->mSampleTime - c->lastSample;
        if (t->mHostTime <= c->lastHost || advance <= 0 || fabs(advance - c->lastFrames) > .01) {
            if (!relaxed) return invalid(c, true);
            c->anchored = false; c->valid = false;
        }
    }
    if (!c->anchored) {
        c->anchorSample = t->mSampleTime; c->anchorHost = t->mHostTime;
        c->lastSample = t->mSampleTime; c->lastHost = t->mHostTime;
        c->lastFrames = frames; c->anchored = true;
        atomic_store_explicit(&c->publishedFactor, 0, memory_order_relaxed); return 0;
    }
    c->lastSample = t->mSampleTime; c->lastHost = t->mHostTime; c->lastFrames = frames;
    double elapsed = (double)(t->mHostTime - c->anchorHost) / c->hostFrequency;
    if (elapsed > 10) return invalid(c, !relaxed);
    if (elapsed >= .25) {
        double measured = (t->mSampleTime - c->anchorSample) / elapsed / c->nominalRate;
        if (!isfinite(measured) || measured < .995 || measured > 1.005) return invalid(c, !relaxed);
        // Hardware timestamps, not callback arrival time. Bound later changes to
        // 1000 ppm per second; the renderer additionally slews the total ratio.
        if (!c->valid) c->factor = measured;
        else {
            double limit = .001 * elapsed;
            c->factor += fmax(-limit, fmin(limit, measured - c->factor));
        }
        c->valid = true; c->anchorHost = t->mHostTime; c->anchorSample = t->mSampleTime;
    }
    float result = c->valid ? (float)c->factor : 0;
    atomic_store_explicit(&c->publishedFactor, result, memory_order_relaxed);
    return result;
}
uint64_t rl_clock_discontinuities(RLClock *c) {
    return c ? atomic_load_explicit(&c->discontinuities, memory_order_relaxed) : 0;
}
float rl_clock_factor(RLClock *c) { return c ? atomic_load_explicit(&c->publishedFactor, memory_order_relaxed) : 0; }
