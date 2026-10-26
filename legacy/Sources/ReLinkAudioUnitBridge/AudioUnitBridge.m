#import "RaveLinkAudioUnitBridge.h"
#include <Block.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>

#define RL_AU_BLOCK_FRAMES 512
// The implementation is configured off callback and uses cached blocks only.
typedef struct {
    AURenderBlock render;
    AURenderPullInputBlock pull;
    AUScheduleParameterBlock schedule;
    uint32_t slot, frames;
    float *input[2];
    _Atomic bool bypassed;
    float wet;
    uint32_t latencyFrames, delayCursor;
    float *dryDelay;
} RLAUNode;
typedef struct {
    RLAUNode **nodes;
    uint32_t count;
    float *buffers; // Two stereo planar buffers, 512 frames per plane.
    double sampleTime;
    bool active;
    uint32_t latencyFrames, alignmentFrames, delayCursor;
    float *alignmentDelay;
} RLAUChain;
struct RLAUGraph {
    uint32_t sourceCount, maxFrames, maxLatencyFrames, alignmentLatencyFrames;
    uint64_t delayBytes, maxDelayBytes;
    bool finalized, configurationClosed;
    float smoothing;
    RLAUChain *chains;
    _Atomic int32_t error;
    _Atomic uint32_t errorSource, errorSlot;
};
// Force private writable pages during configuration, before real-time access.
// A 4 KiB stride covers both supported 4 KiB and 16 KiB macOS page sizes.
static float *audio_buffer_create(size_t samples) {
    float *buffer = calloc(samples, sizeof(float));
    if (buffer && samples) {
        volatile float *pages = buffer;
        for (size_t index = 0; index < samples; index += 1024) pages[index] = 0;
        pages[samples - 1] = 0;
    }
    return buffer;
}
static int32_t fail(RLAUGraph *g, uint32_t source, uint32_t slot, int32_t status) {
    atomic_store_explicit(&g->errorSource, source, memory_order_relaxed);
    atomic_store_explicit(&g->errorSlot, slot, memory_order_relaxed);
    atomic_store_explicit(&g->error, status, memory_order_release);
    return status;
}
RLAUGraph *rl_au_graph_create(uint32_t sources, uint32_t maxFrames, double rate) {
    if (!maxFrames || !isfinite(rate) || rate < 8000 || rate > 192000) return NULL;
    RLAUGraph *g = calloc(1, sizeof(*g));
    if (!g) return NULL;
    if (!atomic_is_lock_free(&g->error) || !atomic_is_lock_free(&g->errorSource)) { free(g); return NULL; }
    g->chains = calloc(sources ? sources : 1, sizeof(RLAUChain));
    if (!g->chains) { free(g); return NULL; }
    g->sourceCount = sources; g->maxFrames = maxFrames;
    g->maxLatencyFrames = (uint32_t)ceil(rate * 2);
    g->maxDelayBytes = 128ull * 1024 * 1024;
    g->smoothing = (float)exp(-1.0 / (.005 * rate));
    return g;
}
void rl_au_graph_destroy(RLAUGraph *g) {
    if (!g) return;
    for (uint32_t s = 0; s < g->sourceCount; s++) {
        RLAUChain *chain = &g->chains[s];
        for (uint32_t n = 0; n < chain->count; n++) {
            RLAUNode *node = chain->nodes[n];
            if (node->render) Block_release(node->render);
            if (node->pull) Block_release(node->pull);
            if (node->schedule) Block_release(node->schedule);
            free(node->dryDelay); free(node);
        }
        free(chain->nodes); free(chain->buffers); free(chain->alignmentDelay);
    }
    free(g->chains); free(g);
}
bool rl_au_graph_enable_source(RLAUGraph *g, uint32_t source) {
    if (!g || g->configurationClosed || source >= g->sourceCount) return false;
    g->chains[source].active = true; return true;
}
bool rl_au_graph_set_latency_limits(RLAUGraph *g, uint32_t maxFrames, uint64_t maxBytes) {
    if (!g || g->configurationClosed || g->delayBytes || maxFrames > g->maxLatencyFrames || maxBytes > g->maxDelayBytes) return false;
    g->maxLatencyFrames = maxFrames; g->maxDelayBytes = maxBytes; return true;
}
bool rl_au_graph_append(RLAUGraph *g, uint32_t source, uint32_t slot, AURenderBlock block, AUScheduleParameterBlock schedule, bool bypassed, uint32_t latencyFrames) {
    if (!g || g->configurationClosed || source >= g->sourceCount || !block) return false;
    RLAUChain *chain = &g->chains[source];
    uint64_t bytes = (uint64_t)latencyFrames * 2 * sizeof(float);
    if (latencyFrames > g->maxLatencyFrames - chain->latencyFrames || bytes > g->maxDelayBytes - g->delayBytes) return false;
    if (chain->count == UINT32_MAX || (size_t)chain->count + 1 > SIZE_MAX / sizeof(RLAUNode *)) return false;
    for (uint32_t n = 0; n < chain->count; n++) if (chain->nodes[n]->slot == slot) return false;
    if (!chain->buffers) { chain->buffers = audio_buffer_create(4 * RL_AU_BLOCK_FRAMES); if (!chain->buffers) return false; }
    RLAUNode **grown = realloc(chain->nodes, ((size_t)chain->count + 1) * sizeof(*grown));
    if (!grown) return false;
    chain->nodes = grown;
    RLAUNode *node = calloc(1, sizeof(*node));
    if (!node) return false;
    if (!atomic_is_lock_free(&node->bypassed)) { free(node); return false; }
    node->slot = slot; node->wet = bypassed ? 0 : 1; node->latencyFrames = latencyFrames;
    if (latencyFrames) {
        node->dryDelay = audio_buffer_create((size_t)latencyFrames * 2);
        if (!node->dryDelay) { free(node); return false; }
    }
    atomic_init(&node->bypassed, bypassed);
    node->render = Block_copy(block);
    node->schedule = schedule ? Block_copy(schedule) : NULL;
    node->pull = Block_copy(^AUAudioUnitStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *time, AUAudioFrameCount frames, NSInteger bus, AudioBufferList *data) {
        if (bus != 0 || frames != node->frames || !data || data->mNumberBuffers != 2) return kAudioUnitErr_FormatNotSupported;
        for (uint32_t c = 0; c < 2; c++) {
            data->mBuffers[c].mNumberChannels = 1;
            data->mBuffers[c].mDataByteSize = frames * sizeof(float);
            data->mBuffers[c].mData = node->input[c];
        }
        return noErr;
    });
    chain->nodes[chain->count++] = node;
    chain->active = true; chain->latencyFrames += latencyFrames; g->delayBytes += bytes;
    return true;
}
bool rl_au_graph_finalize(RLAUGraph *g) {
    if (!g || g->configurationClosed) return false;
    g->configurationClosed = true;
    uint32_t longest = 0;
    for (uint32_t s = 0; s < g->sourceCount; s++) if (g->chains[s].active && g->chains[s].latencyFrames > longest) longest = g->chains[s].latencyFrames;
    uint64_t extraBytes = 0;
    for (uint32_t s = 0; s < g->sourceCount; s++) {
        RLAUChain *chain = &g->chains[s]; if (!chain->active) continue;
        uint64_t bytes = (uint64_t)(longest - chain->latencyFrames) * 2 * sizeof(float);
        if (bytes > g->maxDelayBytes - g->delayBytes - extraBytes) return false;
        extraBytes += bytes;
    }
    for (uint32_t s = 0; s < g->sourceCount; s++) {
        RLAUChain *chain = &g->chains[s]; if (!chain->active) continue;
        chain->alignmentFrames = longest - chain->latencyFrames;
        if (chain->alignmentFrames) {
            chain->alignmentDelay = audio_buffer_create((size_t)chain->alignmentFrames * 2);
            if (!chain->alignmentDelay) return false; // Owner discards the partial graph.
        }
    }
    g->delayBytes += extraBytes; g->alignmentLatencyFrames = longest; g->finalized = true; return true;
}
uint32_t rl_au_graph_latency_frames(RLAUGraph *g) { return g ? g->alignmentLatencyFrames : 0; }
uint64_t rl_au_graph_delay_bytes(RLAUGraph *g) { return g ? g->delayBytes : 0; }
void rl_au_graph_invalidate(RLAUGraph *g, uint32_t source, uint32_t slot, int32_t status) {
    if (g && status) fail(g, source, slot, status);
}
static RLAUNode *find_node(RLAUGraph *g, uint32_t source, uint32_t slot) {
    if (!g || source >= g->sourceCount) return NULL;
    RLAUChain *chain = &g->chains[source];
    for (uint32_t n = 0; n < chain->count; n++) if (chain->nodes[n]->slot == slot) return chain->nodes[n];
    return NULL;
}
bool rl_au_graph_set_bypass(RLAUGraph *g, uint32_t source, uint32_t slot, bool bypassed) {
    RLAUNode *node = find_node(g, source, slot); if (!node) return false;
    atomic_store_explicit(&node->bypassed, bypassed, memory_order_relaxed); return true;
}
bool rl_au_graph_set_parameter(RLAUGraph *g, uint32_t source, uint32_t slot, uint64_t address, float value, uint32_t rampFrames) {
    RLAUNode *node = find_node(g, source, slot); if (!node || !node->schedule || !isfinite(value)) return false;
    node->schedule(AUEventSampleTimeImmediate, rampFrames, address, value); return true;
}
int32_t rl_au_graph_error(RLAUGraph *g, uint32_t *source, uint32_t *slot) {
    if (!g) return 0;
    int32_t status = atomic_load_explicit(&g->error, memory_order_acquire);
    if (source) *source = atomic_load_explicit(&g->errorSource, memory_order_relaxed);
    if (slot) *slot = atomic_load_explicit(&g->errorSlot, memory_order_relaxed);
    return status;
}
int32_t rl_au_graph_process(void *context, uint32_t source, float *stereo, uint32_t frames) {
    RLAUGraph *g = context;
    if (!g) return 0;
    int32_t error = atomic_load_explicit(&g->error, memory_order_acquire);
    if (error) return error;
    if (!g->finalized || source >= g->sourceCount || !stereo || frames > g->maxFrames) return fail(g, source, UINT32_MAX, -70001);
    RLAUChain *chain = &g->chains[source];
    if (!chain->active) return 0;
    if (chain->count) for (uint32_t offset = 0; offset < frames;) {
        uint32_t count = frames - offset; if (count > RL_AU_BLOCK_FRAMES) count = RL_AU_BLOCK_FRAMES;
        float *left = chain->buffers, *right = left + RL_AU_BLOCK_FRAMES;
        float *nextLeft = right + RL_AU_BLOCK_FRAMES, *nextRight = nextLeft + RL_AU_BLOCK_FRAMES;
        for (uint32_t f = 0; f < count; f++) { left[f] = stereo[(offset + f) * 2]; right[f] = stereo[(offset + f) * 2 + 1]; }
        for (uint32_t n = 0; n < chain->count; n++) {
            RLAUNode *node = chain->nodes[n]; node->input[0] = left; node->input[1] = right; node->frames = count;
            struct { UInt32 mNumberBuffers; AudioBuffer mBuffers[2]; } output = { 2, { { 1, count * sizeof(float), nextLeft }, { 1, count * sizeof(float), nextRight } } };
            AudioTimeStamp timestamp = {0}; timestamp.mSampleTime = chain->sampleTime; timestamp.mFlags = kAudioTimeStampSampleTimeValid;
            AudioUnitRenderActionFlags flags = 0;
            OSStatus status = node->render(&flags, &timestamp, count, 0, (AudioBufferList *)&output, node->pull);
            if (status) return fail(g, source, node->slot, status);
            if (output.mNumberBuffers != 2) return fail(g, source, node->slot, -70002);
            for (uint32_t c = 0; c < 2; c++) {
                AudioBuffer *buffer = &output.mBuffers[c];
                if (buffer->mNumberChannels != 1 || buffer->mDataByteSize < count * sizeof(float) || !buffer->mData) return fail(g, source, node->slot, -70002);
            }
            float *outLeft = output.mBuffers[0].mData, *outRight = output.mBuffers[1].mData;
            float target = atomic_load_explicit(&node->bypassed, memory_order_relaxed) ? 0 : 1;
            for (uint32_t f = 0; f < count; f++) {
                float l = (flags & kAudioUnitRenderAction_OutputIsSilence) ? 0 : outLeft[f];
                float r = (flags & kAudioUnitRenderAction_OutputIsSilence) ? 0 : outRight[f];
                if (!isfinite(l) || !isfinite(r) || fabsf(l) > 1000000 || fabsf(r) > 1000000) return fail(g, source, node->slot, -70003);
                node->wet = g->smoothing * node->wet + (1 - g->smoothing) * target;
                // Delay dry by the unit's reported latency before a host bypass blend.
                // Reads precede writes in case an effect returned an input pointer.
                float dryLeft = left[f], dryRight = right[f];
                if (node->latencyFrames) {
                    uint32_t index = node->delayCursor * 2;
                    float delayedLeft = node->dryDelay[index], delayedRight = node->dryDelay[index + 1];
                    node->dryDelay[index] = dryLeft; node->dryDelay[index + 1] = dryRight;
                    dryLeft = delayedLeft; dryRight = delayedRight;
                    if (++node->delayCursor == node->latencyFrames) node->delayCursor = 0;
                }
                nextLeft[f] = dryLeft * (1 - node->wet) + l * node->wet;
                nextRight[f] = dryRight * (1 - node->wet) + r * node->wet;
            }
            float *swap = left; left = nextLeft; nextLeft = swap;
            swap = right; right = nextRight; nextRight = swap;
        }
        for (uint32_t f = 0; f < count; f++) { stereo[(offset + f) * 2] = left[f]; stereo[(offset + f) * 2 + 1] = right[f]; }
        chain->sampleTime += count; offset += count;
    }
    if (chain->alignmentFrames) for (uint32_t frame = 0; frame < frames; frame++) {
        uint32_t index = chain->delayCursor * 2;
        float left = stereo[frame * 2], right = stereo[frame * 2 + 1];
        stereo[frame * 2] = chain->alignmentDelay[index]; stereo[frame * 2 + 1] = chain->alignmentDelay[index + 1];
        chain->alignmentDelay[index] = left; chain->alignmentDelay[index + 1] = right;
        if (++chain->delayCursor == chain->alignmentFrames) chain->delayCursor = 0;
    }
    return 0;
}
