---
title: Installation
description: Install Bushido via curl, Homebrew, Cargo, or from source, and verify the correct version
sidebar:
  order: 1
---

# Installation

## Check before installing

```bash
bdo --version   # should print: bdo x.y.z
bdo gain        # should show token savings stats
```

If both commands work, Bushido is already installed. Skip to [Project initialization](#project-initialization).

## Quick install (Linux and macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/tedorigawa001/TokenReductionTool/master/install.sh | sh
```

## Homebrew (macOS and Linux)

```bash
brew install tedorigawa001/tap/bdo
```

## Cargo

```bash
cargo install --git https://github.com/tedorigawa001/TokenReductionTool
```

## Pre-built binaries (Windows, Linux, macOS)

Download from [GitHub releases](https://github.com/tedorigawa001/TokenReductionTool/releases):

- macOS: `bdo-x86_64-apple-darwin.tar.gz` / `bdo-aarch64-apple-darwin.tar.gz`
- Linux: `bdo-x86_64-unknown-linux-musl.tar.gz` / `bdo-aarch64-unknown-linux-gnu.tar.gz`
- Windows: `bdo-x86_64-pc-windows-msvc.zip`

**Windows users**: Extract the zip and place `bdo.exe` in a directory on your PATH. Run Bushido from Command Prompt, PowerShell, or Windows Terminal — do not double-click the `.exe` (it prints usage and exits immediately). For full hook support, use [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) instead.

## Verify installation

```bash
bdo --version   # bdo x.y.z
bdo gain        # token savings dashboard
```

## Project initialization

Run once per project to enable the Claude Code hook:

```bash
bdo init
```

For a global install that patches `settings.json` automatically:

```bash
bdo init --global
```

## Uninstall

```bash
bdo init -g --uninstall    # remove hook, Bushido.md, and settings.json entry
cargo uninstall bdo         # remove binary (if installed via Cargo)
brew uninstall bdo          # remove binary (if installed via Homebrew)
```
