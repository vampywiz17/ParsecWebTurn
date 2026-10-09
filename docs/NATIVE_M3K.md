# M3k / 0.14.0: local desktop interaction

The user confirmed login, the remote computer entry and responsive left-side
menus in 0.13.2. Connect was not yet tested. Clicking a web link closed the app,
and local clipboard paste and Tab navigation did not work. The old bridge
forwarded WM_CHAR only and lacked MTY_HandleProtocol and clipboard imports.

The original core still owns the Computers list, API requests and UI. This
stage supplies missing local platform services rather than duplicating that
logic or automating a connection. The private Matoya ABI remains pinned to
the unchanged original WASM. Windows APIs and UI Events code names are
documented interfaces; their adapter to Parsec is version-dependent.

## Keyboard and scrolling

Win32 key press/release messages carry Matoya's registered key code and the
pinned Shift/Control/Alt/Meta/CapsLock/NumLock mask. All web_set_key aliases
are retained for forward lookup, including aliases without a reverse label.
Navigation and function keys use documented virtual-key values; PC scan codes
identify physical printable-key positions independently of keyboard layout.
WM_CHAR separately supplies layout-correct Unicode text. UTF-16 surrogate
pairs are combined, and control characters are omitted to avoid duplicating
Tab, Enter, Backspace and Ctrl+V as text. This is not IME composition support.
Focus loss releases tracked keys. Wheel events reach mty_window_scroll with
the pinned browser's axis convention. No keys or typed text enter reports.

## Local desktop services

Only account mode creates the real Windows desktop service. Clipboard reads
and writes use CF_UNICODETEXT while the app owns foreground focus. Reads copy
bounded, validated UTF-16 text before releasing the clipboard and global-memory
locks. Writes transfer a movable allocation to Windows only on success.
Clipboard text is limited to 1 MiB of UTF-8, with embedded NUL rejected.
web_get_clipboard returns a NUL-terminated guest-owned allocation even for an
empty/unavailable clipboard, or null on guest allocation failure.

MTY_HandleProtocol opens a validated absolute HTTPS URL through ShellExecuteW
and the default browser. Paths, executables, credentials in URLs and custom
protocols are rejected. There are no shell commands or argument strings. The
original token argument is unused, matching the pinned worker. COM apartment
initialization is balanced. web_alert uses a native informational MessageBox.
Unavailable/failed platform actions return without trapping the whole guest;
unknown imports and malformed guest memory accesses still fail normally.
Reports contain shared counters only, never clipboard text, URLs, dialog
contents or tokens. The exact HTTPS/WSS account network policy is unchanged.

This clipboard is for the local original UI. It does not establish clipboard
delivery to a connected remote host. Actual Connect, remote media and full
menu functionality remain unverified.

## Validation

The guest-platform probe executes the actual imports against a synthetic
desktop without reading/changing the real clipboard, launching a browser or
using an account. It checks Unicode reads/writes, separate guest allocations,
rejection of unsafe link targets, dialogs, aliases, disabled empty fallback,
and secret-sentinel redaction. Unit tests also cover keyboard layout/scan-code
mapping, navigation and surrogate/control character handling. The original
offline login-audit switches fields with Tab and pastes the fixture password
with Ctrl+V through a synthetic clipboard. It then verifies the original
authentication request remains blocked by offline policy. Its nine input
stages never read or modify the OS clipboard.

User acceptance should check Tab/Shift+Tab between login fields, local text
paste/copy, wheel scrolling and an HTTPS help link. Then test Connect separately
and return the account report after closing without restarting. No successful
host connection or decoded video is claimed by this milestone.

References:
- https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-keydown
- https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-mousewheel
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeystate
- https://www.w3.org/TR/uievents-code/
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboarddata
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setclipboarddata
- https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew
