# Sound Check core ownership contract

`SoundCheck` is an independently authored memory-only dry PCM recorder and player.
Its 30-second maximum is ReLink policy, not an observed claim about Wave Link 3.4.
See [the research and observation limits](../../docs/sound-check-research.md).
This module opens no devices, asks for no permission, writes no audio files, and
establishes no native capture or output-isolation guarantee.

Prepare storage on the control thread with the selected input UUID, sample rate,
logical frame capacity and a byte budget. Rates from 8,000 through 384,000 Hz are
accepted. Checked duration and requested-byte checks precede allocation; a second
check charges actual `Vec` capacity before handing storage to the caller.
`required_bytes` is the requested minimum, `allocated_bytes` is actual PCM
allocation capacity and `working_bytes` includes owner metadata. Allocator
bookkeeping and any host buffers are separate. The logical capacity never grows
with allocator excess. Preparation opens Idle and performs no capture or playback.

Each owner has an opaque session identity and operation generation. Begin Record,
Stop, Clear, Close, reaching the cap, beginning Play, finishing nonloop playback
and transferring a clip invalidate prior operation tokens. A stale token cannot
capture or replay an earlier clip. Exhausted counters never wrap; generation
exhaustion closes the owner. A token is a logical state guard and is **not proof
that native capture stopped**.

Record copies only matching-source frames before ReLink DSP, bit for bit. It
cannot remove processing already applied by hardware. The last callback accepts
exactly the remaining frames, then returns Ready with a new token. Stop preserves
a nonempty clip and rewinds it. A Ready-only immutable view exposes exactly the
recorded length. `take_clip` moves the unique storage into an immutable owned clip
and permanently closes the old owner; `from_clip` moves it into a Ready playback
owner. No mutable PCM view or clone operation is provided.

The host must stop **all** native capture, establish callback and native-owner
quiescence, and only then move the completed clip to a separate playback context
which owns no native capture streams. Core methods cannot establish that proof.
The host must also reject stale completions by the returned session/generation,
choose an explicit audition output, and feed dry replay through current DSP once.
Replay itself applies no effects, resampling or gain. Explicit Loop wraps exact
PCM without a fade; a host fade policy is separate. Nonloop playback fills the
remainder of the entire output block with zero, returns Ready and rewinds. Every
rejected render also zeros both supplied output slices.

Clear removes logical access immediately and retains capacity for another
recording. Close removes access permanently and retains capacity until reclamation.
Neither operation overwrites PCM on the audio callback. Old storage cannot be
accessed through the current Idle/Closed token, an earlier token, or a later
shorter recording. After quiescence, `erase_cleared_storage` overwrites every
initialized slot, including old tails, without freeing capacity; consuming
`erase_and_reclaim` does the same before releasing it. Volatile writes prevent
dead-store removal. Allocator slack beyond logical capacity never held captured
PCM and is not erased. These methods do not promise erasure of copies elsewhere,
operating-system paging or other system memory.

Preparation, physical erasure, and final owner/clip destruction are control-thread
work. Other engine methods use fixed metadata and preallocated storage, with no
locks, allocation or deallocation. Moving a clip does not free storage, but the
host must retain it and prevent its final drop on the audio callback. The engine
does not enforce thread identity or provide a cross-thread handoff queue.

Generated-buffer tests cover selected identity, stale generations, overflow,
30-second partial caps, exact dry samples, loops and callback partitions, EOF and
shorter-recording tail silence, immutable ownership transfer, actual capacity
accounting, and explicit erasure. A counting allocator checks first and repeated
callback paths including rejection, Clear/Close and ownership movement. Offline
mixer replay uses an independent gain oracle and checks unchanged raw PCM, current
effects/bypass applied once and an unrelated bus remaining silent. These are core
source/generated-buffer controls, not native, auditory or end-to-end acceptance.
