/* Optional observer diagnostics; no driver code or audio-device mutations. */
#ifndef RAVELINK_PROBE_DIAGNOSTICS_H
#define RAVELINK_PROBE_DIAGNOSTICS_H
#define DIAGNOSTIC_NOTIFICATION_CAPACITY 1024u
#define DIAGNOSTIC_ADDRESSES_PER_CALLBACK 16u
#define DIAGNOSTIC_LISTENERS 8u
_Static_assert(ATOMIC_BOOL_LOCK_FREE == 2 && ATOMIC_INT_LOCK_FREE == 2
    && ATOMIC_LONG_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2,
    "Observer callback diagnostics require lock-free native atomics");

typedef struct {
    uint64_t entry_host_time, exit_host_time, current_host_time;
    double current_sample_time;
    uint32_t current_flags;
    bool current_present;
} TimingEvent;
typedef struct {
    atomic_uint committed;
    uint64_t host_time;
    AudioObjectID object;
    AudioObjectPropertyAddress address;
} NotificationEvent;
typedef struct {
    AudioObjectID object;
    AudioObjectPropertyAddress address;
    bool available, added;
    OSStatus add_status, remove_status;
} DiagnosticListener;
typedef struct {
    TimingEvent *timings;
    NotificationEvent notifications[DIAGNOSTIC_NOTIFICATION_CAPACITY];
    DiagnosticListener listeners[DIAGNOSTIC_LISTENERS];
    unsigned listener_count;
    atomic_bool accepting;
    atomic_uint in_flight;
    atomic_uint_fast64_t notification_count, dropped;
    uint64_t start_request, start_return, stop_request, stop_return, destroy_request, destroy_return;
    bool listeners_stopped, quiescent;
} Diagnostics;

static inline void diagnostic_initialize(Diagnostics *diagnostics) {
    atomic_init(&diagnostics->accepting, true);
    atomic_init(&diagnostics->in_flight, 0);
    atomic_init(&diagnostics->notification_count, 0);
    atomic_init(&diagnostics->dropped, 0);
    for (unsigned index = 0; index < DIAGNOSTIC_NOTIFICATION_CAPACITY; index++)
        atomic_init(&diagnostics->notifications[index].committed, 0);
}
static inline void diagnostic_notify(Diagnostics *diagnostics, AudioObjectID object,
                                     UInt32 count, const AudioObjectPropertyAddress *addresses, uint64_t tick) {
    // A single order across admission and in-flight accounting guarantees that
    // close-then-observe-zero cannot miss a publisher that saw the open gate.
    atomic_fetch_add_explicit(&diagnostics->in_flight, 1, memory_order_seq_cst);
    if (!atomic_load_explicit(&diagnostics->accepting, memory_order_seq_cst)) goto done;
    UInt32 bounded = count < DIAGNOSTIC_ADDRESSES_PER_CALLBACK ? count : DIAGNOSTIC_ADDRESSES_PER_CALLBACK;
    if (!addresses) bounded = 0;
    if (count > bounded) atomic_fetch_add_explicit(&diagnostics->dropped, count - bounded, memory_order_relaxed);
    for (UInt32 index = 0; index < bounded; index++) {
        uint64_t slot = atomic_fetch_add_explicit(&diagnostics->notification_count, 1, memory_order_relaxed);
        if (slot >= DIAGNOSTIC_NOTIFICATION_CAPACITY) {
            atomic_fetch_add_explicit(&diagnostics->dropped, 1, memory_order_relaxed);
            continue;
        }
        NotificationEvent *event = &diagnostics->notifications[slot];
        event->host_time = tick;
        event->object = object;
        event->address = addresses[index];
        atomic_store_explicit(&event->committed, (unsigned)slot + 1, memory_order_release);
    }
done:
    atomic_fetch_sub_explicit(&diagnostics->in_flight, 1, memory_order_seq_cst);
}
#endif
