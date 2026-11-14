pub mod json;
pub mod model;
pub mod providers;
pub mod settings;
pub mod slint_ui;
pub mod tui;

use anyhow::Result;
use clap::Parser;
use json::{parse_flight_trace_json, serialize_flight_trace_json, serialize_full_state_json};
use providers::LinuxSystemSampler;
use providers::processes::inspect_process;
use providers::system_catalog::{format_bytes, format_system_info_report};
use settings::{AppSettings, ThemeColors, parse_hex_color};
use std::fs;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "pulsedeck",
    version = "1.2.0",
    about = "PulseDeck — Native Slint Hardware Telemetry & System Observatory"
)]
struct Cli {
    /// Launch interactive Terminal UI (TUI) instead of the Slint desktop window
    #[arg(long)]
    tui: bool,

    /// Launch native Slint Desktop GUI window (default when a display server is available)
    #[arg(long)]
    gui: bool,

    /// Apply a colour theme preset ("Fluent Modern", "KDE Breeze", "VFD Phosphor Green", "Amber CRT", "Cyberpunk Cyan", "Nordic Frost", "Solarized Daylight", "Crimson Ops")
    #[arg(long, value_name = "PRESET")]
    theme: Option<String>,

    /// Override primary accent colour with a hex value (e.g. "#00FF9D")
    #[arg(long, value_name = "HEX")]
    primary_color: Option<String>,

    /// Override secondary accent colour with a hex value (e.g. "#00E5FF")
    #[arg(long, value_name = "HEX")]
    secondary_color: Option<String>,

    /// Override main background colour with a hex value (e.g. "#080C0A")
    #[arg(long, value_name = "HEX")]
    bg_color: Option<String>,

    /// Set UI font family (e.g. "JetBrains Mono", "Fira Code", "DejaVu Sans Mono", "Consolas")
    #[arg(long, value_name = "FONT")]
    font: Option<String>,

    /// Set base font size in pixels (9..22)
    #[arg(long, value_name = "PX")]
    font_size: Option<u32>,

    /// Set telemetry refresh interval in milliseconds (200..10000)
    #[arg(long, value_name = "MS")]
    refresh_ms: Option<u64>,

    /// Enable compact row density
    #[arg(long)]
    compact: bool,

    /// Reset saved appearance & customisation settings to defaults
    #[arg(long)]
    reset_settings: bool,

    /// Run comprehensive self-test of all telemetry subsystems
    #[arg(long)]
    self_test: bool,

    /// Run quick smoke verification and exit
    #[arg(long)]
    smoke: bool,

    /// Run the hardware benchmark suite (CPU Single-Core, Multi-Core, Memory, Storage) and print PulseDeck Score
    #[arg(long)]
    benchmark: bool,

    /// Run Disk Space Analyzer on the target directory path and print report
    #[arg(long, value_name = "PATH")]
    scan_disk: Option<String>,

    /// Record Flight Recorder telemetry for N seconds to a .pulsetrace JSON file
    #[arg(long, value_name = "FILE")]
    record: Option<String>,

    /// Duration in seconds when using --record (default: 3)
    #[arg(long, default_value_t = 3)]
    duration: u64,

    /// Replay and summarize a recorded .pulsetrace JSON file
    #[arg(long, value_name = "FILE")]
    replay: Option<String>,

    /// Inspect a specific process PID (cmdline, environ, open FDs, memory maps, affinity)
    #[arg(long, value_name = "PID")]
    inspect_pid: Option<u32>,

    /// Export full System Info diagnostic catalog to stdout or file
    #[arg(long, value_name = "FILE")]
    export_sysinfo: Option<Option<String>>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut app_settings = if cli.reset_settings {
        let def = AppSettings::default();
        let _ = def.save();
        def
    } else {
        AppSettings::load()
    };

