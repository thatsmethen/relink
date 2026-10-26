#include "RaveLinkRealtime.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <limits.h>
#include <CoreAudio/HostTime.h>

_Static_assert(ATOMIC_INT_LOCK_FREE == 2, "Realtime controls require lock-free atomics");
_Static_assert(ATOMIC_LLONG_LOCK_FREE == 2, "Realtime counters require lock-free atomics");
// Atomic float lock freedom is checked when the first graph object is allocated.
static bool atomics_supported(void) { _Atomic float f = 0; return atomic_is_lock_free(&f); }
uint32_t rl_maximum_callback_frames(double rate) {
    if (!isfinite(rate) || rate < 8000 || rate > 384000) return 0;
    return 16384;
}
// Prepare writable pages on the control thread. calloc alone can defer backing
// page allocation until the first render/capture write. This is not an mlock or
// a promise of residency under later memory pressure.
static float *audio_samples_create(size_t samples) {
    if (samples > SIZE_MAX / sizeof(float)) return NULL;
    float *buffer = calloc(samples, sizeof(float));
    if (buffer && samples) {
        volatile float *pages = buffer;
        // A 4 KiB stride covers macOS 4 KiB and 16 KiB memory pages.
        for (size_t index = 0; index < samples; index += 1024) pages[index] = 0;
        pages[samples - 1] = 0;
    }
    return buffer;
}
static float db_gain(float db) { return powf(10.f, fmaxf(-120.f, fminf(db, 36.f)) / 20.f); }
static float finite_sample(float x) { return isfinite(x) ? x : 0.f; }
static void peak_store(_Atomic float *meter, float value) {
    float old = atomic_load_explicit(meter, memory_order_relaxed);
    while (value > old && !atomic_compare_exchange_weak_explicit(meter, &old, value,
            memory_order_relaxed, memory_order_relaxed)) {}
}
struct RLRing {
    float *data;
    float *kernel;
    uint32_t capacity, mask;
    _Atomic uint64_t write, read, dropped, underruns;
    _Atomic uint32_t producerBlockFrames;
    _Atomic float sourceClockFactor;
    _Atomic bool resetRequested;
    _Atomic bool clockMismatch;
    double fraction;
    bool primed;
    double correction;
    double configuredRatio;
    double outputSampleRate, consumerBlockFrames, targetFill;
    bool tracksProducerBlocks;
    uint32_t taps;
};
#define RL_PHASES 512
bool rl_ring_prepare_ratio(RLRing *r, double ratio) {
    if (!r || !isfinite(ratio) || ratio < 1.0 / 24 || ratio > 24) return false;
    uint32_t taps = 64 * (uint32_t)ceil(fmax(1, ratio));
    float *kernel = calloc((size_t)taps * RL_PHASES, sizeof(float));
    if (!kernel) return false;
    free(r->kernel); r->kernel = kernel; r->taps = taps;
    double cutoff = fmin(1, 1 / ratio) * .90;
    for (uint32_t phase = 0; phase < RL_PHASES; phase++) {
        double fraction = (double)phase / RL_PHASES, sum = 0;
        for (uint32_t t = 0; t < taps; t++) {
            double x = (double)t - (taps / 2 - 1) - fraction;
            double sinc = fabs(x) < 1e-9 ? cutoff : sin(M_PI * cutoff * x) / (M_PI * x);
            double window = .42 - .5 * cos(2 * M_PI * t / (taps - 1)) + .08 * cos(4 * M_PI * t / (taps - 1));
            r->kernel[phase * taps + t] = (float)(sinc * window); sum += sinc * window;
        }
        for (uint32_t t = 0; t < taps; t++) r->kernel[phase * taps + t] /= (float)sum;
    }
    r->configuredRatio = ratio; r->outputSampleRate = 48000; r->tracksProducerBlocks = false;
    atomic_store(&r->sourceClockFactor, 0);
    atomic_store(&r->clockMismatch, false);
    r->consumerBlockFrames = 0; r->targetFill = 0; atomic_store(&r->producerBlockFrames, 0);
    r->correction = 0; r->fraction = 0; r->primed = false;
    return true;
}
bool rl_ring_prepare_clock(RLRing *r, double inputRate, double outputRate) {
    if (!r || !isfinite(inputRate) || !isfinite(outputRate) || inputRate < 8000 || outputRate < 8000 ||
        inputRate > 384000 || outputRate > 384000 || !rl_ring_prepare_ratio(r, inputRate / outputRate)) return false;
    r->outputSampleRate = outputRate; r->tracksProducerBlocks = true;
    r->consumerBlockFrames = 0; atomic_store(&r->producerBlockFrames, 0);
    r->correction = 0; r->fraction = 0; r->primed = false;
    return true;
}
bool rl_ring_prepare_buffers(RLRing *r, uint32_t inputFrames, uint32_t outputFrames) {
    if (!r || !r->tracksProducerBlocks || !inputFrames || !outputFrames ||
        inputFrames > rl_maximum_callback_frames(r->outputSampleRate * r->configuredRatio) ||
        outputFrames > rl_maximum_callback_frames(r->outputSampleRate)) return false;
    double headroom = fmax(inputFrames, outputFrames * r->configuredRatio);
    if (4 * headroom + r->taps >= r->capacity) return false;
    atomic_store(&r->producerBlockFrames, inputFrames);
    r->consumerBlockFrames = outputFrames * r->configuredRatio; r->targetFill = 0;
    return true;
}
RLRing *rl_ring_create(uint32_t capacity) {
    if (!atomics_supported() || capacity < 4 || capacity > (1u << 25)) return NULL;
    uint32_t size = 4; while (size < capacity) size <<= 1;
    RLRing *r = calloc(1, sizeof(*r)); if (!r) return NULL;
    r->outputSampleRate = 48000;
    r->data = audio_samples_create((size_t)size * 2);
    if (!r->data) { free(r); return NULL; }
    r->capacity = size; r->mask = size - 1;
    if (!rl_ring_prepare_ratio(r, 1)) { rl_ring_destroy(r); return NULL; }
    return r;
}
void rl_ring_destroy(RLRing *r) { if (r) { free(r->data); free(r->kernel); free(r); } }
uint32_t rl_ring_write(RLRing *r, const float *stereo, uint32_t frames) {
    if (!r || !stereo) return 0;
    // One writer owns this maximum. A stable target handles unequal callback
    // sizes without chasing each individual producer burst or consumer block.
    if (r->tracksProducerBlocks && frames > atomic_load_explicit(&r->producerBlockFrames, memory_order_relaxed))
        atomic_store_explicit(&r->producerBlockFrames, frames, memory_order_relaxed);
    uint64_t w = atomic_load_explicit(&r->write, memory_order_relaxed);
    uint64_t read = atomic_load_explicit(&r->read, memory_order_acquire);
    uint32_t count = (uint32_t)fmin((double)frames, (double)(r->capacity - (w - read)));
    for (uint32_t i = 0; i < count; i++) {
        uint32_t at = (uint32_t)(w + i) & r->mask;
        r->data[at * 2] = finite_sample(stereo[i * 2]);
        r->data[at * 2 + 1] = finite_sample(stereo[i * 2 + 1]);
    }
    atomic_store_explicit(&r->write, w + count, memory_order_release);
    atomic_fetch_add_explicit(&r->dropped, frames - count, memory_order_relaxed);
    return count;
}
void rl_ring_set_source_clock(RLRing *r, float factor) {
    if (r) atomic_store_explicit(&r->sourceClockFactor,
        isfinite(factor) && factor >= .995f && factor <= 1.005f ? factor : 0, memory_order_relaxed);
}
uint32_t rl_ring_read(RLRing *r, float *stereo, uint32_t frames, double ratio) {
    return rl_ring_read_clocked(r, stereo, frames, ratio, 0);
}
uint32_t rl_ring_read_clocked(RLRing *r, float *stereo, uint32_t frames, double ratio, float outputFactor) {
    if (!r || !stereo || !frames) return 0;
    if (atomic_load_explicit(&r->clockMismatch, memory_order_relaxed)) {
        memset(stereo, 0, (size_t)frames * 2 * sizeof(float)); return 0;
    }
    if (atomic_exchange_explicit(&r->resetRequested, false, memory_order_acquire)) {
        atomic_store_explicit(&r->read, atomic_load_explicit(&r->write, memory_order_acquire), memory_order_release);
        r->fraction = 0; r->correction = 0; r->targetFill = 0; r->primed = false;
    }
    if (!isfinite(ratio) || ratio < 1.0 / 24 || ratio > 24 || fabs(ratio - r->configuredRatio) > .000001) {
        memset(stereo, 0, (size_t)frames * 2 * sizeof(float)); return 0;
    }
    uint64_t rd = atomic_load_explicit(&r->read, memory_order_relaxed);
    uint64_t w = atomic_load_explicit(&r->write, memory_order_acquire);
    // Reserve two observed callback periods, plus sinc lookahead. Maxima only
    // increase until graph reconstruction, so variable block sizes do not move
    // the target backwards and forwards on every callback.
    r->consumerBlockFrames = fmax(r->consumerBlockFrames, frames * ratio);
    double headroom = fmax(16, fmax(r->consumerBlockFrames,
        atomic_load_explicit(&r->producerBlockFrames, memory_order_relaxed)));
    double target = fmin(r->capacity - 1, headroom * 2 + r->taps);
    if (!r->primed) {
        if ((double)(w - rd) < target) { memset(stereo, 0, frames * 2 * sizeof(float)); return 0; }
        // Capture arrives in whole blocks. Retain the rounded-up priming reserve
        // instead of speeding up playback simply to discard that initial excess.
        r->targetFill = r->tracksProducerBlocks ? fmax(target, fmin((double)(w - rd), r->capacity - headroom)) : target;
        r->primed = true;
    }
    r->targetFill = fmax(r->targetFill, target);
    target = r->targetFill;
    // Regulate delay error in seconds, not fractions of a callback: proportional
    // convergence must not become slower when devices use larger buffers. Both
    // signs reach the +/-3000 ppm limit before the FIFO reaches an edge.
    // Smooth in seconds, not callbacks, including long/unequal device periods.
    float sourceFactor = atomic_load_explicit(&r->sourceClockFactor, memory_order_relaxed);
    bool timed = sourceFactor > 0 && isfinite(outputFactor) && outputFactor >= .995f && outputFactor <= 1.005f;
    double clockCorrection = timed ? (double)sourceFactor / outputFactor - 1 : 0;
    if (fabs(clockCorrection) > .0030001) {
        atomic_store_explicit(&r->clockMismatch, true, memory_order_relaxed);
        memset(stereo, 0, (size_t)frames * 2 * sizeof(float)); return 0;
    }
    double errorFrames = (double)(w - rd) - target;
    if (timed) {
        // Whole capture packets produce a sawtooth even when the hardware rates
        // are known exactly. Do not cancel feed-forward to chase that packet
        // phase. Feedback remains active outside a guarded one-period reserve.
        double band = fmax(0, fmin(headroom, target - r->taps - frames * ratio * 1.004 - 16));
        errorFrames = copysign(fmax(0, fabs(errorFrames) - band), errorFrames);
    }
    double errorSeconds = errorFrames / (r->outputSampleRate * ratio);
    double timeConstant = fmax(.25, 4 * headroom / (r->outputSampleRate * ratio));
    double alpha = -expm1(-(double)frames / r->outputSampleRate / timeConstant);
    double desired = fmax(-.003, fmin(.003, clockCorrection + errorSeconds));
    r->correction += (desired - r->correction) * alpha;
    double step = ratio * (1.0 + r->correction);
    uint32_t produced = 0;
    for (; produced < frames; produced++) {
        uint64_t whole = (uint64_t)r->fraction;
        if (rd + whole + r->taps >= w) break;
        float fraction = (float)(r->fraction - whole);
        uint32_t phase = (uint32_t)(fraction * RL_PHASES); if (phase >= RL_PHASES) phase = RL_PHASES - 1;
        float *kernel = r->kernel + phase * r->taps;
        for (uint32_t c = 0; c < 2; c++) {
            double value = 0;
            for (uint32_t tap = 0; tap < r->taps; tap++) value += r->data[(((uint32_t)(rd + whole) + tap) & r->mask) * 2 + c] * kernel[tap];
            stereo[produced * 2 + c] = (float)value;
        }
        r->fraction += step;
        uint64_t advance = (uint64_t)r->fraction;
        // Never publish a read cursor beyond the write cursor.
        if (advance > w - rd) advance = w - rd;
        rd += advance; r->fraction -= advance;
    }
    atomic_store_explicit(&r->read, rd, memory_order_release);
    if (produced < frames) {
        memset(stereo + produced * 2, 0, (frames - produced) * 2 * sizeof(float));
        atomic_fetch_add_explicit(&r->underruns, frames - produced, memory_order_relaxed);
        r->primed = false; r->fraction = 0;
    }
    return produced;
}
uint64_t rl_ring_dropped(RLRing *r) { return r ? atomic_load(&r->dropped) : 0; }
uint64_t rl_ring_underruns(RLRing *r) { return r ? atomic_load(&r->underruns) : 0; }
bool rl_ring_clock_mismatch(RLRing *r) { return r && atomic_load_explicit(&r->clockMismatch, memory_order_relaxed); }
void rl_ring_request_reset(RLRing *r) { if (r) atomic_store_explicit(&r->resetRequested, true, memory_order_release); }

