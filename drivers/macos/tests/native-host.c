/* Original ABI test host. It loads only the supplied local library, never the HAL. */
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CFPlugInCOM.h>
#include <assert.h>
#include <dlfcn.h>
#include <math.h>
#include <stdio.h>
#include <stddef.h>
#include <string.h>

_Static_assert(sizeof(AudioServerPlugInDriverInterface) == 184, "driver ABI");
_Static_assert(sizeof(AudioServerPlugInHostInterface) == 40, "host ABI");
_Static_assert(sizeof(AudioServerPlugInClientInfo) == 24, "client ABI");
_Static_assert(sizeof(AudioServerPlugInIOCycleInfo) == 224, "cycle ABI");
_Static_assert(sizeof(AudioTimeStamp) == 64, "timestamp ABI");
_Static_assert(sizeof(AudioStreamBasicDescription) == 40, "format ABI");
_Static_assert(sizeof(AudioStreamRangedDescription) == 56, "format range ABI");
_Static_assert(offsetof(AudioServerPlugInDriverInterface, DoIOOperation) == 168, "vtable order");
_Static_assert(offsetof(AudioServerPlugInIOCycleInfo, mInputTime) == 80, "input timestamp");
_Static_assert(offsetof(AudioServerPlugInIOCycleInfo, mOutputTime) == 144, "output timestamp");

