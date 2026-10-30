# Installation

Installing this skill only downloads instructions. Loading it in an agent is a separate step. The `skills` installer does not implicitly execute an AIConnect installer.

The distribution supplies a platform installer and a release manifest containing native artifact URLs, sizes, SHA-256 hashes, and an Ed25519 signature. The trusted signing key must come from the independently accepted distribution configuration, not from an untrusted manifest itself.

The skill includes `scripts/install.ps1`, `scripts/install.sh`, and their sibling `scripts/install.mjs` engine when distribution tooling is staged. Use PowerShell on Windows or `sh` on Unix, passing the accepted manifest URL and pinned trusted public key as documented by the script. Keep `install.mjs` beside its wrapper. The site also serves these three files at its root. Read the downloaded script before executing it. Use a per-user installation location, preserve existing state, and run setup/status after installation. Do not require an administrator or install a compiler on the user's computer.

## Signed release installation

Manifest: `https://connect.lumigrav.space/releases/stable.json`

Pinned Ed25519 public key (Base64 SPKI DER):

```text
MCowBQYDK2VwAyEAubdppWBlKweZ1XkjVVBsEYNKO3IFBNqQeyT50/ah7vI=
```

Verify this pin against the versioned repository at https://github.com/ACCSCI/AIConnect/blob/main/distribution/release.json before installing from a new distribution source. Never replace it with a key found in a manifest.

Run the installed Skill's engine by absolute path:

```sh
node ABSOLUTE_SKILL_DIRECTORY/scripts/install.mjs --manifest https://connect.lumigrav.space/releases/stable.json --public-key MCowBQYDK2VwAyEAubdppWBlKweZ1XkjVVBsEYNKO3IFBNqQeyT50/ah7vI=
```

The JSON result gives the exact executable path. Run that executable with `setup --name NAME --json`, then `status --json`. Preserve the existing profile. The installer receipt carries the pinned key and enables signed update checks on daemon startup; updates install for the next restart without interrupting active work. Use `aiconnect update check`, `aiconnect update`, or `aiconnect update rollback`. Use `aiconnect update configure --manifest URL --public-key KEY --automatic false` to keep checks but disable automatic installation.

If the release is unreachable or your platform has no artifact, stop and report that result. Do not weaken signature checking or silently compile/install dependencies. Explicit local test opt-ins must never be applied to public releases.

The current verified bootstrap engine requires Node.js 22.20 or newer at installation time, which is also used by the `npx skills` path. The installed native AIConnect executable does not require Node.js. Computers without Node.js can manually obtain and independently verify the native executable; do not claim the current bootstrap wrappers run without Node.js.

The Skill also includes `scripts/codex-app-server.mjs` for optional task execution with an existing local Codex CLI. This bridge needs Node.js at runtime, independently of the native messaging binary. Installation of the Skill makes the bridge available but does not attach it, grant contact permissions, or modify global Codex settings. Follow the Skill's local runtime attachment workflow only when requested.

Optional A2A/MCP source is bundled under `scripts/interop`, including its own
package.json and exact package-lock.json. Tests and node_modules are not distributed.
Only if the user requests those adapters, check Node compatibility and run
`npm ci --omit=dev --prefix ABSOLUTE_SKILL_DIRECTORY/scripts/interop`. Then pass
the absolute `scripts/interop/bridge.mjs` path to `aiconnect bridge ... --script`.
This is a local dependency install, not npm publication or a global install.
Do not run it merely because the Skill was added, and do not enable a listener,
Tunnel, runtime attachment or remote execution as part of dependency installation.
Normal native communication does not require Node.js or these adapter dependencies.