struct RLDSP {
    double sampleRate;
    _Atomic float gain, highpass, gate, threshold, ratio;
    float previousInput[2], previousOutput[2], envelope, smoothedGain, inputGain;
};
RLDSP *rl_dsp_create(double sampleRate) {
    if (!atomics_supported() || sampleRate < 8000 || !isfinite(sampleRate)) return NULL;
    RLDSP *d = calloc(1, sizeof(*d)); if (!d) return NULL;
    d->sampleRate = sampleRate; atomic_store(&d->gain, 1); atomic_store(&d->ratio, 1);
    atomic_store(&d->threshold, 1); atomic_store(&d->gate, 0); d->smoothedGain = 1; d->inputGain = 1;
    return d;
}
void rl_dsp_destroy(RLDSP *d) { free(d); }
void rl_dsp_controls(RLDSP *d, float gainDB, float highPassHz, float gateDB, float compressorDB, float ratio) {
    if (!d || !isfinite(gainDB) || !isfinite(highPassHz) || !isfinite(gateDB) ||
        !isfinite(compressorDB) || !isfinite(ratio)) return;
    atomic_store(&d->gain, db_gain(gainDB)); atomic_store(&d->highpass, fmaxf(0, fminf(highPassHz, 20000)));
    atomic_store(&d->gate, gateDB <= -80 ? 0 : db_gain(gateDB));
    atomic_store(&d->threshold, db_gain(compressorDB)); atomic_store(&d->ratio, fmaxf(1, fminf(ratio, 20)));
}
void rl_dsp_process(RLDSP *d, float *stereo, uint32_t frames) {
    if (!d || !stereo) return;
    float gain = atomic_load(&d->gain), hp = atomic_load(&d->highpass), gate = atomic_load(&d->gate);
    float threshold = atomic_load(&d->threshold), ratio = atomic_load(&d->ratio);
    float alpha = hp > 0 ? (float)(1 / (1 + 2 * M_PI * hp / d->sampleRate)) : 0;
    float attack = (float)exp(-1 / (.005 * d->sampleRate)), release = (float)exp(-1 / (.100 * d->sampleRate));
    float smooth = (float)exp(-1 / (.005 * d->sampleRate));
    if (!isfinite(d->inputGain)) d->inputGain = 0;
    if (!isfinite(d->envelope)) d->envelope = 0;
    if (!isfinite(d->smoothedGain)) d->smoothedGain = 1;
    for (uint32_t i = 0; i < frames; i++) {
        d->inputGain = smooth * d->inputGain + (1 - smooth) * gain;
        float pair[2];
        for (int c = 0; c < 2; c++) {
            // Reject corrupt Float32 magnitudes before multiplication can overflow
            // and poison the filter/envelope across all later valid audio. The
            // bound matches the AU bridge's output validity envelope (+120 dBFS).
            float raw = stereo[i * 2 + c];
            float x = (isfinite(raw) && fabsf(raw) <= 1000000.f ? raw : 0.f) * d->inputGain;
            float y = hp > 0 ? alpha * (d->previousOutput[c] + x - d->previousInput[c]) : x;
            if (!isfinite(y)) { y = 0; d->previousInput[c] = 0; }
            else d->previousInput[c] = x;
            d->previousOutput[c] = y; pair[c] = y;
        }
        float level = fmaxf(fabsf(pair[0]), fabsf(pair[1]));
        float coefficient = level > d->envelope ? attack : release;
        d->envelope = coefficient * d->envelope + (1 - coefficient) * level;
        float target = 1;
        if (gate > 0 && d->envelope < gate) target = 0;
        else if (d->envelope > threshold && ratio > 1)
            target = powf(d->envelope / threshold, 1 / ratio - 1);
        d->smoothedGain = smooth * d->smoothedGain + (1 - smooth) * target;
        stereo[i * 2] = pair[0] * d->smoothedGain;
        stereo[i * 2 + 1] = pair[1] * d->smoothedGain;
    }
}

