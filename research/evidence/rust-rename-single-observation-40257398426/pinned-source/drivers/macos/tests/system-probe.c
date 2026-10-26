/* Independently authored Core Audio system observer/player, 2026-10-24.
 * Uses Apple's public AudioHardware/AudioStream APIs; does not load driver
 * internals. This test instrument is not part of the shipping Rust product.
 * All allocation and file I/O occur outside the real-time IOProc. */
#include <CoreAudio/CoreAudio.h>
#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>

#define EXPECTED_RATE 48000.0
#define MAX_CALLBACK_FRAMES 8192u
#define MAX_EVENTS 262144u
#define PI 3.14159265358979323846
#include "probe-diagnostics.h"

typedef struct {
    double sample_time;
    uint64_t host_time;
    uint64_t observed_host_time;
    uint32_t flags, frames, stored_frames;
} Event;
typedef struct {
    bool recording;
    double amplitude, left_hz, right_hz;
    uint64_t target_frames;
    float *pcm;
    Event *events;
    Diagnostics *diagnostics;
    atomic_bool accepting;
    atomic_uint in_flight, fault;
    atomic_uint_fast64_t frames, event_count;
} Context;
typedef struct {
    const char *mode, *uid, *report, *wav, *events, *ready, *diagnostics;
    double seconds, amplitude, left_hz, right_hz;
} Options;