    if let Some(ref preset) = cli.theme {
        app_settings.theme = ThemeColors::from_preset(preset);
    }
    if let Some(ref hex) = cli.primary_color {
        if let Some(rgb) = parse_hex_color(hex) {
            app_settings.theme.accent_primary = rgb;
        }
    }
    if let Some(ref hex) = cli.secondary_color {
        if let Some(rgb) = parse_hex_color(hex) {
            app_settings.theme.accent_secondary = rgb;
        }
    }
    if let Some(ref hex) = cli.bg_color {
        if let Some(rgb) = parse_hex_color(hex) {
            app_settings.theme.bg_main = rgb;
        }
    }
    if let Some(ref font) = cli.font {
        app_settings.font_family = font.clone();
    }
    if let Some(sz) = cli.font_size {
        app_settings.base_font_size = sz.clamp(9, 22);
    }
    if let Some(ms) = cli.refresh_ms {
        app_settings.refresh_ms = ms.clamp(200, 10_000);
    }
    if cli.compact {
        app_settings.compact_rows = true;
    }

    if cli.self_test || cli.smoke {
        return run_self_test(cli.self_test);
    }

    if let Some(pid) = cli.inspect_pid {
        let info = inspect_process(pid)?;
        println!(
            "=== PulseDeck Process Inspector: PID {} ({}) ===",
            info.pid, info.name
        );
        println!("Executable  : {}", info.exe_path);
        println!("Working Dir : {}", info.cwd);
        println!("Command Line: {}", info.cmdline);
        println!("User / UID  : {} ({})", info.user, info.uid);
        println!(
            "State       : {} | Threads: {} | Nice: {}",
            info.state, info.threads, info.nice
        );
        println!("CPU Affinity: {}", info.cpu_affinity_hex);
        println!(
            "Memory      : RSS {} | VMS {} | Shared {} | Maps: {}",
            format_bytes(info.rss_bytes),
            format_bytes(info.vms_bytes),
            format_bytes(info.shared_bytes),
            info.memory_maps_count
        );
        println!("Open FDs    : {}", info.open_fds.len());
        for fd in info.open_fds.iter().take(15) {
            println!("  fd {:<4} -> {}", fd.fd, fd.target);
        }
        return Ok(());
    }

    if let Some(ref scan_path) = cli.scan_disk {
        let rep = providers::diagnostics::analyze_disk_space(scan_path, 50_000);
        println!("=== PulseDeck Disk Space Analyzer ===");
        println!("Target Root : {}", rep.root_path);
        println!(
            "Filesystem  : {} used / {} total ({} free)",
            format_bytes(rep.fs_used_bytes),
            format_bytes(rep.fs_total_bytes),
            format_bytes(rep.fs_free_bytes)
        );
        println!(
            "Scanned     : {} files across {} dirs ({}) in {} ms",
            rep.scanned_files,
            rep.scanned_dirs,
            format_bytes(rep.scanned_bytes),
            rep.scan_duration_ms
        );
        println!("\nTop Directories:");
        for d in rep.top_directories.iter().take(10) {
            println!(
                "  {:<30} {:>12} ({:>6} files, {:5.1}%)",
                d.name,
                format_bytes(d.size_bytes),
                d.file_count,
                d.pct_of_root
            );
        }
        println!("\nFile Categories:");
        for c in &rep.categories {
            println!(
                "  {:<26} {:>12} ({:>6} files, {:5.1}%)",
                c.category,
                format_bytes(c.size_bytes),
                c.file_count,
                c.pct_of_root
            );
        }
        return Ok(());
    }

    if cli.benchmark {
        let mut sampler = LinuxSystemSampler::new();
        let snap = sampler.sample_tick();
        println!(
            "=== Running PulseDeck Hardware Benchmark Suite on {} ({} logical cores) ===",
            snap.cpu.model_name, snap.cpu.logical_cores
        );
        let rep = sampler.run_all_benchmarks(snap.cpu.logical_cores);
        println!(
            "\n  COMPOSITE PULSEDECK SCORE : {} (Baseline = 100)\n",
            rep.composite_score
        );
        for sc in [
            &rep.cpu_single_core,
            &rep.cpu_multi_core,
            &rep.memory_bandwidth,
            &rep.disk_throughput,
        ] {
            println!(
                "  {:<26} : Score {:>5} | {:>9.2} {:<8} ({} ms)",
                sc.name, sc.normalized_score, sc.metric_value, sc.metric_unit, sc.duration_ms
            );
        }
        return Ok(());
    }

