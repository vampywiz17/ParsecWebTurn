ParsecWebTurn v0.4.0 adds a modern settings interface and direct support for your own STUN/TURN servers.

- Parsec-inspired dark header, pink accents, a spacious connection card and inline validation.
- Matching Parsec icons in the settings window title bar and Windows taskbar.
- Automatic Edge translation prompts disabled in the dedicated app profile, preserving other preferences and existing sign-in sessions.
- Choose Cloudflare Realtime or a custom provider with fields tailored to each option.
- Configure coturn, eturnal and other standard services using multiple URLs and a common username/password.
- UDP, TCP, TLS and STUN-only configurations are supported.
- Custom passwords are saved using Windows DPAPI; custom mode requires no Cloudflare credentials or API calls.
- Switching providers preserves existing settings, and older Cloudflare settings migrate automatically.
- Additional tests cover provider selection, encrypted custom storage, offline resolution, migration and saving through the native form.

Download **ParsecWebTurn-v0.4.0-win64.zip**, extract it to a writable directory, and run `ParsecWebTurn.exe`. Existing `settings.json` files remain compatible; Cloudflare caching is still opt-in. The standalone EXE also needs the `extension` files provided in the ZIP.

Close existing ParsecWebTurn Edge windows before starting this version. Credentials are refreshed at launch; choose a TTL longer than your expected session. There is no automatic credential refresh within an already open session.

Custom TURN accepts static or externally generated username/password pairs. A TURN REST shared secret is not a client password: generate the temporary credential pair on your server first. The app does not generate or refresh custom REST credentials.

The no-install design is unchanged. Saved secrets are protected by Windows DPAPI for the current user. Generated `extension/inject.js` necessarily contains the active TURN credential, which can be long-lived for custom static accounts; do not share that file or the browser profile.
