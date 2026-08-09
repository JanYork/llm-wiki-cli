# npm Distribution Design

## Goal

Publish LWC as the public npm package `@i-xor/lwc` so Node users can install
the native CLI with `npm install --global @i-xor/lwc`. Keep GitHub Release as
the single source of native binaries and checksums.

## Package shape

Keep the npm package under `npm/` with no npm runtime dependencies:

- `package.json` declares the matching LWC version, supported Node version,
  public access, repository metadata, and the `lwc` executable.
- A small Node installer maps `process.platform` and `process.arch` to an
  existing release asset, downloads the archive and `SHA256SUMS` for the exact
  package version, verifies SHA-256, and invokes the host `tar` executable to
  extract the native binary into package-local storage. Current macOS/Linux and
  supported Windows releases include `tar`; a missing executable is an explicit
  prerequisite error rather than a reason to add an extraction library.
- A small Node launcher executes that verified binary and forwards arguments,
  stdio, signals, and exit status.

The supported mappings are `darwin` x64/arm64 to
`x86_64-apple-darwin`/`aarch64-apple-darwin`, `linux` x64/arm64 to
`x86_64-unknown-linux-gnu`/`aarch64-unknown-linux-gnu`, and `win32` x64/arm64
to `x86_64-pc-windows-msvc`/`aarch64-pc-windows-msvc`. Windows Node shells use
the `.zip` archive and `lwc.exe`; macOS/Linux use `.tar.gz` and `lwc`.
Unsupported combinations fail with a structured actionable message.
Installation never writes into the user's global LWC data directories.

## Release flow

The Cargo version, npm package version, Git tag, and GitHub archive version must
match. The fixed order is: tag acceptance, native archives, combined
`SHA256SUMS`, GitHub Release, npm install smoke against that exact tag, then npm
publication.

npm publication is deliberately local, not a GitHub Actions job. After the
GitHub Release succeeds, an authenticated maintainer runs
`npm publish --access public` from `npm/` and verifies the registry result. The
repository currently has no local npm login, so `npm publish` cannot be claimed
successful until npm accepts it. No npm token, trusted-publisher configuration,
or npm publication step is stored in the GitHub workflow.

After publication, require `npm view @i-xor/lwc version` to equal the tag
version, then install `@i-xor/lwc@<version>` into a fresh temporary prefix and
require its `lwc --version` output to match.

## README presentation

Add a compact badge row near the title in both English and Chinese READMEs for:

- npm package/version
- CI status
- supported platforms
- Apache-2.0 license

The npm installation command appears next to the existing release installer.
Badges link to npm, Actions, or the repository license and do not introduce
generated documentation.

## Security and failure handling

- Download only HTTPS assets from the repository's exact immutable release tag.
- Verify the archive against the release `SHA256SUMS` before extraction.
- Reject checksum lines that do not exactly name the expected archive.
- Extract only the known `lwc` or `lwc.exe` member and install it package-locally.
- Execute the extracted binary and require `lwc --version` to equal the npm
  package version before making it available to the launcher.
- Use a temporary directory and clean it on success or failure.
- Never print registry credentials or persist tokens in repository files.
- Do not silently fall back to another version, platform, or unverified binary.

## Verification

Automated tests cover all six platform-to-asset mappings, version parity,
checksum success/failure, missing `tar`, launcher argument/exit forwarding, npm
package contents, and README/package contracts. Pre-tag acceptance runs
`npm pack --dry-run` and offline fixture installation. After GitHub Release, the
release job installs from the packed npm directory against the exact live tag
assets and requires `lwc --version` to match before the maintainer publishes
locally from `npm/`.

Success means the Rust suite remains green, the npm tarball contains only the
declared package files, the installed `lwc --version` matches the npm version,
and both GitHub and npm publication are independently verified.