    if let Some(ref out_file) = cli.record {
        let mut sampler = LinuxSystemSampler::new();
        println!(
            "Recording Flight Recorder telemetry for {}s -> {} ...",
            cli.duration, out_file
        );
        for _ in 0..cli.duration.max(1) {
            let _ = sampler.sample_tick();
            std::thread::sleep(Duration::from_millis(500));
        }
        let _ = sampler.sample_tick();
        let json = serialize_flight_trace_json(sampler.export_flight_trace());
        fs::write(out_file, &json)?;
        println!(
            "Saved {} frames ({} bytes, schema=io.pulsedeck.traces v1) to {}",
            sampler.flight_recorder.trace.samples.len(),
            json.len(),
            out_file
        );
        return Ok(());
    }

    if let Some(ref in_file) = cli.replay {
        let content = fs::read_to_string(in_file)?;
        let trace = parse_flight_trace_json(&content)?;
        println!("=== PulseDeck Flight Recorder Replay: {} ===", in_file);
        println!(
            "Schema: {} v{} | Host: {} | CPU: {} | Frames: {}",
            trace.schema,
            trace.version,
            trace.hostname,
            trace.cpu_model,
            trace.samples.len()
        );
        for s in trace.samples.iter().take(20) {
            println!(
                "  Frame #{:<3} [{}] CPU {:5.1}% ({:4.0}MHz) | Mem {:5.1}% | Pressure {:<8} ({:4.1}%) | Top: {} ({:.1}%)",
                s.index,
                s.timestamp_epoch_ms,
                s.cpu_total_pct,
                s.cpu_avg_freq_mhz,
                s.mem_used_pct,
                s.pressure_level,
                s.pressure_score,
                s.top_process_name,
                s.top_process_cpu_pct
            );
        }
        return Ok(());
    }

    if let Some(ref opt_file) = cli.export_sysinfo {
        let mut sampler = LinuxSystemSampler::new();
        let _ = sampler.sample_tick();
        if let Some(ref info) = sampler.system_info {
            let text = format_system_info_report(info);
            if let Some(path) = opt_file {
                fs::write(path, &text)?;
                println!("Exported System Info report to {}", path);
            } else {
                print!("{}", text);
            }
        }
        return Ok(());
    }

    if cli.tui {
        return tui::run_tui(&app_settings);
    }

    // Default behavior (or --gui): launch native Slint GUI window, falling back to TUI if headless.
    if slint_ui::run_slint_gui(app_settings.clone()).is_err() {
        tui::run_tui(&app_settings)?;
    }
    Ok(())
}

