# Releasing marklark

A release is one tag. `.github/workflows/release.yml` then checks, builds and publishes:

| Channel | What | Users install with |
|---|---|---|
| crates.io | the source | `cargo install marklark` |
| PyPI | one wheel per platform with the compiled binary inside (maturin, `bindings = "bin"`), plus the sdist | `pip install marklark` / `uv tool install marklark` |
| GitHub Release | binary archives with SHA-256 checksums | download, or `cargo binstall marklark` |

Platforms: Linux x86_64 and aarch64 (manylinux and musllinux wheels, static musl binaries), macOS x86_64
and Apple Silicon, Windows x86_64.

## One-time setup

1. **Make the repository public** (or accept that the GitHub Release is visible only to you while the
   crate and the wheels are public). Note: publishing to crates.io and PyPI makes the source public in
   any case.
2. **crates.io.** Create a token at https://crates.io/settings/tokens with the scope `publish-update`,
   limited to the crate `marklark`. In GitHub: Settings → Environments → create `crates-io`, add the
   secret `CARGO_REGISTRY_TOKEN` there.
3. **PyPI trusted publishing** (no token): at https://pypi.org/manage/account/publishing/ add a pending
   publisher — project `marklark`, owner `verhas`, repository `marklark`, workflow `release.yml`,
   environment `pypi`. In GitHub: Settings → Environments → create `pypi`. Optionally require your
   approval on both environments, so every publish waits for a click.
4. **Remove `publish = false`** from `Cargo.toml`.

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

Cost: on a private repository, GitHub Actions minutes count against your plan, and macOS runners count
ten times. One full run is roughly 15–25 jobs-minutes on Linux/Windows and 20 on macOS (≈ 200 billed
minutes). Public repositories run for free.
