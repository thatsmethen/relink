#ifndef RAVELINK_CLOCK_H
#define RAVELINK_CLOCK_H
#include <CoreAudio/CoreAudio.h>
#include <stdint.h>
#include <stdbool.h>

// Allocate/reset only while its callback is stopped. One callback owns every
// observation; the discontinuity counter is atomic for control-thread telemetry.
typedef struct RLClock RLClock;
RLClock *rl_clock_create(double nominalSampleRate, double hostTicksPerSecond);
void rl_clock_destroy(RLClock *clock);
void rl_clock_reset(RLClock *clock);
// Zero means no trustworthy estimate yet. Missing flags reset the observation
// window; invalid/retrograde device timelines increment a latched failure count.
// Relaxed continuity supports process taps that legitimately pause while idle.
float rl_clock_observe(RLClock *clock, const AudioTimeStamp *timestamp,
                       uint32_t frames, bool relaxedContinuity);
uint64_t rl_clock_discontinuities(RLClock *clock);
float rl_clock_factor(RLClock *clock);
#endif
