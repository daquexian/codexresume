# codexresume

[English](README.md) | [简体中文](README.zh-CN.md)

## 为什么做这个

当前 `codex resume` 会受到 provider、source、cwd 等因素影响，
导致一部分 session 在 `codex resume` 里[看不到](https://dev.to/vild_da_f524590ed3ae13840/why-codex-history-disappears-after-switching-providers-and-how-i-fixed-it-f0j)。
这个工具通过直接读取本地 session 元数据来绕过问题，同时尽量保留接近原版 `codex resume` 的交互体验。可以无脑把它当作 codex resume 来用。

## 特性

- 默认展示所有 provider 的 session（不会按默认 provider 过滤）
- 可选 `--only-openai`，只显示内建 OpenAI provider
- 支持常用的 `codex resume` 参数（`--last`、`--all`、`--include-non-interactive`）

当你在 TUI 里选中某个 session 并按回车后，它会执行：

```bash
codex resume <SESSION_ID> <所有转发参数>
```

## 安装

从 crates.io 安装：

```bash
cargo install codexresume
```

或者本地源码编译安装

```bash
git clone https://github.com/daquexian/codexresume
cd codexresume
cargo install --path .
```

## 使用方法

如果已经安装，可以直接执行：

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

## 备注

- `--only-openai` 是 `codexresume` 自己的参数，不会转发给 `codex resume`。
- `--include-archived`、`--codex-home` 和 `--sqlite-home` 也是 `codexresume`
  自己的参数，不会转发给 `codex resume`。
- `codexresume` 会尽量贴近原版 `codex resume` 的体验，但它并没有直接复用
  Codex 内部 UI 组件，所以不会做到像素级一致。
- `codexresume` 不依赖 Codex 的 Rust crate，而是直接读取 `config.toml`、
  `session_index.jsonl` 和 `state_*.sqlite`。
- 如果你传了 `--remote`，`codexresume` 会直接透传给原版 `codex resume`
  （remote session 不是本地可发现的数据）。
- 如果 session index 里有线程名，`codexresume` 会尽量把名字补出来。

如果你想让 `codex resume` 自动走 `codexresume`，可以在 `~/.bashrc` 或 `~/.zshrc` 里添加：

```bash
codex() {
  if [ "$1" = "resume" ]; then shift; command codexresume "$@"; else command codex "$@"; fi
}
```
