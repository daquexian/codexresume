# Releasing `codexresume`

## Before the first public release

- Create the public repository.
- Add the repository URL to `Cargo.toml`.
- Review `README.md` and `README.zh-CN.md` for any machine-specific paths.

## Release checklist

```bash
cargo fmt
cargo test
cargo package
cargo publish --dry-run
```

If those pass:

```bash
git tag v0.1.0
cargo publish
```

## Notes

- `codexresume` intentionally does not depend on Codex Rust crates.
- It reads `config.toml`, `session_index.jsonl`, and `state_*.sqlite` directly.
- If Codex changes the SQLite schema in the future, test against a recent local
  `~/.codex` before publishing a new release.
