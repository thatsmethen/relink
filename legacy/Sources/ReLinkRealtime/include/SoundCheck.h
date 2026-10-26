#ifndef RAVELINK_SOUND_CHECK_H
#define RAVELINK_SOUND_CHECK_H
#include <stdint.h>
#include <stdbool.h>

// One control-thread command writer and exactly one render-thread consumer.
// Creation/destruction require stopped callbacks. No allocation, locks, file IO,
// or object dispatch occur in process(). The clip remains only in this memory.
typedef struct RLSoundCheck RLSoundCheck;
RLSoundCheck *rl_sound_check_create(double sampleRate, double maximumSeconds);
void rl_sound_check_destroy(RLSoundCheck *check);
// 1 record (replace), 2 stop, 3 play (loop from beginning), 4 clear.
// False means invalid command or full bounded mailbox; caller must report it.
bool rl_sound_check_command(RLSoundCheck *check, uint32_t command);
// Call on the dry, summed stereo input BEFORE any DSP/effects.
void rl_sound_check_process(RLSoundCheck *check, float *stereo, uint32_t frames);
// Call AFTER all DSP/effects to suppress effect tails outside playback.
void rl_sound_check_silence_if_stopped(RLSoundCheck *check, float *stereo, uint32_t frames);
// Modes: 0 empty, 1 recording, 2 ready, 3 playing.
uint32_t rl_sound_check_mode(RLSoundCheck *check);
uint64_t rl_sound_check_frames(RLSoundCheck *check);
// Coherent telemetry pair; use this instead of two accessors for validation.
void rl_sound_check_snapshot(RLSoundCheck *check, uint32_t *mode, uint64_t *frames);
double rl_sound_check_rate(RLSoundCheck *check);
float rl_sound_check_take_peak(RLSoundCheck *check);
#endif
