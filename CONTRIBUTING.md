# Contributing

Thanks for helping. Bug reports, ideas and pull requests are welcome.

## Setup

Windows 10/11, stable Rust (MSVC toolchain) and the Visual Studio Build Tools. See
[Building from source](README.md#building-from-source).

## Before you open a pull request

Run the same checks as CI:

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

- Keep changes small and focused; one pull request per change.
- `rustfmt.toml` sets the line width to 110.
- Do not change the update signing or verification flow without opening an issue first.
- Describe what changed and how you tested it in the pull request.

Questions and ideas are welcome in Discussions. Please report security problems as described in
[SECURITY.md](SECURITY.md).
