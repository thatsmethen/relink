// Original offline controls: invoke only parser and pure callback with owned
// synthetic buffers. The included observer's hardware main is never called.
#define main hardware_observer_main_unused
#include "system-probe.c"
#undef main
#include <assert.h>
#include <stddef.h>
#include <sys/proc_info.h>

static void check_parser(void) {
    Options value;
    char *defaults[] = {"probe", "play", "--uid", "owned", "--report", "a", "--events", "b", "--ready", "c"};
    assert(options(10, defaults, &value));
    assert(value.amplitude == .125 && value.left_amplitude == .125 && value.right_amplitude == .125);
    char *shared[] = {"probe", "play", "--uid", "owned", "--report", "a", "--events", "b", "--ready", "c", "--amplitude", ".25"};
    assert(options(12, shared, &value));
    assert(value.left_amplitude == .25 && value.right_amplitude == .25);
    char *override[] = {"probe", "play", "--uid", "owned", "--report", "a", "--events", "b", "--ready", "c", "--left-amplitude", "0", "--amplitude", ".25"};
    assert(options(14, override, &value));
    assert(value.left_amplitude == 0 && value.right_amplitude == .25);
    char *reverse[] = {"probe", "play", "--uid", "owned", "--report", "a", "--events", "b", "--ready", "c", "--amplitude", ".25", "--left-amplitude", "0"};
    assert(options(14, reverse, &value));
    assert(value.left_amplitude == 0 && value.right_amplitude == .25);
    char *invalid[] = {"probe", "play", "--uid", "owned", "--report", "a", "--events", "b", "--ready", "c", "--right-amplitude", "nan"};
    assert(!options(12, invalid, &value));
    invalid[11] = ""; assert(!options(12, invalid, &value));
    invalid[11] = "-0.01"; assert(!options(12, invalid, &value));
    invalid[11] = "0.50001"; assert(!options(12, invalid, &value));
    invalid[11] = "0.5"; assert(options(12, invalid, &value));
}
static void check_pcm(double left, double right) {
    float samples[128 * 2];
    Event event = {0};
    Context context = {.left_amplitude=left, .right_amplitude=right, .left_hz=440, .right_hz=997,
                       .target_frames=128, .events=&event};
    atomic_init(&context.accepting, true); atomic_init(&context.in_flight, 0);
    atomic_init(&context.fault, 0); atomic_init(&context.frames, 0); atomic_init(&context.event_count, 0);
    AudioBufferList output = {.mNumberBuffers=1, .mBuffers={{.mNumberChannels=2, .mDataByteSize=sizeof samples, .mData=samples}}};
    AudioTimeStamp timestamp = {.mSampleTime=1234, .mHostTime=100000,
                               .mFlags=kAudioTimeStampSampleTimeValid | kAudioTimeStampHostTimeValid};
    assert(io_proc(0, &timestamp, NULL, NULL, &output, &timestamp, &context) == noErr);
    assert(atomic_load(&context.frames) == 128 && atomic_load(&context.fault) == 0);
    for (size_t frame=0; frame<128; ++frame) {
        double seconds = (1234 + frame) / 48000.0;
        assert(samples[frame*2] == (float)(left*sin(2*PI*440*seconds)));
        assert(samples[frame*2+1] == (float)(right*sin(2*PI*997*seconds)));
    }
}
int main(void) {
    _Static_assert(sizeof(struct proc_bsdinfo) == 136, "owned PID parser size");
    _Static_assert(offsetof(struct proc_bsdinfo, pbi_pid) == 12, "owned PID offset");
    _Static_assert(offsetof(struct proc_bsdinfo, pbi_start_tvsec) == 120, "owned PID birth offset");
    check_parser(); check_pcm(.125,.125); check_pcm(.125,0); check_pcm(0,.125);
    puts("offline generator/parser/owned-PID SDK layout controls passed; no audio API called");
    return 0;
}
