# Elastic clock controller qualification

The deterministic schedule instrument at
`research/evidence/elastic-clock-baseline-probe.c` exercises the actual FIFO and
resampler from source checkpoint `784e38f30a7b51f2fc8d40d58413780ae11e9944`.
Exact source hashes, compiler, reproduction command, schedule, and limitations
are recorded in `elastic-clock-baseline.json`; original results are in
`elastic-clock-baseline.csv`.

Each case simulates 180 seconds with a 48 kHz nominal output and 256-frame input
and output callbacks. The input clock runs at a fixed offset from nominal. No
physical device, process tap, microphone, callback contention, or real clock is
involved. Initial FIFO priming is reported separately from later underflows.

| Input clock offset | Late wholly silent blocks | Underflow frames | First late silent block |
| --- | ---: | ---: | ---: |
| -500 ppm | 8 | 1,581 | 21.339 s |
| -250 ppm | 4 | 627 | 42.672 s |
| -100 ppm | 1 | 73 | 106.677 s |
| -50 ppm | 0 | 0 | none |
| 0, +50, +100, +250, +500 ppm | 0 | 0 | none |

These schedules expose an asymmetric controller. Its proportional term is
`(occupancy - target) / target * 0.0002`. Occupancy cannot be negative, so its
negative correction cannot exceed 200 ppm even though the final clamp is
±3,000 ppm. SRC lookahead reduces the usable negative range further. The positive
side has considerably more buffer headroom. The initial 256-frame schedule also
has occupancy ripple that this very small proportional correction does not
fully absorb.

This is the retained pre-fix result. The same nine-case instrument against the
corrected implementation has zero late silent blocks, dropped frames, or
underflow frames in every case (`elastic-clock-baseline-after.csv`). This first
instrument measures FIFO continuity with constant samples; the signal matrix
below adds frequency, amplitude, and phase-step checks.

## Controller and timestamp handling

Each unique capture and each output has its own rate estimator. The callback
provides CoreAudio sample/host timestamps; callback arrival time is deliberately
not used to estimate hardware rate. Sample and host validity flags must both be
present. A measurement window covers at least 250 ms, with finite, strictly
increasing host/sample time and the expected sample advance from the previous
callback. Each estimate is normalized to that device's nominal sample rate.
Subsequent estimate changes are limited to 1,000 ppm per second. Observed device
rates outside ±5,000 ppm or windows exceeding ten seconds are rejected.

The FIFO uses the input/output factor ratio as feed-forward. A bounded occupancy
correction remains available for buffer position. It measures position error in
seconds, reaches both signs of its ±3,000 ppm limit, and smooths changes in time
rather than callback count. While timestamps are valid, a guarded one-period
deadband prevents whole capture packets from canceling the clock estimate. A
relative measured clock mismatch outside the correction range latches silence
and stops the graph with an actionable error.

Declared minimum/maximum callback sizes are read before starting. The maximum
seeds both FIFO sides, so a small first callback cannot establish insufficient
headroom for a later larger callback. The initial target is two maximum callback
periods plus SRC lookahead, rounded up to the actual whole-block fill at priming.
That reserve is retained rather than chased on every callback. Capacity allows
at least four maximum periods plus kernel storage; larger callback sizes can
therefore increase preparation memory and latency. The existing resource budget
still bounds total graph allocation.

Missing timestamp metadata resets the estimate window and selects occupancy-only
fallback; telemetry counts nodes in this state, including initial observation
and idle process taps. A flagged nonfinite timestamp or a device timeline that
skips/repeats samples or goes backwards latches failure. The affected callback
immediately produces no new audio, and the control poll stops the graph without
automatic restart. Process taps permit an idle pause and re-anchor instead of
treating an application's silence as a failed physical clock. Rate/format/buffer
changes require a fresh graph. Successful explicit re-preparation clears the
clock-mismatch latch only while callbacks are stopped.

The supported nominal range is 8–384 kHz with input/output ratios from 1:24 to
24:1 and callbacks no larger than 16,384 frames. Both declared limits and actual
callbacks are checked; an oversized callback latches silence. No new duration
cutoff was introduced to remove a failing test. In particular, the original
8 kHz/16,384-frame case remains in the matrix and now passes with valid timestamp
estimation. Its large FIFO latency still makes it unsuitable for ordinary live
monitoring.

## Retained comparison and signal matrix

`scripts/elastic-clock-probe.c` executes deterministic generated schedules against
the actual C FIFO/resampler and clock estimator, with no HAL device or wall-clock
waiting. Its signal is a 440 Hz stereo quadrature tone with radius 0.125. The
55 cases total 7,500 virtual seconds. They cover ±50 through ±1,000 ppm offsets;
independent opposing and matching input/output offsets; source drift steps and
sinusoidal drift; unequal and varying callback blocks; deterministic arrival
jitter up to 35% of a callback period; 8–384 kHz; rate-ratio endpoints; long
buffers including 100 ms and 2.048 seconds; late metadata and a temporary metadata
gap. Arrival jitter does not alter the generated hardware timestamps.

