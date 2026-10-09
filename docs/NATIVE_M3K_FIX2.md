# M3k cursor fix (0.14.2)

The user's 0.14.1 report passed the previous `MTY_WaitPtr` boundary. The main
guest then stopped at the unimplemented `env::web_set_png_cursor` import. There
was also a disconnect call, backend status -3 and no received control frames;
this report does not prove an established host connection or identify the reason
for that disconnect. Cursor support fixes the reported process exit independently.

The pinned Matoya cursor ABI now supports PNG/RGBA images, null image reset,
visibility and switching between the custom cursor and the default system arrow.
Guest image data is copied and bounded (256 by 256 pixels, 1 MiB encoded PNG).
Invalid, unsupported or unavailable custom images fall back to the system arrow
without trapping the guest. Hotspots are clamped to the image bounds.

Cursor images use premultiplied BGRA and an alpha-aware monochrome mask. Native
resources are created, replaced and destroyed on the window UI thread, with RAII
bitmap cleanup. Pending updates are coalesced to one image and one posted message;
there is no growing cursor image cache. `WM_SETCURSOR` changes only the client-area
cursor and leaves window borders/titlebar cursors to Windows. Visibility uses
`SetCursor`, without changing the thread's global ShowCursor counter.

These are documented Windows APIs implementing a pinned private Parsec/Matoya
bridge, not a new web standard or a modification to the original WASM:

- https://learn.microsoft.com/en-us/windows/win32/menurc/using-cursors
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createiconindirect
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-destroycursor
- https://learn.microsoft.com/en-us/windows/win32/menurc/wm-setcursor

Tests cover bounded images, PNG decoding, alpha conversion, hotspot bounds,
80 native cursor create/replace/reset cycles, and actual guest calls including
null reset and invalid guest buffers. No real clipboard or account is involved.
Real-host stream startup and remote video/audio remain incomplete and unverified.
