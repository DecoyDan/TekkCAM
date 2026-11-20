---
title: TekkCAM
description: TekkCAM aims to make CAM easier.
---

# TekkCAM

**TekkCAM aims to make CAM easier.** It is a desktop computer-aided manufacturing (CAM) application for preparing CNC work from 2D designs and STEP models. Import Gerber, Excellon, DXF, SVG, or STEP files, create and simulate toolpaths, and export G-code.

TekkCAM includes a graphical desktop application and an offline command-line tool. The CLI helps inspect projects and generate G-code; it does not connect to or move a machine.

## Download

These links download the files directly from the latest GitHub release:

| Platform | Installer | Portable executables |
| --- | --- | --- |
| Windows x64 | [Setup installer](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-gui_0.1.7_x64-setup.exe) | [ZIP archive](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-v0.1.7-windows-x86_64.zip) |
| macOS Apple Silicon | [DMG installer](https://github.com/Risbern21/TekkCAM/releases/latest/download/TekkCAM_0.1.7_aarch64.dmg) | [Tarball](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-v0.1.7-macos-aarch64.tar.gz) |
| macOS Intel | [DMG installer](https://github.com/Risbern21/TekkCAM/releases/latest/download/TekkCAM_0.1.7_x64.dmg) | [Tarball](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-v0.1.7-macos-x86_64.tar.gz) |
| Debian or Ubuntu x64 | [DEB package](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-gui_0.1.7_amd64.deb) | [Tarball](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-v0.1.7-linux-x86_64.tar.gz) |
| Other Linux x64 | [AppImage](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-gui_0.1.7_x86_64.AppImage) | [Tarball](https://github.com/Risbern21/TekkCAM/releases/latest/download/tekkcam-v0.1.7-linux-x86_64.tar.gz) |

The portable archives contain both the desktop app and CLI. Installers also include both. See the [full installation guide](INSTALLATION.md) for Linux serial-port permissions and local packaging details. [Browse all releases](https://github.com/Risbern21/TekkCAM/releases) if you need an older version.

Installers include both the desktop app and CLI. See the [full installation guide](INSTALLATION.md) for details, including Linux serial-port permissions and local packaging.

## Build from source

Install [Rust](https://rustup.rs/) and the native build tools and libraries for your operating system. Then:

```sh
git clone https://github.com/Risbern21/TekkCAM.git
cd TekkCAM
cargo build --release --bins
```

Launch the desktop app with `./target/release/tekkcam-gui` on Linux or macOS, or `target/release/tekkcam-gui.exe` on Windows. See the [README build instructions](../README.md#build-from-source) for platform dependencies and more commands.

## Project links

- [Source code](https://github.com/Risbern21/TekkCAM)
- [CLI guide](CLI.md)
- [CAM workflow and capabilities](CAM_WORKFLOW.md)
- [Safety notes and limitations](SAFETY.md)
