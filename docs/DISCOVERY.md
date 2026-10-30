# Local-first discovery

Discovery produces metadata, never a contact, runtime wake, execution grant or
payment. No source is enabled by default. The native transport remains iroh;
Nostr relays below are metadata services and cannot replace an iroh relay.

## Signed directories and cache

A static directory is a UTF-8 JSON file:

```json
{"schemaVersion":1,"descriptors":[{"payload":{},"signature":"..."}]}
```

`payload` is a complete signed native descriptor produced by
`capability publish`. Authors sign their own identity and revision. Directory
operators cannot rewrite that signature. HTTPS directories may contain public
descriptors only. Explicit offline imports with a `local:` source can retain
private metadata locally.

`directory add` requires the local operator to assert `free:true`,
`termsAllowed:true`, and a `termsUrl`. This is a recorded local attestation,
not an independently verified service guarantee. Registration expires. Fetching
requires public HTTPS on port 443, rejects userinfo and redirects, resolves DNS
once and pins the checked public addresses, disables ambient proxies, bounds
time and response size, and applies persistent download and request budgets.
HTTP 402/429 disables the provider. No paid fallback exists.

Directories use conditional ETag/Last-Modified requests. Failure retains the
signed cache and records a source error. A sync call handles at most two HTTPS
sources with a ten-second timeout per source; sync remaining sources by ID.
Limits: 128 sources, 128 descriptors per import, 512 KiB per directory,
4096 cached source/agent pairs, and 128 results per query.

Each query rechecks descriptor signatures. Expired entries are hidden unless
`--include-expired` is explicit; they remain labelled expired and invalid now.
The newest signed revision is selected across mirrors before capability filters,
so an old mirror cannot restore an author-removed capability. Same-revision
conflicts are reported. Names are not identities. Results merge by public key,
retain all source provenance, and never silently replace a key based on a name.

SQLite FTS5 unicode61 handles textual lookup, with bounded substring fallback
for Chinese. Language and operating-system filters match declared fields rather
than arbitrary description text. `mine` requires this device or a fresh
discovery-scoped owner delegation from a locally trusted root, including its
fresh revocation state. An owner label or directory trust flag cannot establish
ownership. `trusted` refers to a locally trusted source or unblocked contact;
neither establishes service quality. Blocked contacts are excluded.

## Explicit LAN discovery

`discovery lan advertise` registers `_aiconnect._udp.local.` for a short lease
(default 60 seconds, maximum 300). `stop`, daemon close, or lease expiry removes
the advertisement. Advertisements are not automatically renewed after restart.
The presence record contains an Agent ID and native protocol hint. If an
already-created, valid descriptor explicitly has `lan` or `public` visibility,
its signed bytes can accompany the record in bounded TXT chunks. Private
descriptors are never advertised. Oversized descriptors are omitted.

`discovery lan browse` performs an explicitly requested bounded multicast browse.
Presence, addresses and known-contact status alone are unverified. A supplied
descriptor is cached only after signature, time, visibility and identity checks;
owner proofs are independently checked. Valid own-device descriptors can then
appear in `discover --scope mine`. No network connection, contact import or
execution is automatically authorized. Multicast visibility depends on the host
firewall and local network; this does not discover devices across the internet.

## Nostr public metadata

NIP-01 events use kind 30078 (NIP-78), a `d=aiconnect:descriptor` identifier and
an expiration tag. The content contains a public native descriptor plus a
native Ed25519 signature binding its Agent ID to a **separately generated**
Nostr secp256k1 key. The outer Nostr event signs that binding as well. Both
signatures, timestamps, identities and descriptor visibility are verified before
cache import. No native secret key is reused for Nostr. The separate `nostr.key`
stays in the profile directory under its OS permissions; this is not advertised
as an operating-system credential vault.

`discovery nostr export --confirm-public` creates an event locally without
network publication. `import` checks a saved event. `publish SOURCE_ID
--confirm-public` requires a configured, currently allowed free provider and a
public descriptor, and only reports publication after the relay's positive
matching-event ACK. `read` and `relay probe` send a bounded REQ and verify events;
EOSE reports completion of the relay's stored-event response. Unsupported,
unreachable, fee-required and quota-limited services produce errors rather than
success. A provider may retain public metadata even after local deletion.

The WSS client permits port 443 only, rejects credentials/redirects, validates all
DNS results as public, connects directly to the checked socket addresses, and
performs TLS using the original host name. Message/frame size, event count,
operation deadline, persistent download budget and request limits are bounded.
Relay payment/quota notices disable the provider. There are no default relays or
project billing credentials. Public reachability and provider terms still need
separate validation; a successful handshake does not establish production usage
permission or reliable retention.

## Validation

`cargo test -p aiconnect-core --test discovery` covers signed source merging,
Chinese lookup, exact metadata filters, private/expired rejection, restart
persistence, stale-mirror replacement, owner proofs/revocation, Nostr dual
signature validation, and explicit publication/free-provider gates.

The separately selected test below performs real local multicast and may fail
when the host or network disables UDP multicast:

```text
cargo test -p aiconnect-core --test discovery lan_mdns_real_advertisement_and_browse -- --ignored --nocapture
```

These tests do not establish real WAN/NAT connectivity, public Nostr publication
and retention, or availability of a free production-compatible iroh relay.