// Buffers are Float32, validated against stream virtual formats before IO starts.
// A channel index spans AudioBufferList buffers, supporting planar and interleaved devices.
static float input_sample(const AudioBufferList *list, uint32_t channel, uint32_t frame) {
    if (!list) return 0;
    for (uint32_t b = 0; b < list->mNumberBuffers; b++) {
        const AudioBuffer *buf = &list->mBuffers[b];
        if (channel < buf->mNumberChannels) {
            size_t at = (size_t)frame * buf->mNumberChannels + channel;
            return buf->mData && (at + 1) * sizeof(float) <= buf->mDataByteSize ? ((float *)buf->mData)[at] : 0;
        }
        channel -= buf->mNumberChannels;
    }
    return 0;
}
static uint32_t list_channels(const AudioBufferList *list) {
    uint32_t n = 0; if (list) for (uint32_t b = 0; b < list->mNumberBuffers; b++) n += list->mBuffers[b].mNumberChannels; return n;
}
static uint32_t list_frames(const AudioBufferList *list) {
    uint32_t n = UINT_MAX;
    if (list) for (uint32_t b = 0; b < list->mNumberBuffers; b++) {
        const AudioBuffer *buf = &list->mBuffers[b];
        if (buf->mNumberChannels && buf->mData) {
            uint32_t count = buf->mDataByteSize / sizeof(float) / buf->mNumberChannels;
            if (count < n) n = count;
        }
    }
    return n == UINT_MAX ? 0 : n;
}
static void add_sample(AudioBufferList *list, uint32_t channel, uint32_t frame, float sample) {
    if (!list) return;
    for (uint32_t b = 0; b < list->mNumberBuffers; b++) {
        AudioBuffer *buf = &list->mBuffers[b];
        if (channel < buf->mNumberChannels) {
            size_t at = (size_t)frame * buf->mNumberChannels + channel;
            if (buf->mData && (at + 1) * sizeof(float) <= buf->mDataByteSize) ((float *)buf->mData)[at] += sample;
            return;
        }
        channel -= buf->mNumberChannels;
    }
}
struct RLCapture {
    RLDSP *dsp;
    RLClock *clock;
    bool relaxedTiming;
    uint32_t channelOffset, maxFrames, destinationCount;
    RLRing **destinations;
    float *scratch;
    _Atomic float peak;
    _Atomic uint64_t frames, oversized;
    uint32_t callbackLimit;
};
RLCapture *rl_capture_create(double rate, uint32_t offset, uint32_t maxFrames, uint32_t destinations) {
    if (!maxFrames || maxFrames > (1u << 20) || !rl_maximum_callback_frames(rate)) return NULL;
    RLCapture *c = calloc(1, sizeof(*c)); if (!c) return NULL;
    c->dsp = rl_dsp_create(rate); c->scratch = audio_samples_create((size_t)maxFrames * 2);
    c->clock = rl_clock_create(rate, AudioGetHostClockFrequency());
    c->destinations = calloc(destinations ? destinations : 1, sizeof(RLRing *));
    if (!c->dsp || !c->clock || !c->scratch || !c->destinations) { rl_capture_destroy(c); return NULL; }
    c->channelOffset = offset; c->maxFrames = maxFrames; c->destinationCount = destinations;
    c->callbackLimit = rl_maximum_callback_frames(rate); return c;
}
void rl_capture_destroy(RLCapture *c) { if (c) { rl_dsp_destroy(c->dsp); rl_clock_destroy(c->clock); free(c->scratch); free(c->destinations); free(c); } }
void rl_capture_set_destination(RLCapture *c, uint32_t i, RLRing *ring) { if (c && i < c->destinationCount) c->destinations[i] = ring; }
void rl_capture_controls(RLCapture *c, float gain, float hp, float gate, float threshold, float ratio) { if (c) rl_dsp_controls(c->dsp, gain, hp, gate, threshold, ratio); }
float rl_capture_take_peak(RLCapture *c) { return c ? atomic_exchange(&c->peak, 0) : 0; }
uint64_t rl_capture_frames(RLCapture *c) { return c ? atomic_load(&c->frames) : 0; }
uint64_t rl_capture_oversized_blocks(RLCapture *c) { return c ? atomic_load(&c->oversized) : 0; }
uint64_t rl_capture_timestamp_discontinuities(RLCapture *c) { return c ? rl_clock_discontinuities(c->clock) : 0; }
float rl_capture_clock_factor(RLCapture *c) { return c ? rl_clock_factor(c->clock) : 0; }
void rl_capture_allow_idle_pauses(RLCapture *c, bool allowed) { if (c) c->relaxedTiming = allowed; }
void rl_capture_reset_timing(RLCapture *c) {
    if (!c) return;
    rl_clock_reset(c->clock);
    for (uint32_t d = 0; d < c->destinationCount; d++) rl_ring_set_source_clock(c->destinations[d], 0);
}
OSStatus rl_capture_callback(AudioObjectID device, const AudioTimeStamp *now, const AudioBufferList *input,
        const AudioTimeStamp *inputTime, AudioBufferList *output, const AudioTimeStamp *outputTime, void *context) {
    // A capture IOProc on a duplex device still owns an output contribution.
    // Silence it explicitly; CoreAudio may mix it with another render IOProc.
    if (output) for (uint32_t b = 0; b < output->mNumberBuffers; b++)
        if (output->mBuffers[b].mData) memset(output->mBuffers[b].mData, 0, output->mBuffers[b].mDataByteSize);
    RLCapture *c = context; if (!c) return noErr;
    if (atomic_load(&c->oversized)) return noErr;
    uint32_t frames = list_frames(input), channels = list_channels(input);
    if (frames > c->callbackLimit || frames > c->maxFrames) { atomic_fetch_add(&c->oversized, 1); return noErr; }
    float factor = rl_clock_observe(c->clock, inputTime, frames, c->relaxedTiming);
    if (rl_clock_discontinuities(c->clock)) return noErr;
    for (uint32_t d = 0; d < c->destinationCount; d++) rl_ring_set_source_clock(c->destinations[d], factor);
    uint32_t right = c->channelOffset + 1 < channels ? c->channelOffset + 1 : c->channelOffset;
    for (uint32_t base = 0; base < frames; base += c->maxFrames) {
        uint32_t count = frames - base; if (count > c->maxFrames) count = c->maxFrames;
        for (uint32_t f = 0; f < count; f++) {
            c->scratch[f * 2] = input_sample(input, c->channelOffset, base + f);
            c->scratch[f * 2 + 1] = input_sample(input, right, base + f);
        }
        rl_dsp_process(c->dsp, c->scratch, count);
        float peak = 0; for (uint32_t f = 0; f < count * 2; f++) peak = fmaxf(peak, fabsf(c->scratch[f]));
        peak_store(&c->peak, peak);
        for (uint32_t d = 0; d < c->destinationCount; d++) rl_ring_write(c->destinations[d], c->scratch, count);
    }
    atomic_fetch_add(&c->frames, frames); return noErr;
}

