# Application capture preview

Application capture is an early preview in the current source. The previously
published development ZIPs predate this feature. Compilation and deterministic
tests pass separately from native capture qualification, which remains incomplete.

1. Open the application whose audio you want to mix and start its audio.
2. In ReLink, stop audio, choose **Add source → Application**, then refresh the list.
3. Choose the application and click **Add application**. Select its sends and the
   output devices in your studio, then start ReLink audio.

On macOS, application capture requires macOS 14.2 or later. Start may request
system audio-recording permission, separately from microphone permission. On
Windows, the OS must support process-loopback activation; unsupported builds or
inaccessible processes return an error. This is not a claim of support for every
Windows 10 build.

The application choice is saved with the studio. Each Start resolves its current
processes. If the application restarts or changes its captured process membership,
ReLink stops with an error; stop and start audio again. Seamless relaunch and idle
source recovery are not yet qualified. Refreshing the list does not start capture.

Capture leaves the application's original playback audible. Use an appropriate
separate destination for that playback if hearing it alongside ReLink would double
the sound. ReLink does not change the system default output. Selecting ReLink
itself, overlapping process trees or a Windows tree containing ReLink is rejected
to prevent capture feedback or duplicated sources.

The current macOS adapter includes audio processes inside the exact selected app
bundle, including nested helper bundles. Unattributable shared system services
are outside that boundary. Windows includes all current instances of the exact
selected executable and their descendant trees without counting nested roots
twice. A complete selection must fit the memory budget; it is never silently
truncated to a fixed number of channels or instances.

Unsupported rates, missing processes, ambiguous identities and unavailable native
capture report errors. There is no automatic system-audio replacement. Built-in
channel controls and effects apply after the application's streams are combined.
Third-party plugin hosting and a system-audio remainder channel remain future work.

[Technical research and unresolved qualification](audio-application-capture-research.md)