static unsigned notifications[2];
static OSStatus changed(AudioServerPlugInHostRef host, AudioObjectID object, UInt32 count, const AudioObjectPropertyAddress *addresses) {
    (void)host;
    assert((object == 100 || object == 200) && count == 1 && addresses[0].mSelector == kAudioDevicePropertyDeviceIsRunning);
    notifications[object == 200]++;
    return 0;
}
static OSStatus copy_storage(AudioServerPlugInHostRef host, CFStringRef key, CFPropertyListRef *data) { (void)host; (void)key; (void)data; return kAudioHardwareUnsupportedOperationError; }
static OSStatus write_storage(AudioServerPlugInHostRef host, CFStringRef key, CFPropertyListRef data) { (void)host; (void)key; (void)data; return kAudioHardwareUnsupportedOperationError; }
static OSStatus delete_storage(AudioServerPlugInHostRef host, CFStringRef key) { (void)host; (void)key; return kAudioHardwareUnsupportedOperationError; }
static OSStatus request_change(AudioServerPlugInHostRef host, AudioObjectID device, UInt64 action, void *info) { (void)host; (void)device; (void)action; (void)info; return kAudioHardwareUnsupportedOperationError; }

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: native-host /absolute/path/to/ReLinkHAL\n"); return 2; }
    void *library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!library) { fprintf(stderr, "%s\n", dlerror()); return 1; }
    void *(*factory)(CFAllocatorRef, CFUUIDRef) = dlsym(library, "RaveLinkHALFactory");
    assert(factory != NULL);
    assert(factory(NULL, NULL) == NULL);
    assert(factory(NULL, IUnknownUUID) == NULL);
    AudioServerPlugInDriverRef driver = factory(NULL, kAudioServerPlugInTypeUUID);
    assert(driver && *driver);
    void *queried = NULL;
    assert((*driver)->QueryInterface(driver, CFUUIDGetUUIDBytes(kAudioServerPlugInDriverInterfaceUUID), &queried) == S_OK);
    assert(queried == driver);
    assert((*driver)->QueryInterface(driver, CFUUIDGetUUIDBytes(kAudioServerPlugInTypeUUID), &queried) == E_NOINTERFACE);
    assert(queried == NULL);
    AudioServerPlugInHostInterface host = { changed, copy_storage, write_storage, delete_storage, request_change };
    assert((*driver)->Initialize(driver, &host) == 0);
    AudioObjectPropertyAddress address = { kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    AudioObjectID devices[2] = {0};
    UInt32 size = 0;
    assert((*driver)->GetPropertyDataSize(driver, kAudioObjectPlugInObject, 0, &address, 0, NULL, &size) == 0 && size == sizeof devices);
    assert((*driver)->GetPropertyData(driver, kAudioObjectPlugInObject, 0, &address, 0, NULL, sizeof devices, &size, devices) == 0 && devices[0] == 100 && devices[1] == 200);
    const AudioObjectID device = devices[0], second = devices[1];
    const CFStringRef expected_uids[] = { CFSTR("au.textual.ravelink.cable.1"), CFSTR("au.textual.ravelink.cable.2") };
    const CFStringRef expected_names[] = { CFSTR("ReLink"), CFSTR("ReLink 2") };
    for (unsigned index = 0; index < 2; index++) {
        address.mSelector = kAudioDevicePropertyDeviceUID;
        CFStringRef uid = NULL;
        assert((*driver)->GetPropertyData(driver, devices[index], 0, &address, 0, NULL, sizeof uid, &size, &uid) == 0);
        assert(CFEqual(uid, expected_uids[index]));
        address.mSelector = kAudioPlugInPropertyTranslateUIDToDevice;
        AudioObjectID translated = 0;
        assert((*driver)->GetPropertyData(driver, 1, 0, &address, sizeof uid, &uid, sizeof translated, &size, &translated) == 0 && translated == devices[index]);
        CFRelease(uid);
        address.mSelector = kAudioObjectPropertyName;
        CFStringRef name = NULL;
        assert((*driver)->GetPropertyData(driver, devices[index], 0, &address, 0, NULL, sizeof name, &size, &name) == 0);
        assert(CFEqual(name, expected_names[index]));
        CFRelease(name);
        address.mSelector = kAudioDevicePropertyStreams;
        AudioObjectID streams[2] = {0};
        assert((*driver)->GetPropertyData(driver, devices[index], 0, &address, 0, NULL, sizeof streams, &size, streams) == 0);
        assert(streams[0] == devices[index] + 1 && streams[1] == devices[index] + 2);
    }
    address.mSelector = kAudioStreamPropertyPhysicalFormat;
    AudioStreamBasicDescription format;
    const AudioObjectID streams[] = {101, 102, 201, 202};
    for (unsigned index = 0; index < 4; index++) {
        assert((*driver)->GetPropertyData(driver, streams[index], 0, &address, 0, NULL, sizeof format, &size, &format) == 0);
        assert(format.mSampleRate == 48000 && format.mChannelsPerFrame == 2 && format.mFormatID == kAudioFormatLinearPCM);
        assert(format.mFormatFlags == kAudioFormatFlagsNativeFloatPacked && format.mBytesPerFrame == 8);
    }
    format.mSampleRate = 44100;
    assert((*driver)->SetPropertyData(driver, 101, 0, &address, 0, NULL, sizeof format, &format) == kAudioDeviceUnsupportedFormatError);
    AudioServerPlugInClientInfo client = { 17, 0, true, NULL };
    assert((*driver)->AddDeviceClient(driver, device, &client) == 0);
    assert((*driver)->StartIO(driver, device, client.mClientID) == 0);
    /* Client IDs are scoped to their device, even when both are running. */
    assert((*driver)->AddDeviceClient(driver, second, &client) == 0);
    assert((*driver)->StartIO(driver, second, client.mClientID) == 0);
    Boolean will = false, in_place = false;
    assert((*driver)->WillDoIOOperation(driver, device, 17, kAudioServerPlugInIOOperationWriteMix, &will, &in_place) == 0 && will && in_place);
    AudioServerPlugInIOCycleInfo cycle = {0};
    cycle.mOutputTime.mSampleTime = 1000;
    cycle.mInputTime.mSampleTime = 1512;
    cycle.mOutputTime.mFlags = kAudioTimeStampSampleTimeValid;
    cycle.mInputTime.mFlags = kAudioTimeStampSampleTimeValid;
    Float32 source[] = { .25f, -.25f, .5f, -.5f, .75f, -.75f, 1, -1 };
    Float32 second_source[] = { -.875f, .875f, -.625f, .625f, -.375f, .375f, -.125f, .125f };
    Float32 captured[8] = {0};
    assert((*driver)->BeginIOOperation(driver, device, 17, kAudioServerPlugInIOOperationWriteMix, 4, &cycle) == 0);
    assert((*driver)->DoIOOperation(driver, device, 102, 17, kAudioServerPlugInIOOperationWriteMix, 4, &cycle, source, NULL) == 0);
    assert((*driver)->DoIOOperation(driver, second, 202, 17, kAudioServerPlugInIOOperationWriteMix, 4, &cycle, second_source, NULL) == 0);
    assert((*driver)->DoIOOperation(driver, device, 101, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == 0);
    assert(memcmp(source, captured, sizeof source) == 0);
    assert((*driver)->DoIOOperation(driver, second, 201, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == 0);
    assert(memcmp(second_source, captured, sizeof second_source) == 0);
    assert((*driver)->DoIOOperation(driver, device, 201, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == kAudioHardwareBadStreamError);
    AudioServerPlugInClientInfo extra = { 18, 0, true, NULL };
    assert((*driver)->AddDeviceClient(driver, second, &extra) == 0);
    assert((*driver)->StartIO(driver, device, 18) == kAudioHardwareIllegalOperationError);
    assert((*driver)->StartIO(driver, second, 18) == 0);
    assert((*driver)->DoIOOperation(driver, second, 201, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == 0);
    assert(memcmp(second_source, captured, sizeof second_source) == 0);
    assert((*driver)->StopIO(driver, second, 18) == 0);
    assert((*driver)->RemoveDeviceClient(driver, second, &extra) == 0);
    /* Restarting the second cable must leave the first cable's data intact. */
    assert((*driver)->StopIO(driver, second, 17) == 0);
    assert((*driver)->StartIO(driver, second, 17) == 0);
    assert((*driver)->DoIOOperation(driver, second, 201, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == 0);
    for (unsigned index = 0; index < 8; index++) { assert(captured[index] == 0); }
    assert((*driver)->DoIOOperation(driver, device, 101, 17, kAudioServerPlugInIOOperationReadInput, 4, &cycle, captured, NULL) == 0);
    assert(memcmp(source, captured, sizeof source) == 0);
    assert((*driver)->EndIOOperation(driver, device, 17, kAudioServerPlugInIOOperationWriteMix, 4, &cycle) == 0);
    /* Exercise wraparound, partial blocks, maximum supported blocks, missing
       data, and the stopped-to-started clear. These are synthetic timings. */
    const UInt32 frame_sizes[] = { 1, 127, 512, 4096, 8192 };
    Float32 block_source[8192 * 2];
    Float32 block_capture[8192 * 2];
    Float32 block_second[8192 * 2];
    for (unsigned pass = 0; pass < sizeof frame_sizes / sizeof frame_sizes[0]; pass++) {
        UInt32 frames = frame_sizes[pass];
        cycle.mOutputTime.mSampleTime = 16000 + pass * 32768;
        cycle.mInputTime.mSampleTime = cycle.mOutputTime.mSampleTime + 512;
        for (UInt32 i = 0; i < frames * 2; i++) { block_source[i] = ((Float32)(i % 511) - 255) / 256; block_second[i] = ((Float32)(i % 127) + 1) / 256; block_capture[i] = 99; }
        assert((*driver)->DoIOOperation(driver, device, 101, 17, kAudioServerPlugInIOOperationReadInput, frames, &cycle, block_capture, NULL) == 0);
        for (UInt32 i = 0; i < frames * 2; i++) { assert(block_capture[i] == 0); }
        assert((*driver)->DoIOOperation(driver, device, 102, 17, kAudioServerPlugInIOOperationWriteMix, frames, &cycle, block_source, NULL) == 0);
        assert((*driver)->DoIOOperation(driver, device, 101, 17, kAudioServerPlugInIOOperationReadInput, frames, &cycle, block_capture, NULL) == 0);
        assert(memcmp(block_source, block_capture, frames * 2 * sizeof(Float32)) == 0);
        assert((*driver)->DoIOOperation(driver, second, 202, 17, kAudioServerPlugInIOOperationWriteMix, frames, &cycle, block_second, NULL) == 0);
        assert((*driver)->StopIO(driver, device, 17) == 0);
        assert((*driver)->StartIO(driver, device, 17) == 0);
        assert((*driver)->DoIOOperation(driver, device, 101, 17, kAudioServerPlugInIOOperationReadInput, frames, &cycle, block_capture, NULL) == 0);
        for (UInt32 i = 0; i < frames * 2; i++) { assert(block_capture[i] == 0); }
        assert((*driver)->DoIOOperation(driver, second, 201, 17, kAudioServerPlugInIOOperationReadInput, frames, &cycle, block_capture, NULL) == 0);
        assert(memcmp(block_second, block_capture, frames * 2 * sizeof(Float32)) == 0);
    }
    Float64 sample = -1;
    UInt64 ticks = 0, seed = 0;
    assert((*driver)->GetZeroTimeStamp(driver, device, 17, &sample, &ticks, &seed) == 0 && sample >= 0 && ticks > 0 && seed == 1);
    assert((*driver)->StopIO(driver, device, 17) == 0);
    assert((*driver)->RemoveDeviceClient(driver, device, &client) == 0);
    assert((*driver)->GetZeroTimeStamp(driver, second, 17, &sample, &ticks, &seed) == 0 && sample >= 0 && ticks > 0 && seed == 1);
    assert((*driver)->StopIO(driver, second, 17) == 0);
    assert((*driver)->RemoveDeviceClient(driver, second, &client) == 0);
    assert(notifications[0] == 12 && notifications[1] == 4);
    assert((*driver)->Release(driver) == 1);
    assert((*driver)->Release(driver) == 0);
    dlclose(library);
    puts("PASS: Apple SDK ABI, factory, COM, two-cable enumeration, independent clients/rings/restarts, timestamps, and stereo loopback (local host only)");
    return 0;
}