static uint64_t monotonic_ns(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return 0;
    return (uint64_t)now.tv_sec * 1000000000ULL + (uint64_t)now.tv_nsec;
}
static void sleep_ms(unsigned milliseconds) {
    struct timespec duration = {milliseconds / 1000u, (long)(milliseconds % 1000u) * 1000000L};
    while (nanosleep(&duration, &duration) != 0 && errno == EINTR) {}
}
static AudioObjectPropertyAddress address(AudioObjectPropertySelector selector, AudioObjectPropertyScope scope) {
    return (AudioObjectPropertyAddress){selector, scope, kAudioObjectPropertyElementMain};
}
static OSStatus read_exact(AudioObjectID object, AudioObjectPropertyAddress property, void *destination, UInt32 expected) {
    UInt32 bytes = expected;
    OSStatus status = AudioObjectGetPropertyData(object, &property, 0, NULL, &bytes, destination);
    if (status == noErr && bytes != expected) return kAudioHardwareBadPropertySizeError;
    return status;
}
static OSStatus find_device(const char *uid, AudioObjectID *found, unsigned *matches) {
    AudioObjectPropertyAddress property = address(kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal);
    UInt32 bytes = 0;
    OSStatus status = AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &property, 0, NULL, &bytes);
    if (status != noErr) return status;
    if (bytes > 1024u * 1024u || bytes % sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
    AudioObjectID *devices = malloc(bytes ? bytes : 1);
    if (!devices) return kAudioHardwareUnspecifiedError;
    status = AudioObjectGetPropertyData(kAudioObjectSystemObject, &property, 0, NULL, &bytes, devices);
    *found = kAudioObjectUnknown; *matches = 0;
    if (status == noErr) {
        CFStringRef requested = CFStringCreateWithCString(NULL, uid, kCFStringEncodingUTF8);
        if (!requested) status = kAudioHardwareIllegalOperationError;
        for (UInt32 index = 0; requested && status == noErr && index < bytes / sizeof(AudioObjectID); index++) {
            CFStringRef candidate = NULL;
            status = read_exact(devices[index], address(kAudioDevicePropertyDeviceUID, kAudioObjectPropertyScopeGlobal), &candidate, sizeof candidate);
            if (status == noErr && candidate) {
                if (CFEqual(candidate, requested)) { *found = devices[index]; (*matches)++; }
                CFRelease(candidate);
            }
        }
        if (requested) CFRelease(requested);
    }
    free(devices);
    return status;
}
static OSStatus stream_format(AudioObjectID device, AudioObjectPropertyScope scope, AudioStreamBasicDescription *format) {
    AudioObjectPropertyAddress property = address(kAudioDevicePropertyStreams, scope);
    UInt32 bytes = 0;
    OSStatus status = AudioObjectGetPropertyDataSize(device, &property, 0, NULL, &bytes);
    if (status != noErr) return status;
    if (bytes != sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
    AudioObjectID stream = kAudioObjectUnknown;
    status = read_exact(device, property, &stream, sizeof stream);
    if (status != noErr) return status;
    return read_exact(stream, address(kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal), format, sizeof *format);
}
static bool supported_format(const AudioStreamBasicDescription *f) {
    return isfinite(f->mSampleRate) && f->mSampleRate == EXPECTED_RATE && f->mFormatID == kAudioFormatLinearPCM
        && f->mFormatFlags == kAudioFormatFlagsNativeFloatPacked && f->mChannelsPerFrame == 2
        && f->mBitsPerChannel == 32 && f->mBytesPerFrame == 8 && f->mBytesPerPacket == 8 && f->mFramesPerPacket == 1;
}
static OSStatus set_direction(AudioObjectID device, AudioDeviceIOProcID proc, AudioObjectPropertyScope scope, bool enabled) {
    // This fixture requires exactly one stream in each direction. The HAL owns
    // IOProc usage, so the plug-in need not implement this client-side property.
    AudioHardwareIOProcStreamUsage usage = { .mIOProc = (void *)proc, .mNumberStreams = 1, .mStreamIsOn = {enabled ? 1u : 0u} };
    AudioObjectPropertyAddress property = address(kAudioDevicePropertyIOProcStreamUsage, scope);
    return AudioObjectSetPropertyData(device, &property, 0, NULL, sizeof usage, &usage);
}
static OSStatus diagnostic_listener(AudioObjectID object, UInt32 count,
                                    const AudioObjectPropertyAddress *addresses, void *context) {
    diagnostic_notify(context, object, count, addresses, mach_absolute_time());
    return noErr;
}
static void diagnostic_register(Diagnostics *diagnostics, AudioObjectID object,
                                 AudioObjectPropertySelector selector) {
    DiagnosticListener *listener = &diagnostics->listeners[diagnostics->listener_count++];
    listener->object = object;
    listener->address = address(selector, kAudioObjectPropertyScopeGlobal);
    listener->available = AudioObjectHasProperty(object, &listener->address);
    if (listener->available) {
        listener->add_status = AudioObjectAddPropertyListener(object, &listener->address, diagnostic_listener, diagnostics);
        listener->added = listener->add_status == noErr;
    }
}
static OSStatus diagnostic_start(Context *context, AudioObjectID device) {
    Diagnostics *diagnostics = calloc(1, sizeof *diagnostics);
    if (!diagnostics) return kAudioHardwareUnspecifiedError;
    diagnostic_initialize(diagnostics);
    context->diagnostics = diagnostics;
    diagnostics->timings = calloc(MAX_EVENTS, sizeof(TimingEvent));
    if (!diagnostics->timings) return kAudioHardwareUnspecifiedError;
    volatile unsigned char *bytes = (volatile unsigned char *)diagnostics->timings;
    for (size_t p = 0; p < MAX_EVENTS * sizeof(TimingEvent); p += 4096) bytes[p] = 0;
    const AudioObjectPropertySelector selectors[] = { kAudioDeviceProcessorOverload,
        kAudioDevicePropertyDeviceIsRunning, kAudioDevicePropertyDeviceIsAlive,
        kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyBufferFrameSize };
    _Static_assert(sizeof selectors / sizeof selectors[0] + 2 <= DIAGNOSTIC_LISTENERS,
        "Diagnostic registration must fit its fixed listener storage");
    for (unsigned index = 0; index < sizeof selectors / sizeof selectors[0]; index++)
        diagnostic_register(diagnostics, device, selectors[index]);
    const AudioObjectPropertyScope scopes[] = { kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput };
    for (unsigned index = 0; index < 2; index++) {
        AudioObjectID stream = 0;
        OSStatus status = read_exact(device, address(kAudioDevicePropertyStreams, scopes[index]), &stream, sizeof stream);
        if (status != noErr) return status;
        diagnostic_register(diagnostics, stream, kAudioStreamPropertyPhysicalFormat);
    }
    return noErr;
}
static void diagnostic_stop(Diagnostics *diagnostics) {
    // No new publisher may enter after this gate, even if an unregister fails.
    // Diagnostics storage is deliberately retained until process exit: a late
    // CoreAudio notification can still safely inspect its closed gate.
    atomic_store_explicit(&diagnostics->accepting, false, memory_order_seq_cst);
    for (unsigned index = 0; index < diagnostics->listener_count; index++) {
        DiagnosticListener *listener = &diagnostics->listeners[index];
        if (listener->added) listener->remove_status = AudioObjectRemovePropertyListener(
            listener->object, &listener->address, diagnostic_listener, diagnostics);
    }
    uint64_t deadline = monotonic_ns() + 2000000000ULL;
    while (atomic_load_explicit(&diagnostics->in_flight, memory_order_seq_cst) && monotonic_ns() < deadline) sleep_ms(1);
    diagnostics->quiescent = atomic_load_explicit(&diagnostics->in_flight, memory_order_seq_cst) == 0;
    diagnostics->listeners_stopped = true;
}
static bool diagnostic_complete(const Diagnostics *diagnostics) {
    if (!diagnostics || !diagnostics->timings || !diagnostics->listeners_stopped || !diagnostics->quiescent
        || atomic_load_explicit(&diagnostics->dropped, memory_order_acquire)) return false;
    for (unsigned index = 0; index < diagnostics->listener_count; index++) {
        const DiagnosticListener *listener = &diagnostics->listeners[index];
        if (listener->available && (!listener->added || listener->add_status || listener->remove_status)) return false;
    }
    return true;
}
static void latch_fault(Context *context, unsigned value) {
    unsigned expected = 0;
    atomic_compare_exchange_strong_explicit(&context->fault, &expected, value, memory_order_release, memory_order_relaxed);
}
static void silence(AudioBufferList *buffers) {
    if (!buffers) return;
    for (UInt32 index = 0; index < buffers->mNumberBuffers; index++) {
        AudioBuffer *buffer = &buffers->mBuffers[index];
        if (buffer->mData) memset(buffer->mData, 0, buffer->mDataByteSize);
    }
}
static OSStatus io_proc(AudioObjectID device, const AudioTimeStamp *now, const AudioBufferList *input,
                        const AudioTimeStamp *input_time, AudioBufferList *output,
                        const AudioTimeStamp *output_time, void *data) {
    (void)device;
    Context *context = data;
    uint64_t entry_tick = context->diagnostics ? mach_absolute_time() : 0;
    uint64_t timing_index = UINT64_MAX;
    unsigned already_active = atomic_fetch_add_explicit(&context->in_flight, 1, memory_order_seq_cst);
    silence(output);
    // Buffer/event storage has one owner. An unexpected overlapping invocation
    // is an observer failure, never permission to race or invent a timing row.
    if (already_active) { latch_fault(context, 8); goto done; }
    if (!atomic_load_explicit(&context->accepting, memory_order_seq_cst)) goto done;
    const AudioBufferList *buffers = context->recording ? input : output;
    const AudioTimeStamp *timestamp = context->recording ? input_time : output_time;
    uint64_t position = atomic_load_explicit(&context->frames, memory_order_relaxed);
    if (position >= context->target_frames) goto done;
    if (!buffers || buffers->mNumberBuffers != 1 || !buffers->mBuffers[0].mData
        || buffers->mBuffers[0].mNumberChannels != 2 || buffers->mBuffers[0].mDataByteSize % 8u) { latch_fault(context, 1); goto done; }
    const UInt32 frames = buffers->mBuffers[0].mDataByteSize / 8u;
    if (!frames || frames > MAX_CALLBACK_FRAMES) { latch_fault(context, 2); goto done; }
    if (!timestamp || !(timestamp->mFlags & kAudioTimeStampSampleTimeValid) || !(timestamp->mFlags & kAudioTimeStampHostTimeValid)
        || !isfinite(timestamp->mSampleTime) || timestamp->mSampleTime < 0.0 || timestamp->mHostTime == 0) { latch_fault(context, 3); goto done; }
    uint64_t event_index = atomic_load_explicit(&context->event_count, memory_order_relaxed);
    if (event_index >= MAX_EVENTS) { latch_fault(context, 4); goto done; }
    UInt32 count = frames;
    if ((uint64_t)count > context->target_frames - position) count = (UInt32)(context->target_frames - position);
    if (context->recording) {
        memcpy(context->pcm + position * 2, buffers->mBuffers[0].mData, (size_t)count * 8u);
    } else {
        float *samples = output->mBuffers[0].mData;
        for (UInt32 frame = 0; frame < count; frame++) {
            double time = (timestamp->mSampleTime + frame) / EXPECTED_RATE;
            samples[frame * 2] = (float)(context->amplitude * sin(2.0 * PI * context->left_hz * time));
            samples[frame * 2 + 1] = (float)(context->amplitude * sin(2.0 * PI * context->right_hz * time));
        }
    }
    context->events[event_index] = (Event){ timestamp->mSampleTime, timestamp->mHostTime, mach_absolute_time(), timestamp->mFlags, frames, count };
    if (context->diagnostics) {
        context->diagnostics->timings[event_index] = (TimingEvent){
            .entry_host_time = entry_tick, .current_host_time = now ? now->mHostTime : 0,
            .current_sample_time = now ? now->mSampleTime : 0,
            .current_flags = now ? now->mFlags : 0, .current_present = now != NULL };
        timing_index = event_index;
    }
    atomic_store_explicit(&context->event_count, event_index + 1, memory_order_release);
    atomic_store_explicit(&context->frames, position + count, memory_order_release);
done:
    if (timing_index != UINT64_MAX) context->diagnostics->timings[timing_index].exit_host_time = mach_absolute_time();
    atomic_fetch_sub_explicit(&context->in_flight, 1, memory_order_seq_cst);
    return noErr;
}
static void json_string(FILE *file, const char *value) {
    fputc('"', file);
    for (const unsigned char *p = (const unsigned char *)value; *p; p++) {
        if (*p == '"' || *p == '\\') { fputc('\\', file); fputc(*p, file); }
        else if (*p < 32) fprintf(file, "\\u%04x", *p);
        else fputc(*p, file);
    }
    fputc('"', file);
}
static bool u16(FILE *file, uint16_t value) {
    unsigned char bytes[2] = {(unsigned char)value, (unsigned char)(value >> 8)};
    return fwrite(bytes, 1, 2, file) == 2;
}
static bool u32(FILE *file, uint32_t value) {
    unsigned char bytes[4] = {(unsigned char)value, (unsigned char)(value >> 8), (unsigned char)(value >> 16), (unsigned char)(value >> 24)};
    return fwrite(bytes, 1, 4, file) == 4;
}
static bool write_wav(const char *path, const float *pcm, uint64_t frames) {
    if (frames > (UINT32_MAX - 36u) / 8u) return false;
    FILE *file = fopen(path, "wb"); if (!file) return false;
    uint32_t bytes = (uint32_t)frames * 8u;
    bool ok = fwrite("RIFF", 1, 4, file) == 4 && u32(file, 36u + bytes) && fwrite("WAVEfmt ", 1, 8, file) == 8
        && u32(file, 16) && u16(file, 3) && u16(file, 2) && u32(file, 48000) && u32(file, 48000 * 8)
        && u16(file, 8) && u16(file, 32) && fwrite("data", 1, 4, file) == 4 && u32(file, bytes)
        && fwrite(pcm, 1, bytes, file) == bytes;
    return fclose(file) == 0 && ok;
}
static bool write_events(const char *path, const Event *events, uint64_t count) {
    FILE *file = fopen(path, "w"); if (!file) return false;
    fputs("sample_time,host_time,observed_host_time,flags,frames,stored_frames\n", file);
    for (uint64_t index = 0; index < count; index++) {
        const Event *e = &events[index];
        fprintf(file, "%.17g,%llu,%llu,%u,%u,%u\n", e->sample_time, (unsigned long long)e->host_time,
                (unsigned long long)e->observed_host_time, e->flags, e->frames, e->stored_frames);
    }
    return fclose(file) == 0;
}
static bool write_diagnostics(const char *path, const Context *context, bool io_quiescent) {
    const Diagnostics *d = context->diagnostics;
    FILE *file = fopen(path, "wx"); if (!file) return false;
    mach_timebase_info_data_t timebase; mach_timebase_info(&timebase);
    uint64_t count = atomic_load_explicit(&context->event_count, memory_order_acquire);
    uint64_t notifications = atomic_load_explicit(&d->notification_count, memory_order_acquire);
    fprintf(file, "{\"schema\":1,\"complete\":%s,\"io_quiescent\":%s,\"listener_quiescent\":%s,\"host_time_numer\":%u,\"host_time_denom\":%u,\"event_count\":%llu,\"notification_reservations\":%llu,\"dropped_notifications\":%llu,\"lifecycle\":{\"start_request\":%llu,\"start_return\":%llu,\"stop_request\":%llu,\"stop_return\":%llu,\"destroy_request\":%llu,\"destroy_return\":%llu},\"listeners\":[",
        diagnostic_complete(d) && io_quiescent ? "true" : "false", io_quiescent ? "true" : "false", d->quiescent ? "true" : "false",
        timebase.numer, timebase.denom, (unsigned long long)count, (unsigned long long)notifications,
        (unsigned long long)atomic_load_explicit(&d->dropped, memory_order_acquire),
        (unsigned long long)d->start_request, (unsigned long long)d->start_return,
        (unsigned long long)d->stop_request, (unsigned long long)d->stop_return,
        (unsigned long long)d->destroy_request, (unsigned long long)d->destroy_return);
    for (unsigned index = 0; index < d->listener_count; index++) {
        const DiagnosticListener *l = &d->listeners[index];
        fprintf(file, "%s{\"object\":%u,\"selector\":%u,\"scope\":%u,\"element\":%u,\"available\":%s,\"added\":%s,\"add_status\":%d,\"remove_status\":%d}",
            index ? "," : "", l->object, l->address.mSelector, l->address.mScope, l->address.mElement,
            l->available ? "true" : "false", l->added ? "true" : "false", l->add_status, l->remove_status);
    }
    fputs("],\"notifications\":[", file);
    bool comma = false;
    for (uint64_t index = 0; index < notifications && index < DIAGNOSTIC_NOTIFICATION_CAPACITY; index++) {
        const NotificationEvent *event = &d->notifications[index];
        if (atomic_load_explicit(&event->committed, memory_order_acquire) != index + 1) continue;
        fprintf(file, "%s{\"index\":%llu,\"host_time\":%llu,\"object\":%u,\"selector\":%u,\"scope\":%u,\"element\":%u}",
            comma ? "," : "", (unsigned long long)index, (unsigned long long)event->host_time, event->object,
            event->address.mSelector, event->address.mScope, event->address.mElement);
        comma = true;
    }
    fputs("],\"timings\":[", file);
    if (io_quiescent && d->timings) for (uint64_t index = 0; index < count; index++) {
        const TimingEvent *event = &d->timings[index];
        fprintf(file, "%s{\"index\":%llu,\"entry_host_time\":%llu,\"after_pcm_host_time\":%llu,\"exit_host_time\":%llu,\"current_present\":%s,\"current_host_time\":%llu,\"current_flags\":%u,\"current_sample_time\":",
            index ? "," : "", (unsigned long long)index, (unsigned long long)event->entry_host_time,
            (unsigned long long)context->events[index].observed_host_time, (unsigned long long)event->exit_host_time,
            event->current_present ? "true" : "false", (unsigned long long)event->current_host_time, event->current_flags);
        if (isfinite(event->current_sample_time)) fprintf(file, "%.17g", event->current_sample_time);
        else fputs("null", file);
        fputc('}', file);
    }
    fputs("]}\n", file);
    bool saved = !ferror(file);
    return fclose(file) == 0 && saved;
}
static bool ready_file(const char *path, uint64_t frames, uint64_t now) {
    if (!path) return true;
    // Publish a complete document atomically. The controller must never see a
    // newly created, empty file while the readiness JSON is still being written.
    size_t length = strlen(path);
    if (length > 4096) return false;
    char *pending = malloc(length + 9);
    if (!pending) return false;
    snprintf(pending, length + 9, "%s.pending", path);
    FILE *file = fopen(pending, "wx");
    if (!file) { free(pending); return false; }
    fprintf(file, "{\"ready\":true,\"frames\":%llu,\"monotonic_ns\":%llu}\n", (unsigned long long)frames, (unsigned long long)now);
    bool saved = fclose(file) == 0;
    // link is atomic and refuses an existing readiness name. Both paths live
    // in the same evidence directory on the same filesystem.
    bool published = saved && link(pending, path) == 0;
    unlink(pending);
    free(pending);
    return published;
}
static bool write_report(const Options *o, AudioObjectID device, unsigned matches, OSStatus status, OSStatus stop_status,
                         OSStatus destroy_status, Context *context, bool complete, bool ready, uint64_t started_ns, uint64_t ended_ns) {
    FILE *file = fopen(o->report, "w"); if (!file) return false;
    mach_timebase_info_data_t timebase; mach_timebase_info(&timebase);
    uint64_t frames = context ? atomic_load(&context->frames) : 0;
    uint64_t count = context ? atomic_load(&context->event_count) : 0;
    fprintf(file, "{\"schema\":1,\"mode\":"); json_string(file, o->mode); fputs(",\"uid\":", file); json_string(file, o->uid);
    fprintf(file, ",\"device_id\":%u,\"matches\":%u,\"status\":%d,\"stop_status\":%d,\"destroy_status\":%d,\"fault\":%u,\"complete\":%s,\"ready\":%s,\"frames\":%llu,\"events\":%llu,\"target_frames\":%llu,\"sample_rate\":48000,\"channels\":2,\"amplitude\":%.17g,\"left_hz\":%.17g,\"right_hz\":%.17g,\"started_monotonic_ns\":%llu,\"ended_monotonic_ns\":%llu,\"host_time_numer\":%u,\"host_time_denom\":%u}\n",
            device, matches, status, stop_status, destroy_status, context ? atomic_load(&context->fault) : 0, complete ? "true" : "false", ready ? "true" : "false",
            (unsigned long long)frames, (unsigned long long)count, (unsigned long long)(context ? context->target_frames : 0), o->amplitude, o->left_hz, o->right_hz,
            (unsigned long long)started_ns, (unsigned long long)ended_ns, timebase.numer, timebase.denom);
    return fclose(file) == 0;
}
static bool numeric(const char *value, double *output) {
    char *end = NULL; errno = 0; double result = strtod(value, &end);
    if (errno || !end || *end || !isfinite(result)) return false;
    *output = result; return true;
}
static bool options(int argc, char **argv, Options *o) {
    *o = (Options){.seconds = 20.0, .amplitude = 0.125, .left_hz = 440.0, .right_hz = 997.0};
    if (argc < 2) return false;
    o->mode = argv[1];
    if (strcmp(o->mode, "discover") && strcmp(o->mode, "record") && strcmp(o->mode, "play")) return false;
    for (int index = 2; index < argc; index += 2) {
        if (index + 1 >= argc) return false;
        const char *key = argv[index], *value = argv[index + 1];
        if (!strcmp(key, "--uid")) o->uid = value;
        else if (!strcmp(key, "--report")) o->report = value;
        else if (!strcmp(key, "--wav")) o->wav = value;
        else if (!strcmp(key, "--events")) o->events = value;
        else if (!strcmp(key, "--ready")) o->ready = value;
        else if (!strcmp(key, "--diagnostics")) o->diagnostics = value;
        else if (!strcmp(key, "--seconds")) { if (!numeric(value, &o->seconds)) return false; }
        else if (!strcmp(key, "--amplitude")) { if (!numeric(value, &o->amplitude)) return false; }
        else if (!strcmp(key, "--left-hz")) { if (!numeric(value, &o->left_hz)) return false; }
        else if (!strcmp(key, "--right-hz")) { if (!numeric(value, &o->right_hz)) return false; }
        else return false;
    }
    bool io = strcmp(o->mode, "discover") != 0;
    return o->uid && *o->uid && o->report && (!io || (o->events && o->ready))
        && (strcmp(o->mode, "record") || o->wav) && o->seconds >= 0.25 && o->seconds <= 60.0
        && o->amplitude >= 0.0 && o->amplitude <= 0.5 && o->left_hz >= 20.0 && o->left_hz < 20000.0
        && o->right_hz >= 20.0 && o->right_hz < 20000.0;
}
int main(int argc, char **argv) {
    Options o;
    if (!options(argc, argv, &o)) {
        fprintf(stderr, "usage: system-probe discover|play|record --uid UID --report JSON [--seconds 20 --events CSV --ready FILE --wav WAV --diagnostics JSON --amplitude .125 --left-hz 440 --right-hz 997]\n");
        return 2;
    }
    AudioObjectID device = kAudioObjectUnknown; unsigned matches = 0;
    OSStatus status = find_device(o.uid, &device, &matches), stop_status = 0, destroy_status = 0;
    Context *context = NULL; AudioDeviceIOProcID proc = NULL;
    bool started = false, complete = false, ready = false, quiescent = true;
    uint64_t started_ns = 0, ended_ns = 0;
    if (status == noErr && matches > 1) status = kAudioHardwareIllegalOperationError;
    if (!strcmp(o.mode, "discover")) {
        complete = status == noErr;
        if (!write_report(&o, device, matches, status, 0, 0, NULL, complete, false, 0, 0)) return 4;
        return status == noErr ? 0 : 1;
    }
    if (status == noErr && device == kAudioObjectUnknown) status = kAudioHardwareBadDeviceError;
    AudioStreamBasicDescription in_format = {0}, out_format = {0};
    if (status == noErr) status = stream_format(device, kAudioObjectPropertyScopeInput, &in_format);
    if (status == noErr) status = stream_format(device, kAudioObjectPropertyScopeOutput, &out_format);
    if (status == noErr && (!supported_format(&in_format) || !supported_format(&out_format))) status = kAudioDeviceUnsupportedFormatError;
    if (status == noErr) {
        context = calloc(1, sizeof *context);
        if (!context) status = kAudioHardwareUnspecifiedError;
        else {
            atomic_init(&context->accepting, false);
            atomic_init(&context->in_flight, 0);
            atomic_init(&context->fault, 0);
            atomic_init(&context->frames, 0);
            atomic_init(&context->event_count, 0);
        }
    }
    if (status == noErr) {
        context->recording = !strcmp(o.mode, "record"); context->amplitude = o.amplitude;
        context->left_hz = o.left_hz; context->right_hz = o.right_hz;
        context->target_frames = (uint64_t)llround(o.seconds * EXPECTED_RATE);
        context->events = calloc(MAX_EVENTS, sizeof(Event));
        if (context->recording) context->pcm = calloc((size_t)context->target_frames * 2u, sizeof(float));
        if (!context->events || (context->recording && !context->pcm)) status = kAudioHardwareUnspecifiedError;
        // Touch every owned page before real-time execution, including samples
        // that will eventually be overwritten; volatile stores cannot disappear.
        if (status == noErr) {
            volatile unsigned char *bytes = (volatile unsigned char *)context->events;
            for (size_t p = 0; p < MAX_EVENTS * sizeof(Event); p += 4096) bytes[p] = 0;
            bytes = (volatile unsigned char *)context->pcm;
            for (size_t p = 0; bytes && p < context->target_frames * 8u; p += 4096) bytes[p] = 0;
        }
    }
    if (status == noErr && o.diagnostics) status = diagnostic_start(context, device);
    if (status == noErr) status = AudioDeviceCreateIOProcID(device, io_proc, context, &proc);
    if (status == noErr) status = set_direction(device, proc, kAudioObjectPropertyScopeInput, context->recording);
    if (status == noErr) status = set_direction(device, proc, kAudioObjectPropertyScopeOutput, !context->recording);
    if (status == noErr) {
        atomic_store_explicit(&context->accepting, true, memory_order_seq_cst);
        started_ns = monotonic_ns();
        if (context->diagnostics) context->diagnostics->start_request = mach_absolute_time();
        status = AudioDeviceStart(device, proc);
        if (context->diagnostics) context->diagnostics->start_return = mach_absolute_time();
        started = status == noErr;
    }
    if (started) {
        uint64_t deadline = started_ns + (uint64_t)((o.seconds + 8.0) * 1e9), last_progress_ns = started_ns, previous_frames = 0;
        while (!atomic_load_explicit(&context->fault, memory_order_acquire)) {
            uint64_t frames = atomic_load_explicit(&context->frames, memory_order_acquire), now = monotonic_ns();
            if (frames != previous_frames) { last_progress_ns = now; previous_frames = frames; }
            if (!ready && frames) {
                ready = ready_file(o.ready, frames, now);
                if (!ready) { latch_fault(context, 5); break; }
            }
            if (frames >= context->target_frames) { complete = true; break; }
            if (now > deadline || now - last_progress_ns > 4000000000ULL) { latch_fault(context, 6); break; }
            sleep_ms(5);
        }
    }
    if (context) atomic_store_explicit(&context->accepting, false, memory_order_seq_cst);
    if (started) {
        if (context->diagnostics) context->diagnostics->stop_request = mach_absolute_time();
        stop_status = AudioDeviceStop(device, proc);
        if (context->diagnostics) context->diagnostics->stop_return = mach_absolute_time();
    }
    if (proc) {
        if (context->diagnostics) context->diagnostics->destroy_request = mach_absolute_time();
        destroy_status = AudioDeviceDestroyIOProcID(device, proc);
        if (context->diagnostics) context->diagnostics->destroy_return = mach_absolute_time();
    }
    if (context) {
        uint64_t deadline = monotonic_ns() + 2000000000ULL;
        // This count and the admission gate share one order: after closing and
        // observing zero, no future callback may modify the retained PCM/events.
        while (atomic_load_explicit(&context->in_flight, memory_order_seq_cst) && monotonic_ns() < deadline) sleep_ms(1);
        quiescent = atomic_load_explicit(&context->in_flight, memory_order_seq_cst) == 0;
        if (!quiescent) latch_fault(context, 7);
    }
    if (context && context->diagnostics) diagnostic_stop(context->diagnostics);
    ended_ns = monotonic_ns();
    bool saved = true;
    if (context && quiescent) {
        uint64_t frames = atomic_load(&context->frames), count = atomic_load(&context->event_count);
        if (context->recording && context->pcm) saved = write_wav(o.wav, context->pcm, frames) && saved;
        if (context->events) saved = write_events(o.events, context->events, count) && saved;
    }
    saved = write_report(&o, device, matches, status, stop_status, destroy_status, context, complete, ready, started_ns, ended_ns) && saved;
    if (o.diagnostics && context && context->diagnostics) saved = write_diagnostics(o.diagnostics, context, quiescent) && saved;
    bool success = saved && status == noErr && stop_status == noErr && destroy_status == noErr && complete && ready
        && context && atomic_load(&context->fault) == 0
        && (!o.diagnostics || diagnostic_complete(context->diagnostics));
    // Keep context alive until process exit if unregistering failed. No unsafe
    // freeing of storage still reachable from an OS callback.
    if (context && quiescent && destroy_status == noErr) { free(context->pcm); free(context->events); free(context); }
    return success ? 0 : 1;
}
