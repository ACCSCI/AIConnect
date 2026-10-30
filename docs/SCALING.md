# Collaboration groups and public channels

From v0.3.1, a newly created collaboration group defaults to **64 members**.
Choose a smaller signed limit with `group create --max-members 2..64`.
Existing groups keep their policy until the owner runs:

```sh
aiconnect group limit GROUP_ID --max-members 64
```

Only the owner can change capacity. Shrinking below the current membership is
rejected. Changes advance the signed policy revision/epoch and propagate through
the existing member replication protocol. No member is removed implicitly.
Legacy groups without a signed capacity field retain their original fallback.

Public channels have a different distribution model. They have **no subscriber
membership registry or 64-reader cap**. The publisher exports a signed head and
JSON Feed mirror. Readers pull from static hosting/CDNs, keep local cursors,
deduplicate locally, and use ETag/Last-Modified when available. Publishing does
not send one P2P copy to every reader or require all readers to connect.

```sh
aiconnect channel create --name Updates --visibility public
aiconnect channel export CHANNEL_ID --output ./public-channel
# Publish these files with your chosen static host, then subscribe:
aiconnect channel subscribe --url https://PRIMARY/channel.json \
  --expected-publisher PUBLISHER_ENDPOINT_ID \
  --mirror https://MIRROR/channel.json
```

Mirrors are optional HTTPS copies of the same signed channel, up to three per
subscription. A pinned publisher is mandatory with mirrors. An HTTP failure or
invalid primary body allows trying the next configured mirror; all candidates
still pass the existing SSRF, provider, download-budget and signature checks.
Conditional cache headers are reused only for the URL that issued them.
Messages never grant execution rights. Mirroring never publishes private groups.

Private channels use the permissioned group protocol and default to 64 members.
`--max-members` is rejected for public channels because subscribers are not group
members. There is no project-operated relay, subscription broker or R2 bucket.

Validation includes owner-only expansion and signed synchronization, rejection
of invalid limits and unpinned/non-HTTPS mirrors, plus **1,000 logical subscriptions
in one local consumer profile** importing the same signed channel after the
publisher is offline. Each subscription receives/deduplicates independently.
This is not a 1,000-device WAN benchmark. Previously recorded 16/32/64-node tests
remain short, single-host transport evidence.

Subscriber capacity is distinct from history retention: the current exported
head retains at most 64 events (and can trim for byte limits), and readers report
gaps rather than inventing missing history. Long-term public archives and global
peer gossip are separate work; CDN/mirror distribution is used here.
