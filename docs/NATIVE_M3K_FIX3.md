# M3k fullscreen fix (0.14.3)

The latest user report passed the cursor bridge and stopped at the next missing
import, `env::web_set_fullscreen`. A disconnect call, backend status -3 and zero
received control frames are still present. Removing this UI failure does not
prove a connection or identify why the connection attempt ended.

The pinned Matoya boolean fullscreen request now posts a coalesced command to
the owning Windows UI thread. Borderless fullscreen uses the current monitor's
rectangle; exit restores the saved window style and WINDOWPLACEMENT. Repeated
requests are idempotent, including exiting fullscreen while already windowed.
The resulting state is sent to the guest's original
`mty_window_update_fullscreen` export. Windows errors retain the guest event loop
instead of trapping the process. F11 toggles fullscreen locally and ignores key
repeat, providing a way back to a window without a Parsec connection.

This is a private pinned Parsec/Matoya ABI bridge using documented Windows APIs,
not a web standard or a change to the original WASM:

- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowplacement
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowplacement
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-monitorfromwindow

An isolated hidden-window probe performs five fullscreen roundtrips, checking
monitor bounds, border removal, exact style/geometry restoration and repeated
requests. A controlled WASM guest exercises the real boolean import. This probe
does not prove original-guest event feedback or real-host connectivity, and uses
no account, network or clipboard. Remote video/audio playback remains incomplete.

The adjacent teardown calls to disable pointer lock, keyboard grab and wake lock
are also accepted idempotently. These capabilities cannot currently be acquired
by the prototype, so there are no owned resources to release. Requests to enable
them remain explicitly unsupported; inspection labels them `inactive-release-only`
rather than claiming full support. A guest test covers repeated release and
rejected acquisition for all three imports.
