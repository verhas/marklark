# Releasing marklark

A release is one tag. `.github/workflows/release.yml` then checks, builds and publishes:

| Channel | What | Users install with |
|---|---|---|
| crates.io | the source | `cargo install marklark` |
| PyPI | one wheel per platform with the compiled binary inside (maturin, `bindings = "bin"`), plus the sdist | `pip install marklark` / `uv tool install marklark` |
| npm | `marklark` plus one package per platform (`@verhas/marklark-linux-x64`, …) holding the binary | `npm install -g marklark` / `npx marklark` |
| GitHub Release | binary archives with SHA-256 checksums | download, or `cargo binstall marklark` |

Platforms: Linux x86_64 and aarch64 (manylinux and musllinux wheels, static musl binaries), macOS x86_64
and Apple Silicon, Windows x86_64.

## One-time setup

1. **crates.io.** Create a token at https://crates.io/settings/tokens with the scope `publish-update`,
   limited to the crate `marklark`. In GitHub: Settings → Environments → create `crates-io`, add the
   secret `CARGO_REGISTRY_TOKEN` there.
2. **PyPI trusted publishing** (no token): at https://pypi.org/manage/account/publishing/ add a pending
   publisher — project `marklark`, owner `verhas`, repository `marklark`, workflow `release.yml`,
   environment `pypi`. In GitHub: Settings → Environments → create `pypi`. Optionally require your
   approval on both environments, so every publish waits for a click.
3. **npm.**
   1. Create the account `verhas` and turn on two-factor authentication. The platform packages live in
      the user scope `@verhas/…` (the organisation name `marklark` is taken), so no organisation is needed.
   2. **A new package's first version is published by hand**, from your terminal with 2FA: npm lets a
      token that bypasses 2FA only *stage* a first version (`E_STAGE_REQUIRED`), and trusted
      publishing can only be configured on a package that exists. Download the release binaries and
      publish:

      ```sh
      gh run download <run-id> -p 'binary-*' -D /tmp/ml && mkdir /tmp/ml/a && find /tmp/ml -type f -name 'marklark-*' -exec mv {} /tmp/ml/a/ \;
      node npm/build.mjs <version> /tmp/ml/a /tmp/ml/npm
      npm login
      for d in /tmp/ml/npm/*-*/ /tmp/ml/npm/marklark/; do (cd "$d" && npm publish --access public); done
      ```

      (platform packages first, `marklark` last). This is needed again only if a platform is added.
   3. Then, for each of the six packages (`marklark`, `@verhas/marklark-linux-x64`,
      `@verhas/marklark-linux-arm64`, `@verhas/marklark-darwin-x64`, `@verhas/marklark-darwin-arm64`,
      `@verhas/marklark-win32-x64`): npmjs.com → the package → Settings → Trusted Publisher → GitHub
      Actions: owner `verhas`, repository `marklark`, workflow `release.yml`, environment `npm`. From
      then on the workflow publishes without any token. Do not store an npm token in GitHub.

   The `npm` job builds the packages on every run; a manual run ends with `npm publish --dry-run`.
   Local check: `node npm/build.mjs <version> <dir with release archives> npm/out --partial`, then
   `npm pack` in `npm/out/*` and install the tarballs into a scratch project.

4. **macOS signing.** The `macos-sign` job signs the macOS binaries (in the archives and the wheels;
   npm reuses the archives) with a Developer ID certificate made only for this CI, with the hardened
   runtime and a secure timestamp, and notarizes them (`.github/scripts/macos-sign.sh`). GitHub
   environment `apple`, with you as required reviewer, holds: `APPLE_CERT_P12` (base64 `.p12` of that
   certificate and its key, key named `marklark CI`), `APPLE_CERT_PASSWORD`, `APPLE_CERT_SHA1` (the
   certificate is chosen by hash: the name has an accented letter), `APPLE_API_KEY_P8` (base64 App
   Store Connect API key, role Developer), `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`. Without the
   secrets a manual run stays unsigned with a warning; a tag fails. If the key leaks, revoke that one
   certificate (it signs only marklark) and the API key. The certificate expires on 27 Dec 2027.
   Local test (login keychain, no notarization):
   `APPLE_CERT_SHA1=<hash> NOTARIZE=0 .github/scripts/macos-sign.sh <archives-dir> <wheels-dir>`.

## Each release

1. Set `version` in `Cargo.toml` (`0.0.0` is taken by the name placeholder; start at `0.1.0`), run
   `cargo build` so `Cargo.lock` follows, commit.
2. Optional dry run: GitHub → Actions → release → *Run workflow*. It runs the tests and builds every
   binary, wheel and the sdist — and publishes nothing.
3. Tag and push:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

   The workflow refuses a tag that does not match the `Cargo.toml` version.

A version, once on crates.io or PyPI, can never be reused — not even after a yank or a deletion. If a
release goes wrong, fix it and release the next patch version.

## Checked locally

`cargo package --list` (the crate contents), `maturin build` (a wheel for the local platform, installed
into a fresh virtualenv and run), `maturin sdist`, and `actionlint` on the workflow. Cross-platform builds
only run on GitHub; use the dry run.

The repository is public, so the workflow's Actions minutes are free.
