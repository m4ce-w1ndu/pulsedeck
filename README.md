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
