# Security

For a suspected vulnerability, use GitHub's private vulnerability reporting
on this repository. Do not include keys, invitation secrets, daemon tokens,
message contents, or local databases in a public issue.

Report the affected version, operating system, reproduction steps, and impact.
Use isolated test profiles for reproductions. Contact discovery and received
messages never grant permission to execute commands or spend money.

Native releases use an Ed25519-signed manifest and exact SHA-256/size checks.
The release public key is pinned in the versioned Skill installation guide.
Never trust a public key supplied only by the manifest it is supposed to verify.
