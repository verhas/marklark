# Installing marklark

marklark is a single binary with no runtime dependencies. Pick whichever package manager you already
have; they all install the same program.

| You have | Install | Update | Uninstall |
|---|---|---|---|
| Python (uv) | `uv tool install marklark` | `uv tool upgrade marklark` | `uv tool uninstall marklark` |
| Python (pipx) | `pipx install marklark` | `pipx upgrade marklark` | `pipx uninstall marklark` |
| Python (pip) | `pip install marklark` | `pip install -U marklark` | `pip uninstall marklark` |
| Node.js | `npm install -g marklark` | `npm update -g marklark` | `npm uninstall -g marklark` |
| Rust | `cargo install marklark` | `cargo install marklark` | `cargo uninstall marklark` |
| nothing | download a binary (below) | download again | delete the file |

Check the installation:

```sh
marklark --version
```

## Supported platforms

Prebuilt binaries (PyPI wheels, npm packages, GitHub Release archives):

| OS | Architectures |
|---|---|
| Linux | x86_64, aarch64 (static musl binaries; wheels for both glibc and musl) |
| macOS | Apple Silicon, Intel |
| Windows | x86_64 |

On anything else, `cargo install marklark` builds from source. It needs Rust 1.85 or later.

## Notes per channel

**PyPI.** The wheel contains the compiled binary and no Python code. It works with any Python 3.8 or
later, because Python is only the delivery vehicle. On macOS with Homebrew Python, or on Linux
distributions that protect the system Python, a global `pip install` is refused. Use `uv tool` or `pipx`,
or install into a virtualenv.

**npm.** `marklark` depends on one small package per platform (`@verhas/marklark-darwin-arm64`, …).
npm installs only the one that matches your machine, and runs no install scripts. Do not install with
`--omit=optional` or `--no-optional`, because that skips the binary. You can run it once without
installing: `npx marklark --version`.

**crates.io.** `cargo install` compiles the program, which takes a minute or two.

## Download a binary

From [GitHub Releases](https://github.com/verhas/marklark/releases/latest), take the archive for your
platform:

| Platform | Archive |
|---|---|
| Linux x86_64 | `marklark-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux aarch64 | `marklark-<version>-aarch64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `marklark-<version>-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `marklark-<version>-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `marklark-<version>-x86_64-pc-windows-msvc.zip` |

Each archive has a `.sha256` file next to it. Verify, unpack, and put the binary on your `PATH`:

```sh
shasum -a 256 -c marklark-<version>-aarch64-apple-darwin.tar.gz.sha256
tar xzf marklark-<version>-aarch64-apple-darwin.tar.gz
mv marklark-<version>-aarch64-apple-darwin/marklark ~/.local/bin/
```

**macOS:** the binaries are signed with a Developer ID certificate and notarized by Apple, so they run
without a Gatekeeper prompt. (Release 0.1.0 predates this: if macOS blocks it, run
`xattr -d com.apple.quarantine ~/.local/bin/marklark` once.)

## From source

```sh
git clone https://github.com/verhas/marklark
cd marklark
cargo install --path .
```

## Use it from an agent

After installing, give Claude Code the MCP tools, or install the guide as a skill:

```sh
claude mcp add marklark -- marklark mcp
marklark skill --user
```

## Which copy is running?

When marklark is installed in more than one way, the first one on your `PATH` wins. To find it:

```sh
which -a marklark
```

After an uninstall, `hash -r` makes the shell forget a cached location.
