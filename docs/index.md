---
title: TekkCAM
description: TekkCAM aims to make CAM easier.
---

# TekkCAM

**TekkCAM aims to make CAM easier.** It is a desktop computer-aided manufacturing (CAM) application for preparing CNC work from 2D designs and STEP models. Import Gerber, Excellon, DXF, SVG, or STEP files, create and simulate toolpaths, and export G-code.

TekkCAM includes a graphical desktop application and an offline command-line tool. The CLI helps inspect projects and generate G-code; it does not connect to or move a machine.

## Download

- [Download the latest release and installers](https://github.com/Risbern21/TekkCAM/releases/latest)
- [Browse all releases](https://github.com/Risbern21/TekkCAM/releases)
- [View build and installer workflows](https://github.com/Risbern21/TekkCAM/actions)

Release installers are available for Windows x64, macOS (Apple Silicon and Intel), and Linux x64. Choose the file for your system from the latest release:

- **Windows:** run the `.exe` setup installer.
- **macOS:** open the `.dmg` file and copy TekkCAM to Applications.
- **Debian or Ubuntu:** install the `.deb` package, for example with `sudo apt install ./tekkcam*.deb`.
- **Other Linux distributions:** make the `.AppImage` executable, then launch it.

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
