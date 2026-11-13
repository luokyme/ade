<p align="center"><a href="README.md">English</a> | 中文</p>

<div align="center">
  <img src="crates/ade/resources/app-icon.png" alt="Ade" width="128">
  <h1>Ade</h1>
  <p>快速的原生代码编辑器和智能体工作区。</p>
  <p>用 Rust 编写，开箱即用。</p>
  <p><a href="#下载"><strong>下载</strong></a></p>
</div>

---

Ade 是一款用 Rust 编写的快速原生代码编辑器，界面简洁现代，并内置了编程智能体。Ade 是 [Zed](https://github.com/zed-industries/zed) 的分支，但已经和它有了很大的不同。

## 功能

- ⚡ **原生、快速。** 不用 Electron，也没有网页视图。打开就能用，大型视图下也一直流畅。
- 🍃 **轻量。** Ade 占用的内存比 VS Code 和 Zed 都少。查看[测试结果](https://github.com/dstrml/ade/blob/main/docs/memory-benchmark.md)。
- 👾 **内置编程智能体。** 直接支持 Claude Code、Codex 和 OpenCode。
- ✨ **默认就好看。** 外观简洁现代，用起来也顺手。
- 🤫 **无需账号，没有遥测。** 不用登录任何东西。Ade 从不把你的使用数据发送到任何地方。

## 下载

在 macOS 和 Linux 上运行：

```sh
curl -fsSL https://ade.dstrml.org/install.sh | sh
```

Windows 用户请下载[安装程序](https://github.com/dstrml/ade/releases/latest/download/Ade-x86_64.exe)。

<details>
<summary>或者手动安装</summary>

- [macOS（Apple 芯片）](https://github.com/dstrml/ade/releases/latest/download/Ade-aarch64.dmg)
- [macOS（Intel 芯片）](https://github.com/dstrml/ade/releases/latest/download/Ade-x86_64.dmg)
- [Windows x86-64](https://github.com/dstrml/ade/releases/latest/download/Ade-x86_64.exe)
- [Linux x86-64](https://github.com/dstrml/ade/releases/latest/download/ade-linux-x86_64.tar.gz)
- [Linux AArch64](https://github.com/dstrml/ade/releases/latest/download/ade-linux-aarch64.tar.gz)

更多内容请看 [INSTALL.md](https://github.com/dstrml/ade/blob/main/docs/INSTALL.md)。
</details>

---

![Ade 截图](https://ade.dstrml.org/screenshot.png)

## 文档

请看[文档](https://ade.dstrml.org/zh/)。

## 构建

Ade 的构建方式和 Zed 一样。请看 [Zed 开发文档](https://zed.dev/docs/development)。

## 许可证

Ade 采用 GPL-3.0-or-later 许可证，标注的部分采用 Apache-2.0。请看 [LICENSE-GPL](./LICENSE-GPL) 和 [LICENSE-APACHE](./LICENSE-APACHE)。

## 致谢

感谢 Zed 团队打造了这样出色的编辑器，并把它开源。没有他们的工作，就不会有 Ade。
