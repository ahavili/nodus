# Contributing to Nodus

## Development Workflow

Nodus is a Rust CLI. Use the standard Cargo workflow while iterating:

```bash
cargo check
cargo test
```

Before opening a pull request or publishing a release, run the full local preflight:

```bash
bash scripts/rust_ci_preflight.sh
```

That script mirrors the repository CI and checks:

- formatting with `cargo fmt --all --check`
- lints with `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- tests with `cargo test --workspace --all-targets`
- packaging with `cargo package --locked`

## Release Notes

For a crates.io release:

1. Run `bash scripts/rust_ci_preflight.sh`.
2. Run `cargo publish --dry-run`.
3. Confirm the package metadata in `Cargo.toml` still reflects the release.
4. Publish with `cargo publish`.
