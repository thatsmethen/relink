#include "SoundCheck.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <limits.h>

#define MAILBOX_SIZE 16
struct RLSoundCheck {
    float *samples;
    uint64_t capacity, length, position;
    double rate;
    uint32_t mode, fadeFrames;
    uint32_t commands[MAILBOX_SIZE];
    _Atomic uint64_t written, read, publishedState;
    _Atomic float peak;
};

RLSoundCheck *rl_sound_check_create(double rate, double seconds) {
    if (!isfinite(rate) || rate < 8000 || rate > 384000 ||
        !isfinite(seconds) || seconds <= 0 || seconds > 30) return NULL;
    double count = floor(rate * seconds);
    if (count < 1 || count > (double)(SIZE_MAX / 2 / sizeof(float))) return NULL;
    RLSoundCheck *c = calloc(1, sizeof(*c)); if (!c) return NULL;
    atomic_init(&c->written, 0); atomic_init(&c->read, 0);
    atomic_init(&c->publishedState, 0); atomic_init(&c->peak, 0);
    if (!atomic_is_lock_free(&c->written) || !atomic_is_lock_free(&c->read) ||
        !atomic_is_lock_free(&c->publishedState) ||
        !atomic_is_lock_free(&c->peak)) { free(c); return NULL; }
    c->capacity = (uint64_t)count; c->rate = rate;
    c->fadeFrames = (uint32_t)(rate * .005);
    c->samples = calloc((size_t)c->capacity * 2, sizeof(float));
    if (!c->samples) { free(c); return NULL; }
    // Touch every sample before starting the callback. calloc alone can defer
    // zero-page faults until recording begins.
    volatile float *storage = c->samples;
    for (uint64_t i = 0; i < c->capacity * 2; i++) storage[i] = 0;
    return c;
}
void rl_sound_check_destroy(RLSoundCheck *c) { if (c) { free(c->samples); free(c); } }
bool rl_sound_check_command(RLSoundCheck *c, uint32_t command) {
    if (!c || command < 1 || command > 4) return false;
    uint64_t written = atomic_load_explicit(&c->written, memory_order_relaxed);
    uint64_t read = atomic_load_explicit(&c->read, memory_order_acquire);
    if (written - read >= MAILBOX_SIZE) return false;
    c->commands[written % MAILBOX_SIZE] = command;
    atomic_store_explicit(&c->written, written + 1, memory_order_release); return true;
}
static void apply_commands(RLSoundCheck *c) {
    uint64_t read = atomic_load_explicit(&c->read, memory_order_relaxed);
    const uint64_t written = atomic_load_explicit(&c->written, memory_order_acquire);
    while (read != written) {
        uint32_t command = c->commands[read % MAILBOX_SIZE];
        switch (command) {
            case 1: c->length = 0; c->position = 0; c->mode = 1; break;
            case 2: c->mode = c->length ? 2 : 0; c->position = 0; break;
            case 3: c->mode = c->length ? 3 : 0; c->position = 0; break;
            case 4: c->length = 0; c->position = 0; c->mode = 0; break;
        }
        read++;
    }
    atomic_store_explicit(&c->read, read, memory_order_release);
}
void rl_sound_check_process(RLSoundCheck *c, float *stereo, uint32_t frames) {
    if (!c || !stereo) return;
    apply_commands(c);
    float peak = 0;
    if (c->mode == 1) {
        uint64_t count = c->capacity - c->length;
        if (count > frames) count = frames;
        for (uint64_t i = 0; i < count * 2; i++) {
            float value = isfinite(stereo[i]) ? stereo[i] : 0;
            c->samples[c->length * 2 + i] = value;
            peak = fmaxf(peak, fabsf(value));
        }
        c->length += count;
        if (c->length == c->capacity) c->mode = 2;
    }
    if (c->mode == 3 && c->length) {
        uint64_t fade = c->fadeFrames;
        if (fade > c->length / 2) fade = c->length / 2;
        for (uint32_t f = 0; f < frames; f++) {
            float gain = 1;
            if (fade) {
                if (c->position < fade) gain = (float)c->position / (float)fade;
                uint64_t remaining = c->length - 1 - c->position;
                if (remaining < fade) gain = fminf(gain, (float)remaining / (float)fade);
            }
            for (uint32_t side = 0; side < 2; side++) {
                float value = c->samples[c->position * 2 + side] * gain;
                stereo[(size_t)f * 2 + side] = value; peak = fmaxf(peak, fabsf(value));
            }
            if (++c->position == c->length) c->position = 0;
        }
    } else memset(stereo, 0, (size_t)frames * 2 * sizeof(float));
    // Single callback writer; meter consumer uses exchange. A lost max update
    // can only shorten a UI meter hold and never touches audio data.
    float old = atomic_load_explicit(&c->peak, memory_order_relaxed);
    atomic_store_explicit(&c->peak, fmaxf(old, peak), memory_order_relaxed);
    atomic_store_explicit(&c->publishedState, (c->length << 2) | c->mode, memory_order_release);
}
void rl_sound_check_silence_if_stopped(RLSoundCheck *c, float *stereo, uint32_t frames) {
    if (c && stereo && c->mode != 3) memset(stereo, 0, (size_t)frames * 2 * sizeof(float));
}
void rl_sound_check_snapshot(RLSoundCheck *c, uint32_t *mode, uint64_t *frames) {
    uint64_t state = c ? atomic_load_explicit(&c->publishedState, memory_order_acquire) : 0;
    if (mode) *mode = (uint32_t)(state & 3);
    if (frames) *frames = state >> 2;
}
uint32_t rl_sound_check_mode(RLSoundCheck *c) { uint32_t mode; rl_sound_check_snapshot(c, &mode, NULL); return mode; }
uint64_t rl_sound_check_frames(RLSoundCheck *c) { uint64_t frames; rl_sound_check_snapshot(c, NULL, &frames); return frames; }
double rl_sound_check_rate(RLSoundCheck *c) { return c ? c->rate : 0; }
float rl_sound_check_take_peak(RLSoundCheck *c) { return c ? atomic_exchange_explicit(&c->peak, 0, memory_order_relaxed) : 0; }