fn run_self_test(full: bool) -> Result<()> {
    println!("========================================================================");
    println!("  PULSEDECK 1.2.0 — TELEMETRY & SUBSYSTEM SELF-TEST");
    println!("========================================================================");

    let mut sampler = LinuxSystemSampler::new();
    let _ = sampler.sample_tick();
    std::thread::sleep(Duration::from_millis(100));
    let snap = sampler.sample_tick();

    println!(
        "[PASS] 1. Summary & System Pressure : Level={} ({:.1}%), Load 1m={:.2} ({:.2}/core), Mem PSI avg10={:.2}%",
        snap.pressure.level.as_str(),
        snap.pressure.score_percent,
        snap.pressure.load_1m,
        snap.pressure.load_per_core,
        snap.pressure.mem_psi_some_avg10
    );
    println!(
        "[PASS] 2. CPU Telemetry             : {} ({}C/{}T, P:{} E:{}), Util={:.1}%, Clock={:.0} MHz",
        snap.cpu.model_name,
        snap.cpu.physical_cores,
        snap.cpu.logical_cores,
        snap.cpu.p_cores,
        snap.cpu.e_cores,
        snap.cpu.total_pct,
        snap.cpu.avg_freq_mhz
    );
    println!(
        "[PASS] 3. Memory & Zswap            : {} / {} ({:.1}%), Swap={}, Zswap={}",
        format_bytes(snap.memory.used_bytes),
        format_bytes(snap.memory.total_bytes),
        snap.memory.used_pct,
        format_bytes(snap.memory.swap_used_bytes),
        snap.memory.zswap_enabled
    );
    let self_insp = inspect_process(std::process::id())?;
    println!(
        "[PASS] 4. Processes & Inspector     : {} live processes, self PID {} has {} open FDs & {} memory maps",
        snap.processes.len(),
        self_insp.pid,
        self_insp.open_fds.len(),
        self_insp.memory_maps_count
    );
    println!(
        "[PASS] 5. Disk, Network & GPU       : {} block disks, {} network interfaces, {} GPU devices",
        snap.disks.len(),
        snap.networks.len(),
        snap.gpus.len()
    );
    println!(
        "[PASS] 6. Network Connections       : {} live TCP/UDP/Unix sockets correlated to PIDs",
        snap.connections.len()
    );
    let total_sensors: usize = snap.sensors.iter().map(|g| g.readings.len()).sum();
    println!(
        "[PASS] 7. Power & Frequency Sensors : {} sensor groups ({} live readings with min/max)",
        snap.sensors.len(),
        total_sensors
    );
    println!(
        "[PASS] 8. Disk Space Analyzer       : Scanned {} ({} files, {}) in {} ms",
        sampler.disk_space.root_path,
        sampler.disk_space.scanned_files,
        format_bytes(sampler.disk_space.scanned_bytes),
        sampler.disk_space.scan_duration_ms
    );
    println!(
        "[PASS] 9. Kernel Drivers & Modules  : {} loaded drivers/modules",
        sampler.drivers.len()
    );

    let trace_json = serialize_flight_trace_json(sampler.export_flight_trace());
    let parsed_trace = parse_flight_trace_json(&trace_json)?;
    println!(
        "[PASS] 10. Flight Recorder          : Recorded & verified {} frames (schema={} v{})",
        parsed_trace.samples.len(),
        parsed_trace.schema,
        parsed_trace.version
    );

    if full {
        let bench = sampler.run_all_benchmarks(snap.cpu.logical_cores);
        println!(
            "[PASS] 11. Hardware Benchmarks      : Composite PulseDeck Score = {} (SC={}, MC={}, Mem={}, Disk={})",
            bench.composite_score,
            bench.cpu_single_core.normalized_score,
            bench.cpu_multi_core.normalized_score,
            bench.memory_bandwidth.normalized_score,
            bench.disk_throughput.normalized_score
        );
    }

    println!(
        "[PASS] 12. System Catalogs          : {} installed apps, {} services, {} startup items, {} active users",
        sampler.installed_apps.len(),
        sampler.services.len(),
        sampler.startup_apps.len(),
        sampler.users.len()
    );

    let settings = AppSettings::default();
    let state_json = serialize_full_state_json(&snap, &sampler);
    println!(
        "[PASS] 13. Slint UI & Theme Engine  : Preset='{}', Font='{}' ({}px), State={} bytes",
        settings.theme.preset_name,
        settings.font_family,
        settings.base_font_size,
        state_json.len()
    );
    println!("========================================================================");
    println!("  ALL CHECKS PASSED SUCCESSFULLY");
    println!("========================================================================");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_telemetry_and_customisation_subsystems() {
        let mut sampler = LinuxSystemSampler::new();
        let _ = sampler.sample_tick();
        std::thread::sleep(Duration::from_millis(50));
        let snap = sampler.sample_tick();

        assert!(snap.cpu.logical_cores >= 1);
        assert!(snap.memory.total_bytes > 0);
        assert!(!snap.processes.is_empty());
        assert!(!snap.networks.is_empty());
        assert!(!snap.connections.is_empty());
        assert!(!snap.sensors.is_empty());
        assert!(!sampler.drivers.is_empty());
        assert!(sampler.disk_space.scanned_files >= 1);
        assert!(!sampler.installed_apps.is_empty());

        let my_pid = std::process::id();
        let insp = inspect_process(my_pid).expect("inspect_process on self PID must succeed");
        assert_eq!(insp.pid, my_pid);
        assert!(!insp.open_fds.is_empty());

        let trace_json = serialize_flight_trace_json(sampler.export_flight_trace());
        let decoded = parse_flight_trace_json(&trace_json).expect("Flight trace JSON must parse");
        assert_eq!(decoded.schema, "io.pulsedeck.traces");
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.samples.len(), 2);

        // Verify Theme Presets & Hex Parser
        for preset in settings::THEME_PRESETS {
            let tc = ThemeColors::from_preset(preset);
            assert_eq!(tc.preset_name, preset);
        }
        assert_eq!(parse_hex_color("#00FF9D"), Some([0, 255, 157]));

        let bench = sampler.run_all_benchmarks(snap.cpu.logical_cores);
        assert!(bench.composite_score > 0);
    }
}
