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
   2. The first publish needs a token, because trusted publishing can only be set up on a package that
      exists: npmjs.com → Access Tokens → Generate New Token → *Granular Access Token*, read and write,
      all packages of the user, short expiry, "bypass 2FA" on. In GitHub: Settings →
      Environments → create `npm`, add it as the secret `NPM_TOKEN`.
   3. After the first release, for each of the six packages (`marklark`, `@verhas/marklark-linux-x64`,
      `@verhas/marklark-linux-arm64`, `@verhas/marklark-darwin-x64`, `@verhas/marklark-darwin-arm64`,
      `@verhas/marklark-win32-x64`): Settings → Trusted Publisher → GitHub Actions, owner `verhas`, repository
      `marklark`, workflow `release.yml`, environment `npm`. Then delete the `NPM_TOKEN` secret and
      the token.

   The `npm` job builds the packages on every run; a manual run ends with `npm publish --dry-run`.
   Local check: `node npm/build.mjs <version> <dir with release archives> npm/out --partial`, then
   `npm pack` in `npm/out/*` and install the tarballs into a scratch project.

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
