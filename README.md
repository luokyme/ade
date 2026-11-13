<p align="center">English | <a href="README.zh-CN.md">中文</a></p>

<div align="center">
  <img src="crates/ade/resources/app-icon.png" alt="Ade" width="128">
  <h1>Ade</h1>
  <p>The fast, native code editor and agent workspace.</p>
  <p>Written in Rust. Batteries included.</p>
  <p><a href="#download"><strong>Download</strong></a></p>
</div>

---

Ade is a fast, native code editor written in Rust, with a clean, modern design and coding agents built in. Ade is a fork of [Zed](https://github.com/zed-industries/zed) but diverges heavily from it.

## Features

- ⚡ **Native and fast.** No Electron or webviews. Opens instantly and stays responsive on large views.
- 🍃 **Lightweight.** Ade's memory footprint is lower than VS Code and Zed. See [benchmarks](https://github.com/luokyme/ade/blob/main/docs/memory-benchmark.md).
- 👾 **Coding agents included.** Built-in support for Claude Code, Codex, and OpenCode.
- ✨ **Beautiful by default.** Clean, modern look with a delightful UX.
- 🤫 **No account or telemetry.** Nothing to sign in to. Ade never sends your usage data anywhere.

## Download

On macOS and Linux, run:

```sh
curl -fsSL https://luokyme.github.io/ade/install.sh | sh
```

For Windows, download the [installer](https://github.com/luokyme/ade/releases/latest/download/Ade-x86_64.exe).

<details>
<summary>Or install manually</summary>

- [macOS (Apple Silicon)](https://github.com/luokyme/ade/releases/latest/download/Ade-aarch64.dmg)
- [macOS (Intel)](https://github.com/luokyme/ade/releases/latest/download/Ade-x86_64.dmg)
- [Windows x86-64](https://github.com/luokyme/ade/releases/latest/download/Ade-x86_64.exe)
- [Linux x86-64](https://github.com/luokyme/ade/releases/latest/download/ade-linux-x86_64.tar.gz)
- [Linux AArch64](https://github.com/luokyme/ade/releases/latest/download/ade-linux-aarch64.tar.gz)

See [INSTALL.md](https://github.com/luokyme/ade/blob/main/docs/INSTALL.md) for more.
</details>

---

![Ade screenshot](https://luokyme.github.io/ade/screenshot.png)

## Docs

See [docs](https://luokyme.github.io/ade).

## Building

Ade builds the same way Zed does. See the [Zed development docs](https://zed.dev/docs/development).

## Licensing

Ade is licensed under GPL-3.0-or-later with Apache-2.0 components where marked. See [LICENSE-GPL](./LICENSE-GPL) and [LICENSE-APACHE](./LICENSE-APACHE).

## Acknowledgements

Thanks to the Zed team for building an excellent editor and releasing it as open source. Ade would not exist without their work.
