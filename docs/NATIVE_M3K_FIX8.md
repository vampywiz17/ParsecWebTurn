# M3k-fix8 / 0.14.8 — identify remote ICE token rejection without disclosing it

The user's fix7 report retains `remote-begin`, `validation_error: ice-ufrag`,
an eight-byte ufrag, no terminal CR in any compact field, and a matching attempt
ID. The 32-byte password passes validation. No native candidates have been
processed. This proves that the observed rejection is not the terminal-CR case
addressed by fix7; it does not establish a network, STUN or TURN failure.

An eight-byte token is within the accepted length. Its character grammar is the
remaining rejection condition. The old report does not record character classes,
so padding, URL-safe punctuation or other bytes remain hypotheses.

`remote_begin_diagnostic.raw/normalized` now include `ufrag_shape` and
`password_shape`. Each contains a length-valid boolean and aggregate byte counts
for standard ICE characters, equals signs, URL-safe punctuation, space/tab,
CR/LF, other controls, non-ASCII and other ASCII punctuation. These are fixed
categories with no literal bytes, positions, prefixes, suffixes or hashes.
Existing string redaction and default origin omission remain in effect.

[RFC 8839 section 5.4](https://www.rfc-editor.org/rfc/rfc8839.html#section-5.4)
uses `ice-char` (ASCII letters/digits, plus and slash). Equals, hyphen and
underscore are not part of that grammar. This diagnostic deliberately does not
accept them, remove padding or re-encode the peer's credentials. Changing the
credential would change ICE authentication; any interoperability exception must
first be supported by evidence and documented separately from standard behavior.

Two tests cover aggregate categories, unchanged rejection behavior and privacy,
including an actual WASM begin import with a synthetic padded ufrag. It must
return the scoped connection failure without trapping the guest or serializing
the test credentials. This synthetic case is not claimed to match the real host.

Extract the package into a new folder and run `START-NETWORK-DIAGNOSTIC.cmd`.
Log in, press Connect once, close normally and share `account-network-report.json`.
This build improves diagnosis; it is not a connection fix and the same visible
error is expected until the actual representation can be identified.
