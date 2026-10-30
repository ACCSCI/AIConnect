# Native distribution (pre-release)

AIConnect does not publish its own npm package. Skill installation uses the existing `skills` CLI. The Skill subsequently runs this bootstrap tooling; `skills add` does not execute our installers automatically.

## Trust and artifact format

`manifest.schema.json` describes a release envelope. Signatures cover UTF-8 `canonical(payload)` (recursive object-key sorting, arrays ordered) using Ed25519. The install command pins a trusted SPKI DER public key in Base64 via `--public-key`; a key embedded in an untrusted manifest is never accepted as a trust anchor. Each artifact also carries its exact size and SHA-256. All network downloads require HTTPS, including redirect targets. `--allow-http` enables **loopback HTTP only** for tests; unsigned manifests additionally require `--allow-unsigned` and a loopback manifest URL.

There is **no production release key or signed production release yet**. Before publishing, pin the reviewed release public key in the versioned bootstrap instructions. Keep private keys in the release signing environment, never in this repository. Release artifacts are raw native binaries, not archives, removing extraction/path traversal concerns.

## Website staging contract

Copy these public files unchanged:

- `distribution/install.ps1` → `/install.ps1`
- `distribution/install.sh` → `/install.sh`
- `distribution/install.mjs` → `/install.mjs`
- `distribution/install.ps1`, `install.sh`, `install.mjs` may also be mirrored beside the Skill under `/.well-known/skills/aiconnect/scripts/`.

Both wrappers use a sibling `install.mjs`. PowerShell can fetch a missing engine from `-EngineUrl` (default relative to the manifest). For shell bootstrap, download both files into the same staging directory before running. The engine requires Node.js at **installation time only**; installed AIConnect is a native executable. The source Skill can use Node already provided for `npx skills`.

The initial bootstrap scripts/engine must come from a trusted website or reviewed Skill. Release artifact signatures protect the downloaded native binary; they cannot protect a bootstrap script that has itself been replaced by an attacker. The PowerShell engine downloader also restricts each redirect to HTTPS (or explicit loopback HTTP tests) and bounds download size.

Do not create a successful `/releases/stable.json` until signed real artifacts exist. Use an honest unavailable response in development. Installer failure before version validation leaves an existing executable unchanged. Binary replacement is staged on the same filesystem, with previous-binary rollback on replacement/receipt failure. Stop a running daemon (`aiconnect daemon stop --json`) before replacing a Windows executable. Installer never stops unrelated processes, edits PATH, registers startup services, or touches identity/database directories.

## Local usage

```powershell
node distribution/install.mjs --manifest https://YOUR-HOST/releases/stable.json --public-key PINNED_BASE64_DER --version 0.3.0
node distribution/install.mjs --uninstall --install-dir PATH
node --test distribution/test-install.mjs
# When the actual native CLI has built, additionally verify setup/daemon/reinstall identity:
$env:AICONNECT_NATIVE_BINARY = (Resolve-Path target/debug/aiconnect.exe).Path
node --test distribution/test-install.mjs
```

Default install directory: `%LOCALAPPDATA%/AIConnect/bin` on Windows, `~/.local/bin` elsewhere. The JSON installation result returns the exact executable path; the Agent can invoke it immediately. Add its directory to PATH separately if desired. Call `aiconnect setup --json` after successful installation. Setup and daemon auto-start are owned by the CLI, not hidden installer side effects.

Uninstall removes the installed executable and its receipt only. User identity, contacts, messages, credentials and policies remain. Purging these requires a separate explicit data-management operation; this installer deliberately has no recursive data purge.

`build-manifest.mjs --input SPEC --output NEW_FILE [--key-file PRIVATE_PEM]` generates metadata from actual files. Input has `version`, optional `keyId`, and `artifacts` mapping each target to `{path,url}`. Output refuses overwriting an existing manifest. With no signing key it produces a local-test-only unsigned manifest.
