# PulseDeck

**PulseDeck** is a fast, native hardware telemetry and system observatory built in Rust with **Slint** (desktop GUI) and an interactive **Terminal UI (TUI)** fallback. It runs natively on both **Linux** and **Windows**.

## Key Features

- **12 Telemetry & Diagnostic Workspaces**:
  1. **Summary**: VFD-inspired CPU, Memory, Clock, Uptime Odometer, Disk Activity, System Pressure card, and Top CPU/Memory processes.
  2. **Processes**: Live searchable process table (`List` and hierarchical `Tree` views), kernel-thread filter, process signal & priority controls (`SIGTERM`, `SIGKILL`, `SIGSTOP`, `SIGCONT`, `Nice ±`), and deep **Process Inspector** (`cmdline`, `environ`, open file descriptors, memory maps).
  3. **Performance**: Dual-series RGBA8 pixel-buffer telemetry charts (`CPU Total / Kernel`, `Memory / Swap`, `Disk Read / Write`, `Network RX / TX`) and per-core utilization/frequency/temperature bars.
  4. **Connections**: Live TCP, TCP6, UDP, UDP6, and UNIX socket table mapped to owning PID & process name, with protocol and public-listener filters.
  5. **Power & Freq**: Real-time CPU core frequency table and hardware sensor readings (temperatures, RAPL package power, fan speeds, voltages) with min/max tracking.
  6. **Disk Space**: High-speed multi-directory disk space analyzer with directory rankings, file-type category breakdown, and largest files list.
  7. **Drivers**: Loaded kernel modules and system drivers browser with live search filter.
  8. **Flight Recorder**: Multi-sample telemetry recorder that captures, scrubs, exports, and replays `.pulsetrace` JSON files (`io.pulsedeck.traces`).
  9. **Benchmarks**: Built-in Single-Core CPU, Multi-Core CPU, Memory Bandwidth, and Storage I/O benchmark suite producing a composite **PulseDeck Score**.
  10. **Apps & Services**: Installed packages, system services, startup items, application history, and active user sessions.
  11. **System Info**: Comprehensive hardware, hybrid CPU topology, storage, GPU, and OS diagnostic report.
  12. **Appearance**: Live visual customisation studio with 6 colour presets (`VFD Phosphor Green`, `Amber CRT`, `Cyberpunk Cyan`, `Nordic Frost`, `Solarized Daylight`, `Crimson Ops`), custom `#RRGGBB` hex colour pickers, font family & size controls (`9px–22px`), compact row density, graph gridlines toggle, and adjustable refresh rate (`250ms–2000ms`).

## Project Structure

- `src/main.rs` — CLI entrypoint, argument parser, and self-test suite
- `src/slint_ui.rs` — Native Slint desktop UI components, theme bindings, and pixel-buffer graph renderer
- `src/tui.rs` — Interactive terminal UI mode (`--tui`)
- `src/settings.rs` — Theme presets, custom hex colour parser, typography settings, and JSON persistence
- `src/model.rs` — Telemetry data structures
- `src/json.rs` — Zero-dependency JSON serializer and `.pulsetrace` parser
- `src/providers/cpu_mem.rs` — CPU, per-core topology, memory, swap, and system pressure collector
- `src/providers/processes.rs` — Process table, tree builder, process control, and deep PID inspector
- `src/providers/io_gpu.rs` — Block storage, network interface throughput, and GPU telemetry collector
- `src/providers/diagnostics.rs` — Live socket connections, hardware sensors, disk space scanner, kernel modules/drivers, flight recorder, and benchmarks
- `src/providers/system_catalog.rs` — Installed applications, system services, startup entries, user sessions, and system info report

## Installation

Prebuilt packages for every tagged version are on the [Releases page](https://github.com/m4ce-w1ndu/pulsedeck/releases):

| Distro | Command |
|---|---|
| Arch, CachyOS, EndeavourOS, Manjaro | `sudo pacman -U pulsedeck-*.pkg.tar.zst` |
| Debian 12+, Ubuntu 22.04+, Mint, Pop!_OS | `sudo apt install ./pulsedeck_*.deb` |
| Fedora | `sudo dnf install ./pulsedeck-*.rpm` |
| openSUSE Tumbleweed | `sudo zypper install ./pulsedeck-*.rpm` |
| Other x86_64 Linux (glibc ≥ 2.35) | extract `pulsedeck-*-x86_64-linux.tar.gz` and run `./pulsedeck` |

### Build & install from source on Arch-based distros

```bash
packaging/arch/install.sh              # build the current checkout and install it via pacman
packaging/arch/install.sh --worktree   # include uncommitted changes
packaging/arch/install.sh --release    # build the GitHub release matching Cargo.toml
packaging/arch/install.sh --uninstall  # remove it
```

### Releasing

CI (`.github/workflows/ci.yml`) builds and tests every push to `main`. To publish a release, bump
`version` in `Cargo.toml`, commit, then push a matching tag (`git tag v1.2.0 && git push origin v1.2.0`).
`.github/workflows/release.yml` builds the `.deb`, `.rpm`, Arch package and tarball, install-tests them
in clean Debian, Ubuntu, Fedora, openSUSE and Arch containers, and attaches them to a GitHub Release.

## Usage

```bash
# Launch the native Slint desktop GUI
cargo run --release

# Launch the interactive Terminal UI
cargo run --release -- --tui

# Run full telemetry & subsystem self-test
cargo run --release -- --self-test

# Run hardware benchmark suite from CLI
cargo run --release -- --benchmark
```
