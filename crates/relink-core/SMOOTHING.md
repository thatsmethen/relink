# Live mixer controls

Every changed live control target takes **5 ms**, rounded up to a whole sample
frame, to reach its requested linear amplitude. This is 240 frames at 48 kHz,
221 frames at 44.1 kHz, and 480 frames at 96 kHz. `control_ramp_frames()` exposes
the exact count for the compiled session. It does not depend on device callback
size. Initial graph controls are exact from the first sample; compilation does
not add a startup fade.

After `apply_controls` succeeds, the next rendered sample advances one of those
steps. The Nth sample reaches the exact target. The interpolation uses its fixed
starting value and elapsed sample count rather than cumulative floating-point
addition. It explicitly assigns the final target, including zero. A newer
target starts a fresh 5 ms ramp from the last rendered gain. Repeated copies of
an unchanged target leave its existing ramp alone. No elapsed wall-clock time
is inferred while rendering is stopped.

Input and bus gain/pan use stereo ramps before their DSP. Their resolved
mute/solo activity uses a separate scalar ramp after DSP, so a completed channel
mute suppresses its own filter tail exactly. When both gain and activity change,
these two ramps run concurrently; their product need not be linear. Sends have
no DSP and ramp the combined resolved gain/pan/mute/solo target. A muted input or
send can still leave a previously excited *downstream bus filter* tail; muting
the bus itself suppresses its output exactly. Solo scopes and mute precedence
remain those defined by `PreparedControls`.

Every input oscillator, input/bus processor and ramp advances once per rendered
frame, including missing source buffers, muted channels and omitted output buses.
This preserves oscillator phase and filter/envelope history. A control change
does not rebuild the graph or restart DSP. Invalid control banks and rejected
buffer sets are detected before changing any ramp state; zero-frame renders do
not advance it. Sends whose ramp has already finished at zero can skip their
sample loop; an active ramp to or from zero must still advance. Retargeting a
completed muted route resumes its sample loop immediately.

Ramp state is inline in `CompiledChannel` and `CompiledSend`, and is included by
the graph's existing `size_of` memory accounting before allocation. Rendering
and retargeting require no allocation, lock, I/O or unbounded retry. The ramps
suppress discontinuities caused by control steps; they do not provide a delay,
crossfade between unrelated devices, fault recovery or changes to audio capture.

Offline tests compare actual PCM against expected ramp values, bound adjacent
gain steps at several sample rates, check stereo pan/unlink behavior, exact mute
endpoints, rapid retargeting, repeated banks, arbitrary callback partitions,
oscillator/filter continuity, rollback, memory bounds and zero callback
allocations. These tests do not establish installed-driver performance or
audibility on physical hardware. The installed-runtime protocol accounts for
the declared 5 ms window independently of its control acknowledgment and steady
measurement deadlines.
