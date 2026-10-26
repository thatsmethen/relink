#ifndef RAVELINK_AUDIO_UNIT_BRIDGE_H
#define RAVELINK_AUDIO_UNIT_BRIDGE_H
#import <AudioToolbox/AUAudioUnit.h>
#include <stdbool.h>
#include <stdint.h>

typedef struct RLAUGraph RLAUGraph;
RLAUGraph * _Nullable rl_au_graph_create(uint32_t sourceCount, uint32_t maxFrames, double sampleRate);
void rl_au_graph_destroy(RLAUGraph * _Nullable graph);
// Configuration is allowed only before the render callback begins.
bool rl_au_graph_append(RLAUGraph * _Nonnull graph, uint32_t sourceIndex, uint32_t slotIndex, AURenderBlock _Nonnull block, AUScheduleParameterBlock _Nullable schedule, bool bypassed, uint32_t latencyFrames);
// All leader sources, including empty racks, participate in output-clock alignment.
bool rl_au_graph_enable_source(RLAUGraph * _Nonnull graph, uint32_t sourceIndex);
// Limits can only tighten the default 2-second rack / 128 MiB delay budget.
bool rl_au_graph_set_latency_limits(RLAUGraph * _Nonnull graph, uint32_t maxLatencyFrames, uint64_t maxDelayBytes);
bool rl_au_graph_finalize(RLAUGraph * _Nonnull graph);
uint32_t rl_au_graph_latency_frames(RLAUGraph * _Nonnull graph);
// Configured delay storage; read only after successful finalization.
uint64_t rl_au_graph_delay_bytes(RLAUGraph * _Nonnull graph);
void rl_au_graph_invalidate(RLAUGraph * _Nonnull graph, uint32_t sourceIndex, uint32_t slotIndex, int32_t status);
bool rl_au_graph_set_bypass(RLAUGraph * _Nonnull graph, uint32_t sourceIndex, uint32_t slotIndex, bool bypassed);
bool rl_au_graph_set_parameter(RLAUGraph * _Nonnull graph, uint32_t sourceIndex, uint32_t slotIndex, uint64_t address, float value, uint32_t rampFrames);
int32_t rl_au_graph_process(void * _Nullable context, uint32_t sourceIndex, float * _Nullable stereo, uint32_t frames);
int32_t rl_au_graph_error(RLAUGraph * _Nonnull graph, uint32_t * _Nullable sourceIndex, uint32_t * _Nullable slotIndex);
#endif
