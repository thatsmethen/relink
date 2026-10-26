#ifndef RAVELINK_REALTIME_H
#define RAVELINK_REALTIME_H
#include <CoreAudio/CoreAudio.h>
#include <stdint.h>
#include <stdbool.h>
#include "SoundCheck.h"
#include "Clock.h"

// All allocation occurs while the graph is stopped. Controls and telemetry use
// lock-free C11 atomics; each FIFO has exactly one capture writer and render reader.
typedef struct RLRing RLRing;
typedef struct RLCapture RLCapture;
typedef struct RLRender RLRender;
typedef struct RLDSP RLDSP;
// Supported live-device envelope, independent of allocated scratch capacity.
// Returns zero for invalid rates; callbacks above this limit fail closed.
uint32_t rl_maximum_callback_frames(double sampleRate);

RLRing *rl_ring_create(uint32_t capacity);
void rl_ring_destroy(RLRing *ring);
uint32_t rl_ring_write(RLRing *ring, const float *stereo, uint32_t frames);
uint32_t rl_ring_read(RLRing *ring, float *stereo, uint32_t frames, double ratio);
uint32_t rl_ring_read_clocked(RLRing *ring, float *stereo, uint32_t frames, double ratio, float outputClockFactor);
// Called only by the producer; zero marks unavailable hardware timestamps.
void rl_ring_set_source_clock(RLRing *ring, float factor);
// Configure the band-limited resampling kernel before starting either callback.
bool rl_ring_prepare_ratio(RLRing *ring, double ratio);
// Configure streaming device clocks before either callback starts. Each write
// then represents one producer callback block (split blocks use the largest
// chunk). Tracks producer/consumer maxima for stable FIFO headroom; rates must
// be 8...384 kHz and have a ratio within 1:24...24:1. prepare_ratio alone is
// retained for raw prefilled-buffer processing with a 48 kHz control timebase.
bool rl_ring_prepare_clock(RLRing *ring, double inputSampleRate, double outputSampleRate);
// Seed the declared callback maxima while stopped, before any capture arrives.
// Requires prepare_clock; rejects values outside the supported device envelope
// or a FIFO too small for four periods plus sinc lookahead.
bool rl_ring_prepare_buffers(RLRing *ring, uint32_t inputMaximumFrames, uint32_t outputMaximumFrames);
uint64_t rl_ring_dropped(RLRing *ring);
uint64_t rl_ring_underruns(RLRing *ring);
bool rl_ring_clock_mismatch(RLRing *ring);
// Request from control while its producer is stopped. Consumer discards queued
// frames and resets SRC history on its next read; it never edits writer state.
void rl_ring_request_reset(RLRing *ring);

RLDSP *rl_dsp_create(double sampleRate);
void rl_dsp_destroy(RLDSP *dsp);
void rl_dsp_controls(RLDSP *dsp, float gainDB, float highPassHz, float gateDB,
                     float compressorDB, float compressorRatio);
void rl_dsp_process(RLDSP *dsp, float *stereo, uint32_t frames);

RLCapture *rl_capture_create(double sampleRate, uint32_t channelOffset,
                             uint32_t maxFrames, uint32_t destinationCount);
void rl_capture_destroy(RLCapture *capture);
void rl_capture_set_destination(RLCapture *capture, uint32_t index, RLRing *ring);
void rl_capture_controls(RLCapture *capture, float gainDB, float highPassHz,
                          float gateDB, float compressorDB, float compressorRatio);
float rl_capture_take_peak(RLCapture *capture);
uint64_t rl_capture_frames(RLCapture *capture);
uint64_t rl_capture_oversized_blocks(RLCapture *capture);
uint64_t rl_capture_timestamp_discontinuities(RLCapture *capture);
float rl_capture_clock_factor(RLCapture *capture);
void rl_capture_reset_timing(RLCapture *capture);
void rl_capture_allow_idle_pauses(RLCapture *capture, bool allowed);
OSStatus rl_capture_callback(AudioObjectID device, const AudioTimeStamp *now,
                             const AudioBufferList *input, const AudioTimeStamp *inputTime,
                             AudioBufferList *output, const AudioTimeStamp *outputTime, void *context);
AudioDeviceIOProc rl_capture_io_proc(void);

RLRender *rl_render_create(double sampleRate, uint32_t maxFrames,
                           uint32_t sourceCount, uint32_t busCount);
void rl_render_destroy(RLRender *render);
// Independent fail-closed gate. Processing/draining continues while suspended;
// ordinary channel/bus controls cannot reopen it. Resume has a 5 ms ramp.
void rl_render_set_suspended(RLRender *render, bool suspended);
// Install only while stopped. Context must outlive every callback. A nonzero
// return permanently latches silence for this graph until explicit rebuild.
typedef int32_t (*RLGroupProcessor)(void *context, uint32_t sourceIndex, float *stereo, uint32_t frames);
void rl_render_set_group_processor(RLRender *render, RLGroupProcessor processor, void *context);
int32_t rl_render_processor_error(RLRender *render);
// Configure only while stopped. Sound Check storage must outlive callbacks.
bool rl_render_set_sound_check(RLRender *render, uint32_t leader, RLSoundCheck *check);
void rl_render_set_source(RLRender *render, uint32_t index, RLRing *ring,
                          double inputSampleRate, bool tone);
// Configure only while stopped, before assigning a ring/tone to the alias.
// An earlier configured non-alias root is read/resampled exactly once and its
// identical raw block feeds every alias's independent group/effects/sends.
// Reparenting, chains, cycles, and aliasing a source with its own aliases fail.
bool rl_render_set_source_alias(RLRender *render, uint32_t source, uint32_t root);
// Configure while callbacks are stopped. Leaders must be direct roots; invalid
// indices, chains, and reparenting a leader with children are rejected.
// The leader's controls and sends apply once to the sum of all group members.
bool rl_render_set_source_group(RLRender *render, uint32_t source, uint32_t leader);
void rl_render_source_controls(RLRender *render, uint32_t index, bool audible,
                               float gainDB, float highPassHz, float gateDB,
                               float compressorDB, float compressorRatio);
void rl_render_set_bus(RLRender *render, uint32_t index, uint32_t channelOffset);
void rl_render_bus_controls(RLRender *render, uint32_t index, float gainDB, bool muted);
void rl_render_send_controls(RLRender *render, uint32_t source, uint32_t bus,
                             float gainDB, float pan, bool muted);
float rl_render_take_bus_peak(RLRender *render, uint32_t bus);
float rl_render_take_source_peak(RLRender *render, uint32_t source);
uint64_t rl_render_frames(RLRender *render);
uint64_t rl_render_oversized_blocks(RLRender *render);
uint64_t rl_render_timestamp_discontinuities(RLRender *render);
float rl_render_clock_factor(RLRender *render);
OSStatus rl_render_callback(AudioObjectID device, const AudioTimeStamp *now,
                            const AudioBufferList *input, const AudioTimeStamp *inputTime,
                            AudioBufferList *output, const AudioTimeStamp *outputTime, void *context);
AudioDeviceIOProc rl_render_io_proc(void);
#endif