| Implementation | Cases meeting every criterion | Artifact |
| --- | ---: | --- |
| Frozen `784e38f` | 9 / 55 | `elastic-clock-extended-before.csv` |
| Corrected clock controller | 55 / 55 | `elastic-clock-hardware-timestamp-extended-matrix.csv` |

The comparison uses the same final probe, input samples, schedules, ring capacity,
and acceptance limits. Its legacy build flag omits APIs that did not exist in the
frozen source. It does not substitute a mocked FIFO or clock implementation.
Every passing case has zero late silent/partial blocks, dropped/underflow frames,
nonfinite samples, and timestamp faults. Initial FIFO priming is reported
separately. The fixed signal gates are radius error below 0.0001, mean frequency
within 0.15 Hz of 440 Hz, and a phase-step bound consisting of 1% of the expected
step plus the existing 512-phase interpolation quantization allowance. Frequency
and phase-step checks begin after ten virtual seconds.

Across the final matrix, the largest mean-frequency error is 0.03266 Hz, maximum
radius error is 0.000000176, and the greatest phase-step error uses 31.22% of its
allowed bound. Exact values, source hashes, commands, compiler, artifact hashes,
and limitations are in `research/evidence/elastic-clock-controller-final.json`.

Earlier failed candidate results remain in `elastic-clock-first-matrix.csv`
(31/33), `elastic-clock-primed-reserve-matrix.csv` (33/45), and
`elastic-clock-time-normalized-matrix.csv` (36/45). Increasing FIFO reserve and
normalizing proportional feedback alone did not satisfy the frequency gate for
coarse blocks. Those failures motivated using the hardware timestamps; their
acceptance thresholds were not relaxed. These exploratory candidate CSVs retain
their observed values, but do not have complete source snapshots. The frozen
baseline and final comparison do have exact source hashes.

Focused tests additionally exercise invalid/retrograde/duplicate/nonfinite
timestamps, estimator reset and slew bounds, relaxed process-tap pauses, native
callback fail-closed behavior, declared/actual oversized buffers, seeded
headroom, and out-of-range relative clocks. The final focused run passed 47 tests
in 11 suites; its log is `hardware-clock-focused-tests-final.txt`.

Reproduce the final matrix from the repository root:

```sh
clang -O2 -std=c11 -I Sources/ReLinkRealtime/include \
  scripts/elastic-clock-probe.c Sources/ReLinkRealtime/Realtime.c \
  Sources/ReLinkRealtime/SoundCheck.c Sources/ReLinkRealtime/Clock.c \
  -framework CoreAudio -o /tmp/ravelink-clock-matrix
/tmp/ravelink-clock-matrix
```

## Qualification boundaries

This establishes correction of the demonstrated generated-schedule failures.
It does not establish physical-clock accuracy, callback delivery under real
load, sleep/hotplug recovery, complete resampler frequency response, perceptual
transparency, or long-session stability. The independent installed-driver soak
at run `40011635266` remains a failure with retained timestamp gaps and 296
engine underflow frames; these generated results do not replace that outcome.
The next strict installed run must retain its original signal/counter gates.

The estimator corrects rate; it does not align absolute sample time, start phase,
or device latency between different capture selectors or physical devices.
Exact-selector raw sharing preserves sample coherence within that shared path,
and AU compensation aligns effect latency only. Unrelated paths can still have
different start/transport delays. Occupancy-only fallback remains available when
metadata is absent, but the full coarse-block frequency guarantee is not
established for indefinite missing metadata. No universal physical latency or
clock-fidelity claim follows from this matrix.

## Capture callback liveness

An active device capture now has its own frame-progress watchdog. Another input
or output progressing cannot mask a stopped input callback. The initial and
ongoing grace period is the greater of five seconds and four maximum-sized
buffers at that input's sample rate. The monitor counts unique native captures,
so sharing one device selector among many logical inputs does not create extra
timers. It measures returned frames even when every sample is silent. If a
device stops returning frames, all output is silenced and audio stops with an
input-specific recovery message; it does not automatically restart.

Process taps are deliberately exempt because an application that is not playing
may stop delivering callbacks. Existing application-membership and device-format
health checks continue separately. Four pure monitor tests cover an individual
stall amid other progress, silent device progress and idle-app exemption, initial
grace/reset/removal, and the largest supported block duration. Together with
existing render/lifecycle tests, these cases are included in the final focused
run above. They establish the watchdog policy; no native driver was deliberately
stalled.
