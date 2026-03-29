# codexresume

[English](README.md) | [简体中文](README.zh-CN.md)

## Why this exists

Current `codex resume` can be affected by provider, source, cwd, and other
factors, which can make some sessions [disappear from the history
picker](https://dev.to/vild_da_f524590ed3ae13840/why-codex-history-disappears-after-switching-providers-and-how-i-fixed-it-f0j).
This tool works around that by reading local session metadata directly, while
trying to keep an experience close to the stock `codex resume` flow.
You can use it as a drop-in replacement for `codex resume`.

## Features

- Lists sessions across all providers by default (no default-provider filtering)
- Optional `--only-openai` flag for the built-in OpenAI provider
- Supports the common `codex resume` flags (`--last`, `--all`, `--include-non-interactive`)

After you pick a session and press `Enter`, it `exec`s:

```bash
codex resume <SESSION_ID> <all forwarded args>
```

## Installation

Install from crates.io:

```bash
cargo install codexresume
```

Or clone and install from source:

```bash
git clone https://github.com/daquexian/codexresume
cd codexresume
cargo install --path .
```

## Usage

If installed, run it directly:

```bash
codexresume --last
codexresume --yolo
codexresume --only-openai --yolo
codexresume --all
codexresume --include-non-interactive
codexresume --codex-home ~/.codex --sqlite-home ~/.codex --yolo
codexresume -C /path/to/project --model gpt-5.4
codexresume --dangerously-bypass-approvals-and-sandbox
```

## Notes

- `--only-openai` is a `codexresume` flag. It is consumed by the
  wrapper and is not forwarded to `codex resume`.
- `--include-archived`, `--codex-home`, and `--sqlite-home` are `codexresume`
  flags. They are consumed by the wrapper and are not forwarded to `codex resume`.
- `codexresume` tries to stay close to the stock `codex resume` experience,
  but it does not directly reuse Codex's internal UI components, so it is not
  pixel-identical.
- `codexresume` does not depend on Codex Rust crates. It reads `config.toml`,
  `session_index.jsonl`, and `state_*.sqlite` directly.
- If you pass `--remote`, `codexresume` will pass through to the stock `codex resume`
  implementation (remote sessions are not discoverable locally).
- If session names are available in the session index, `codexresume` will try
  to surface them.

To make `codex resume` use `codexresume` automatically, add this to `~/.bashrc` or `~/.zshrc`:

```bash
codex() {
  if [ "$1" = "resume" ]; then shift; command codexresume "$@"; else command codex "$@"; fi
}
```