typedef struct { RLRing *ring; double ratio, phase; uint32_t leader, rawRoot, nextAlias; bool tone; _Atomic bool audible; RLDSP *dsp; _Atomic float peak; } RLSource;
typedef struct { uint32_t offset; _Atomic float gain; _Atomic bool muted; _Atomic float peak; float smoothedGain; } RLBus;
typedef struct { _Atomic float left, right; _Atomic bool muted; float smoothLeft, smoothRight; } RLSend;
struct RLRender {
    double sampleRate;
    RLClock *clock;
    uint32_t maxFrames, callbackLimit, sourceCount, busCount;
    RLSource *sources; RLBus *buses; RLSend *sends;
    float *sourceScratch, *groupScratch, *busScratch;
    _Atomic uint64_t frames, oversized;
    RLGroupProcessor processor; void *processorContext;
    _Atomic int32_t processorError;
    _Atomic bool suspended;
    float resumeGain;
    RLSoundCheck *soundCheck; uint32_t soundCheckLeader;
};
RLRender *rl_render_create(double rate, uint32_t maxFrames, uint32_t sourceCount, uint32_t busCount) {
    if (!atomics_supported() || !maxFrames || !rl_maximum_callback_frames(rate)) return NULL;
    // All size products are checked before allocation; public C inputs need not
    // have passed the Swift model's validation.
    size_t sources = sourceCount ? sourceCount : 1, buses = busCount ? busCount : 1;
    if (sources > SIZE_MAX / buses / sizeof(RLSend) ||
        (size_t)maxFrames > SIZE_MAX / 2 / sizeof(float) / sources ||
        (size_t)maxFrames > SIZE_MAX / 2 / sizeof(float) / buses) return NULL;
    RLRender *r = calloc(1, sizeof(*r)); if (!r) return NULL;
    atomic_init(&r->processorError, 0);
    atomic_init(&r->suspended, false); r->resumeGain = 1;
    if (!atomic_is_lock_free(&r->processorError)) { free(r); return NULL; }
    r->sampleRate = rate; r->maxFrames = maxFrames; r->sourceCount = sourceCount; r->busCount = busCount;
    r->clock = rl_clock_create(rate, AudioGetHostClockFrequency());
    r->callbackLimit = rl_maximum_callback_frames(rate);
    r->sources = calloc(sourceCount ? sourceCount : 1, sizeof(RLSource));
    r->buses = calloc(busCount ? busCount : 1, sizeof(RLBus));
    r->sends = calloc((size_t)(sourceCount ? sourceCount : 1) * (busCount ? busCount : 1), sizeof(RLSend));
    r->sourceScratch = audio_samples_create((size_t)maxFrames * 2);
    r->groupScratch = audio_samples_create((size_t)maxFrames * 2 * sources);
    r->busScratch = audio_samples_create((size_t)maxFrames * 2 * buses);
    if (!r->clock || !r->sources || !r->buses || !r->sends || !r->sourceScratch || !r->groupScratch || !r->busScratch) { rl_render_destroy(r); return NULL; }
    for (uint32_t i = 0; i < sourceCount; i++) {
        r->sources[i].leader = i;
        r->sources[i].rawRoot = i; r->sources[i].nextAlias = UINT32_MAX;
        r->sources[i].dsp = rl_dsp_create(rate); if (!r->sources[i].dsp) { rl_render_destroy(r); return NULL; }
    }
    for (size_t i = 0; i < (size_t)sourceCount * busCount; i++) atomic_store(&r->sends[i].muted, true);
    return r;
}
void rl_render_destroy(RLRender *r) {
    if (!r) return;
    if (r->sources) for (uint32_t i = 0; i < r->sourceCount; i++) rl_dsp_destroy(r->sources[i].dsp);
    rl_clock_destroy(r->clock);
    free(r->sources); free(r->buses); free(r->sends); free(r->sourceScratch); free(r->groupScratch); free(r->busScratch); free(r);
}
void rl_render_set_suspended(RLRender *r, bool suspended) {
    if (r) atomic_store_explicit(&r->suspended, suspended, memory_order_release);
}
void rl_render_set_group_processor(RLRender *r, RLGroupProcessor processor, void *context) {
    if (!r) return; r->processor = processor; r->processorContext = context;
}
int32_t rl_render_processor_error(RLRender *r) { return r ? atomic_load(&r->processorError) : 0; }
bool rl_render_set_sound_check(RLRender *r, uint32_t leader, RLSoundCheck *check) {
    if (!r || leader >= r->sourceCount || r->sources[leader].leader != leader || !check) return false;
    r->soundCheck = check; r->soundCheckLeader = leader; return true;
}
void rl_render_set_source(RLRender *r, uint32_t i, RLRing *ring, double inputRate, bool tone) {
    if (!r || i >= r->sourceCount || r->sources[i].rawRoot != i) return;
    r->sources[i].ring = ring; r->sources[i].ratio = inputRate / r->sampleRate; r->sources[i].tone = tone;
}
bool rl_render_set_source_alias(RLRender *r, uint32_t source, uint32_t root) {
    if (!r || source >= r->sourceCount || root >= r->sourceCount) return false;
    if (source == root) return r->sources[source].rawRoot == source;
    if (root > source || r->sources[root].rawRoot != root ||
        (!r->sources[root].ring && !r->sources[root].tone)) return false;
    RLSource *alias = &r->sources[source], *owner = &r->sources[root];
    if (alias->rawRoot == root) return true;
    if (alias->rawRoot != source || alias->nextAlias != UINT32_MAX || alias->ring || alias->tone) return false;
    alias->rawRoot = root; alias->nextAlias = owner->nextAlias; owner->nextAlias = source;
    return true;
}
bool rl_render_set_source_group(RLRender *r, uint32_t source, uint32_t leader) {
    if (!r || source >= r->sourceCount || leader >= r->sourceCount) return false;
    if (source == leader) { r->sources[source].leader = source; return true; }
    if (r->sources[leader].leader != leader) return false;
    if (source != leader) {
        // Only direct parent links are allowed, so render never traverses a tree
        // (or encounters a cycle) on the real-time thread.
        for (uint32_t i = 0; i < r->sourceCount; i++)
            if (i != source && r->sources[i].leader == source) return false;
    }
    r->sources[source].leader = leader;
    return true;
}
void rl_render_source_controls(RLRender *r, uint32_t i, bool audible, float gain, float hp, float gate, float threshold, float ratio) {
    if (!r || i >= r->sourceCount) return;
    atomic_store(&r->sources[i].audible, audible); rl_dsp_controls(r->sources[i].dsp, gain, hp, gate, threshold, ratio);
}
void rl_render_set_bus(RLRender *r, uint32_t i, uint32_t offset) { if (r && i < r->busCount) r->buses[i].offset = offset; }
void rl_render_bus_controls(RLRender *r, uint32_t i, float gain, bool muted) {
    if (!r || i >= r->busCount) return; atomic_store(&r->buses[i].gain, db_gain(gain)); atomic_store(&r->buses[i].muted, muted);
}
void rl_render_send_controls(RLRender *r, uint32_t source, uint32_t bus, float gain, float pan, bool muted) {
    if (!r || source >= r->sourceCount || bus >= r->busCount) return;
    RLSend *s = &r->sends[(size_t)source * r->busCount + bus]; float g = db_gain(gain); pan = fmaxf(-1, fminf(pan, 1));
    // Stereo balance: unity at centre; attenuate opposite side toward the edges.
    atomic_store(&s->left, g * (pan > 0 ? 1 - pan : 1)); atomic_store(&s->right, g * (pan < 0 ? 1 + pan : 1));
    atomic_store(&s->muted, muted);
}
float rl_render_take_bus_peak(RLRender *r, uint32_t b) { return r && b < r->busCount ? atomic_exchange(&r->buses[b].peak, 0) : 0; }
float rl_render_take_source_peak(RLRender *r, uint32_t s) { return r && s < r->sourceCount ? atomic_exchange(&r->sources[s].peak, 0) : 0; }
uint64_t rl_render_frames(RLRender *r) { return r ? atomic_load(&r->frames) : 0; }
uint64_t rl_render_oversized_blocks(RLRender *r) { return r ? atomic_load(&r->oversized) : 0; }
uint64_t rl_render_timestamp_discontinuities(RLRender *r) { return r ? rl_clock_discontinuities(r->clock) : 0; }
float rl_render_clock_factor(RLRender *r) { return r ? rl_clock_factor(r->clock) : 0; }
OSStatus rl_render_callback(AudioObjectID device, const AudioTimeStamp *now, const AudioBufferList *input,
        const AudioTimeStamp *inputTime, AudioBufferList *output, const AudioTimeStamp *outputTime, void *context) {
    RLRender *r = context; if (!r || !output) return noErr;
    uint32_t frames = list_frames(output);
    for (uint32_t b = 0; b < output->mNumberBuffers; b++) if (output->mBuffers[b].mData) memset(output->mBuffers[b].mData, 0, output->mBuffers[b].mDataByteSize);
    if (atomic_load(&r->processorError) || atomic_load(&r->oversized)) return noErr;
    if (frames > r->maxFrames || frames > r->callbackLimit) { atomic_fetch_add(&r->oversized, 1); return noErr; }
    float clockFactor = rl_clock_observe(r->clock, outputTime, frames, false);
    if (rl_clock_discontinuities(r->clock)) return noErr;
    memset(r->busScratch, 0, (size_t)frames * 2 * r->busCount * sizeof(float));
    memset(r->groupScratch, 0, (size_t)frames * 2 * r->sourceCount * sizeof(float));
    // Read each unique capture once per output clock. Raw aliases fan out that
    // exact block before independent group DSP, avoiding independent FIFO/SRC
    // cursors and phase cancellation when repeated inputs are summed.
    for (uint32_t s = 0; s < r->sourceCount; s++) {
        RLSource *src = &r->sources[s];
        if (src->rawRoot != s) continue;
        if (src->tone) {
            for (uint32_t f = 0; f < frames; f++) {
                float value = (float)sin(src->phase) * .12589254f; // 440 Hz, -18 dBFS peak.
                src->phase += 2 * M_PI * 440 / r->sampleRate; if (src->phase >= 2 * M_PI) src->phase -= 2 * M_PI;
                r->sourceScratch[f * 2] = value; r->sourceScratch[f * 2 + 1] = value;
            }
        } else {
            memset(r->sourceScratch, 0, (size_t)frames * 2 * sizeof(float));
            if (src->ring) rl_ring_read_clocked(src->ring, r->sourceScratch, frames, src->ratio, clockFactor);
        }
        for (uint32_t member = s; member != UINT32_MAX; member = r->sources[member].nextAlias) {
            float *group = r->groupScratch + (size_t)r->sources[member].leader * frames * 2;
            for (size_t sample = 0; sample < (size_t)frames * 2; sample++) group[sample] += r->sourceScratch[sample];
        }
    }
    // The parent owns one effect chain and one set of sends. Child sends are
    // lowering metadata only: routing them again would duplicate the whole mix.
    for (uint32_t s = 0; s < r->sourceCount; s++) {
        RLSource *src = &r->sources[s];
        if (src->leader != s) continue;
        float *group = r->groupScratch + (size_t)s * frames * 2;
        if (r->soundCheck && s == r->soundCheckLeader) rl_sound_check_process(r->soundCheck, group, frames);
        rl_dsp_process(src->dsp, group, frames);
        if (r->processor) {
            int32_t status = r->processor(r->processorContext, s, group, frames);
            if (status) { atomic_store(&r->processorError, status); return noErr; }
        }
        if (r->soundCheck && s == r->soundCheckLeader) rl_sound_check_silence_if_stopped(r->soundCheck, group, frames);
        float peak = 0;
        for (size_t sample = 0; sample < (size_t)frames * 2; sample++) peak = fmaxf(peak, fabsf(group[sample]));
        peak_store(&src->peak, peak);
        bool audible = atomic_load(&src->audible);
        float smoothing = (float)exp(-1 / (.005 * r->sampleRate));
        for (uint32_t b = 0; b < r->busCount; b++) {
            RLSend *send = &r->sends[(size_t)s * r->busCount + b]; bool muted = atomic_load(&send->muted) || !audible;
            float left = muted ? 0 : atomic_load(&send->left), right = muted ? 0 : atomic_load(&send->right);
            float *dst = r->busScratch + (size_t)b * frames * 2;
            for (uint32_t f = 0; f < frames; f++) {
                send->smoothLeft = smoothing * send->smoothLeft + (1 - smoothing) * left;
                send->smoothRight = smoothing * send->smoothRight + (1 - smoothing) * right;
                dst[f * 2] += group[f * 2] * send->smoothLeft; dst[f * 2 + 1] += group[f * 2 + 1] * send->smoothRight;
            }
        }
    }
    bool suspended = atomic_load_explicit(&r->suspended, memory_order_acquire);
    if (suspended) r->resumeGain = 0;
    // Store one frame ramp for reuse by every bus; sourceScratch is no longer
    // needed after group processing. A control edit cannot defeat suspension.
    float resumeStep = (float)(1 / (.005 * r->sampleRate));
    for (uint32_t f = 0; f < frames; f++) {
        if (!suspended) r->resumeGain = fminf(1, r->resumeGain + resumeStep);
        r->sourceScratch[f] = r->resumeGain;
    }
    for (uint32_t b = 0; b < r->busCount; b++) {
        RLBus *bus = &r->buses[b]; float gain = atomic_load(&bus->muted) ? 0 : atomic_load(&bus->gain), peak = 0;
        float *src = r->busScratch + (size_t)b * frames * 2;
        float smoothing = (float)exp(-1 / (.005 * r->sampleRate));
        for (uint32_t f = 0; f < frames; f++) {
            bus->smoothedGain = smoothing * bus->smoothedGain + (1 - smoothing) * gain;
            for (uint32_t c = 0; c < 2; c++) {
                float sample = finite_sample(src[f * 2 + c] * bus->smoothedGain * r->sourceScratch[f]); peak = fmaxf(peak, fabsf(sample)); add_sample(output, bus->offset + c, f, sample);
            }
        }
        peak_store(&bus->peak, peak);
    }
    // Finite hard ceiling on physical device output; meters remain pre-ceiling so
    // overload remains visible. This is protection, not a mastering limiter.
    for (uint32_t b = 0; b < output->mNumberBuffers; b++) {
        AudioBuffer *buf = &output->mBuffers[b]; if (!buf->mData) continue;
        float *p = buf->mData; for (uint32_t i = 0; i < buf->mDataByteSize / sizeof(float); i++) p[i] = fmaxf(-1, fminf(1, finite_sample(p[i])));
    }
    atomic_fetch_add(&r->frames, frames); return noErr;
}
AudioDeviceIOProc rl_capture_io_proc(void) { return rl_capture_callback; }
AudioDeviceIOProc rl_render_io_proc(void) { return rl_render_callback; }
