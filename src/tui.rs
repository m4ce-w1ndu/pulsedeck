use crate::json::serialize_flight_trace_json;
use crate::model::{PressureLevel, SystemSnapshot};
use crate::providers::LinuxSystemSampler;
use crate::providers::processes::{ProcessAction, control_process, inspect_process};
use crate::providers::system_catalog::{format_bytes, format_system_info_report};
use crate::settings::AppSettings;
use anyhow::Result;
use std::fs;
use std::io::{Read, Write, stdout};
use std::time::{Duration, Instant};

const PAGES: [&str; 10] = [
    "1:Summary",
    "2:Processes",
    "3:Performance",
    "4:Connections",
    "5:Power&Freq",
    "6:DiskSpace",
    "7:Drivers",
    "8:FlightRec",
    "9:Benchmarks",
    "0:Catalogs/Info",
];

#[cfg(unix)]
struct RawTermGuard {
    orig: libc::termios,
}

#[cfg(unix)]
impl RawTermGuard {
    fn enter() -> Option<Self> {
        unsafe {
            let mut term: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
                return None;
            }
            let orig = term;
            term.c_lflag &= !(libc::ICANON | libc::ECHO);
            term.c_cc[libc::VMIN] = 0;
            term.c_cc[libc::VTIME] = 0;
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term);
            print!("\x1b[?1049h\x1b[?25l");
            let _ = stdout().flush();
            Some(Self { orig })
        }
    }
}

#[cfg(unix)]
impl Drop for RawTermGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
            print!("\x1b[?25h\x1b[?1049l");
            let _ = stdout().flush();
        }
    }
}

pub fn run_tui(settings: &AppSettings) -> Result<()> {
    #[cfg(unix)]
    let _guard = RawTermGuard::enter();
    let mut sampler = LinuxSystemSampler::new();
    let _ = sampler.sample_tick();
    std::thread::sleep(Duration::from_millis(60));
    let mut snap = sampler.sample_tick();

    let mut page_idx: usize = 0;
    let mut selected_row: usize = 0;
    let mut tree_view = false;
    let mut status_banner = String::from(
        "Keys: [1-0/Tab] Page [Up/Dn] Select [i] Inspect [t] Tree [x] SIGTERM [b] Bench [e] Export Trace [q] Quit",
    );
    let mut last_tick = Instant::now();
    let mut stdin = std::io::stdin();
    let mut key_buf = [0u8; 16];

    loop {
        if last_tick.elapsed() >= Duration::from_millis(settings.refresh_ms) {
            snap = sampler.sample_tick();
            last_tick = Instant::now();
        }

        render_frame(
            &snap,
            &sampler,
            page_idx,
            selected_row,
            tree_view,
            &status_banner,
            settings,
        )?;

        std::thread::sleep(Duration::from_millis(40));
        let n = stdin.read(&mut key_buf).unwrap_or(0);
        if n > 0 {
            match &key_buf[..n] {
                b"q" | b"Q" | [3] => break,
                b"1" => { page_idx = 0; selected_row = 0; }
                b"2" => { page_idx = 1; selected_row = 0; }
                b"3" => { page_idx = 2; selected_row = 0; }
                b"4" => { page_idx = 3; selected_row = 0; }
                b"5" => { page_idx = 4; selected_row = 0; }
                b"6" => { page_idx = 5; selected_row = 0; }
                b"7" => { page_idx = 6; selected_row = 0; }
                b"8" => { page_idx = 7; selected_row = 0; }
                b"9" => { page_idx = 8; selected_row = 0; }
                b"0" => { page_idx = 9; selected_row = 0; }
                b"\t" => { page_idx = (page_idx + 1) % PAGES.len(); selected_row = 0; }
                b"\x1b[Z" => { page_idx = (page_idx + PAGES.len() - 1) % PAGES.len(); selected_row = 0; }
                b"j" | b"\x1b[B" => { selected_row = selected_row.saturating_add(1); }
                b"k" if page_idx != 1 => { selected_row = selected_row.saturating_sub(1); }
                b"\x1b[A" => { selected_row = selected_row.saturating_sub(1); }
                b"t" | b"T" => {
                    tree_view = !tree_view;
                    status_banner = format!("Process Tree View: {}", if tree_view { "ON" } else { "OFF" });
                }
                b"i" | b"I" => {
                    if let Some(p) = snap.processes.get(selected_row) {
                        match inspect_process(p.pid) {
                            Ok(info) => {
                                status_banner = format!(
                                    "Inspect PID {} ({}): exe={} cwd={} threads={} fds={} maps={} affinity={}",
                                    info.pid,
                                    info.name,
                                    info.exe_path,
                                    info.cwd,
                                    info.threads,
                                    info.open_fds.len(),
                                    info.memory_maps_count,
                                    info.cpu_affinity_hex
                                );
                            }
                            Err(e) => status_banner = format!("Inspect error: {}", e),
                        }
                    }
                }
                b"x" | b"X" => {
                    if let Some(p) = snap.processes.get(selected_row) {
                        match control_process(p.pid, ProcessAction::Terminate) {
                            Ok(msg) => status_banner = msg,
                            Err(e) => status_banner = format!("Signal error: {}", e),
                        }
                    }
                }
                b"b" | b"B" => {
                    status_banner = String::from(
                        "Running hardware benchmarks (Single-Core, Multi-Core, RAM, Disk)...",
                    );
                    render_frame(
                        &snap,
                        &sampler,
                        page_idx,
                        selected_row,
                        tree_view,
                        &status_banner,
                        settings,
                    )?;
                    let rep = sampler.run_all_benchmarks(snap.cpu.logical_cores);
                    status_banner = format!(
                        "Benchmarks Complete! Composite PulseDeck Score: {} (SC: {}, MC: {}, Mem: {}, Disk: {})",
                        rep.composite_score,
                        rep.cpu_single_core.normalized_score,
                        rep.cpu_multi_core.normalized_score,
                        rep.memory_bandwidth.normalized_score,
                        rep.disk_throughput.normalized_score
                    );
                    page_idx = 8;
                }
                b"e" | b"E" => {
                    let json = serialize_flight_trace_json(sampler.export_flight_trace());
                    let out_path = "pulsedeck-capture.pulsetrace";
                    if fs::write(out_path, json).is_ok() {
                        status_banner = format!(
                            "Exported {} Flight Recorder frames to {}",
                            sampler.flight_recorder.trace.samples.len(),
                            out_path
                        );
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn render_frame(
    snap: &SystemSnapshot,
    sampler: &LinuxSystemSampler,
    page_idx: usize,
    selected_row: usize,
    tree_view: bool,
    status_banner: &str,
    settings: &AppSettings,
) -> Result<()> {
    let mut out = String::with_capacity(16384);
    out.push_str("\x1b[H\x1b[2J");

    let [pr, pg, pb] = settings.theme.accent_primary;
    let [sr, sg, sb] = settings.theme.accent_secondary;
    let [wr, wg, wb] = settings.theme.accent_warn;
    let [dr, dg, db] = settings.theme.accent_danger;

    let p_color = match snap.pressure.level {
        PressureLevel::Low => format!("\x1b[38;2;{};{};{}m", pr, pg, pb),
        PressureLevel::Elevated => format!("\x1b[38;2;{};{};{}m", sr, sg, sb),
        PressureLevel::High => format!("\x1b[38;2;{};{};{}m", wr, wg, wb),
        PressureLevel::Critical => format!("\x1b[38;2;{};{};{}m", dr, dg, db),
    };
    out.push_str(&format!(
        "\x1b[48;2;13;24;19m\x1b[38;2;{};{};{}m\x1b[1m PULSEDECK // SYSTEM OBSERVATORY \x1b[0m\x1b[48;2;13;24;19m \
         CPU: \x1b[38;2;{};{};{}m{:5.1}%\x1b[0m\x1b[48;2;13;24;19m ({:4.0}MHz) | \
         MEM: \x1b[38;2;{};{};{}m{:5.1}%\x1b[0m\x1b[48;2;13;24;19m ({}/{}) | \
         PRESSURE: {}{:<8}\x1b[0m\x1b[48;2;13;24;19m ({:4.1}%) | \
         UPTIME: \x1b[38;2;{};{};{}m{:>8}\x1b[0m\n",
        pr, pg, pb,
        pr, pg, pb, snap.cpu.total_pct,
        snap.cpu.avg_freq_mhz,
        sr, sg, sb, snap.memory.used_pct,
        format_bytes(snap.memory.used_bytes),
        format_bytes(snap.memory.total_bytes),
        p_color,
        snap.pressure.level.as_str(),
        snap.pressure.score_percent,
        wr, wg, wb, format_uptime(snap.cpu.uptime_seconds)
    ));

    out.push_str("\x1b[48;2;9;15;12m ");
    for (i, label) in PAGES.iter().enumerate() {
        if i == page_idx {
            out.push_str(&format!(
                "\x1b[48;2;{};{};{}m\x1b[38;2;8;12;10m\x1b[1m {} \x1b[0m\x1b[48;2;9;15;12m ",
                pr, pg, pb, label
            ));
        } else {
            out.push_str(&format!(
                "\x1b[38;2;140;185;165m {} \x1b[0m\x1b[48;2;9;15;12m ",
                label
            ));
        }
    }
    out.push_str("\x1b[0m\n\x1b[38;2;31;56;46m────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────\x1b[0m\n");

    match page_idx {
        0 => render_summary_tui(&mut out, snap),
        1 => render_processes_tui(&mut out, snap, selected_row, tree_view),
        2 => render_performance_tui(&mut out, snap),
        3 => render_connections_tui(&mut out, snap),
        4 => render_sensors_tui(&mut out, snap),
        5 => render_diskspace_tui(&mut out, sampler),
        6 => render_drivers_tui(&mut out, sampler),
        7 => render_flight_tui(&mut out, sampler),
        8 => render_benchmarks_tui(&mut out, sampler),
        _ => render_catalogs_tui(&mut out, sampler),
    }

    out.push_str(&format!(
        "\n\x1b[48;2;14;25;20m\x1b[38;2;{};{};{}m {} \x1b[0m",
        sr, sg, sb, status_banner
    ));

    let mut lock = stdout().lock();
    lock.write_all(out.as_bytes())?;
    lock.flush()?;
    Ok(())
}

fn render_summary_tui(out: &mut String, snap: &SystemSnapshot) {
    out.push_str(&format!(
        " \x1b[1m[WHOLE MACHINE TELEMETRY]\x1b[0m  Model: \x1b[1m{}\x1b[0m ({}C/{}T, P:{} E:{})\n",
        snap.cpu.model_name,
        snap.cpu.physical_cores,
        snap.cpu.logical_cores,
        snap.cpu.p_cores,
        snap.cpu.e_cores
    ));
    out.push_str(&format!(
        "   CPU Total  : [{}] {:5.1}%  (User {:4.1}%, Sys {:4.1}%, IOwait {:4.1}%, Temp: {})\n",
        ascii_bar(snap.cpu.total_pct, 26),
        snap.cpu.total_pct,
        snap.cpu.user_pct,
        snap.cpu.system_pct,
        snap.cpu.iowait_pct,
        snap.cpu
            .package_temp_c
            .map_or(String::from("N/A"), |t| format!("{:.1}°C", t))
    ));
    out.push_str(&format!(
        "   Memory     : [{}] {:5.1}%  ({} used / {} total, Cached: {}, Zswap: {})\n",
        ascii_bar(snap.memory.used_pct, 26),
        snap.memory.used_pct,
        format_bytes(snap.memory.used_bytes),
        format_bytes(snap.memory.total_bytes),
        format_bytes(snap.memory.cached_bytes),
        if snap.memory.zswap_enabled { "ON" } else { "OFF" }
    ));
    out.push_str(&format!(
        "   Pressure   : [{}] {:5.1}%  Level: \x1b[1m{}\x1b[0m (Load 1m/5m/15m: {:.2}/{:.2}/{:.2}, Load/Core: {:.2}, Mem PSI avg10: {:.2}%)\n\n",
        ascii_bar(snap.pressure.score_percent, 26),
        snap.pressure.score_percent,
        snap.pressure.level.as_str(),
        snap.pressure.load_1m,
        snap.pressure.load_5m,
        snap.pressure.load_15m,
        snap.pressure.load_per_core,
        snap.pressure.mem_psi_some_avg10
    ));

    out.push_str(" \x1b[1m[TOP ACTIVE PROCESSES]\x1b[0m\n");
    out.push_str(&format!(
        "   {:<8} {:<24} {:<12} {:>8} {:>12} {:>8} {:>12}\n",
        "PID", "NAME", "USER", "CPU%", "MEMORY RSS", "THREADS", "STATE"
    ));
    for p in snap.processes.iter().take(12) {
        out.push_str(&format!(
            "   {:<8} {:<24} {:<12} {:>7.1}% {:>12} {:>8} {:>12}\n",
            p.pid,
            truncate_str(&p.name, 24),
            truncate_str(&p.user, 12),
            p.cpu_pct,
            format_bytes(p.mem_rss_bytes),
            p.threads,
            p.state_desc
        ));
    }
}

fn render_processes_tui(
    out: &mut String,
    snap: &SystemSnapshot,
    selected_row: usize,
    tree_view: bool,
) {
    out.push_str(&format!(
        " \x1b[1m[LIVE PROCESSES ({} total) — Press 't' for Tree View, 'i' to Inspect, 'x' to SIGTERM]\x1b[0m\n",
        snap.processes.len()
    ));
    out.push_str(&format!(
        "   {:<7} {:<7} {:<28} {:<11} {:<10} {:>7} {:>11} {:>11} {:>5}\n",
        "PID", "PPID", "PROCESS", "USER", "STATE", "CPU%", "RSS", "READ/s", "NICE"
    ));
    let mut rows: Vec<_> = snap.processes.iter().filter(|p| !p.is_kernel_thread).collect();
    if tree_view {
        rows.sort_by(|a, b| a.ppid.cmp(&b.ppid).then_with(|| a.pid.cmp(&b.pid)));
    }
    for (idx, p) in rows.iter().take(20).enumerate() {
        let sel = if idx == selected_row {
            "\x1b[48;2;24;52;42m\x1b[1m>"
        } else {
            " "
        };
        let label = if tree_view && p.tree_depth > 0 {
            format!("{}{}", "  ".repeat(p.tree_depth.min(4)), p.name)
        } else {
            p.name.clone()
        };
        out.push_str(&format!(
            "  {}{:<7} {:<7} {:<28} {:<11} {:<10} {:>6.1}% {:>11} {:>9}/s {:>5}\x1b[0m\n",
            sel,
            p.pid,
            p.ppid,
            truncate_str(&label, 28),
            truncate_str(&p.user, 11),
            p.state_desc,
            p.cpu_pct,
            format_bytes(p.mem_rss_bytes),
            format_bytes(p.read_bytes_per_sec as u64),
            p.nice
        ));
    }
}

fn render_performance_tui(out: &mut String, snap: &SystemSnapshot) {
    out.push_str(" \x1b[1m[PER-CORE CPU UTILIZATION & CLOCK GRID]\x1b[0m\n");
    for chunk in snap.cpu.cores.chunks(4) {
        out.push_str("   ");
        for c in chunk {
            out.push_str(&format!(
                "CPU{:<2}({}): {:5.1}% {:4.0}MHz | ",
                c.core_id,
                c.kind.as_str(),
                c.total_pct,
                c.freq_mhz
            ));
        }
        out.push('\n');
    }
    out.push_str("\n \x1b[1m[STORAGE BLOCK DEVICES]\x1b[0m\n");
    for d in &snap.disks {
        out.push_str(&format!(
            "   {:<10} {:<6} {:<24} Cap: {:>10} | Read: {:>10}/s | Write: {:>10}/s | Busy: {:5.1}%\n",
            d.name,
            d.kind,
            truncate_str(&d.model, 24),
            format_bytes(d.capacity_bytes),
            format_bytes(d.read_bytes_per_sec as u64),
            format_bytes(d.write_bytes_per_sec as u64),
            d.busy_pct
        ));
    }
    out.push_str("\n \x1b[1m[NETWORK ADAPTERS & GPUS]\x1b[0m\n");
    for n in &snap.networks {
        out.push_str(&format!(
            "   NET {:<10} {:<9} {:<15} State: {:<8} | RX: {:>10}/s | TX: {:>10}/s\n",
            n.name,
            n.kind,
            truncate_str(&n.ipv4, 15),
            n.oper_state,
            format_bytes(n.rx_bytes_per_sec as u64),
            format_bytes(n.tx_bytes_per_sec as u64)
        ));
    }
    for g in &snap.gpus {
        out.push_str(&format!(
            "   GPU {:<10} Driver: {:<10} {:<32} Busy: {:5.1}%\n",
            g.card_id,
            g.driver,
            truncate_str(&g.name, 32),
            g.busy_pct
        ));
    }
}

fn render_connections_tui(out: &mut String, snap: &SystemSnapshot) {
    out.push_str(&format!(
        " \x1b[1m[LIVE SOCKET CONNECTIONS ({} active)]\x1b[0m\n",
        snap.connections.len()
    ));
    out.push_str(&format!(
        "   {:<6} {:<26} {:<7} {:<24} {:<13} {:<8} {:<18} {:<12}\n",
        "PROTO", "LOCAL ADDR", "PORT", "REMOTE ADDR", "STATE", "PID", "PROCESS", "EXPOSURE"
    ));
    for c in snap.connections.iter().take(20) {
        out.push_str(&format!(
            "   {:<6} {:<26} {:<7} {:<24} {:<13} {:<8} {:<18} {:<12}\n",
            c.protocol,
            truncate_str(&c.local_addr, 26),
            c.local_port,
            truncate_str(&c.remote_addr, 24),
            c.state,
            c.pid.map_or(String::from("-"), |p| p.to_string()),
            truncate_str(&c.process_name, 18),
            if c.is_public_listener { "PUBLIC" } else { "Local" }
        ));
    }
}

fn render_sensors_tui(out: &mut String, snap: &SystemSnapshot) {
    out.push_str(" \x1b[1m[POWER & FREQUENCY LIVE SENSOR TREE]\x1b[0m\n");
    out.push_str(&format!(
        "   {:<28} {:<30} {:<12} {:>10} {:>10} {:>10} {:<6}\n",
        "CHIP / GROUP", "SENSOR", "CATEGORY", "CURRENT", "MIN", "MAX", "UNIT"
    ));
    let mut shown = 0usize;
    for grp in &snap.sensors {
        for r in &grp.readings {
            if shown >= 20 {
                break;
            }
            out.push_str(&format!(
                "   {:<28} {:<30} {:<12} {:>10.2} {:>10.2} {:>10.2} {:<6}\n",
                truncate_str(&grp.chip_name, 28),
                truncate_str(&r.label, 30),
                r.category,
                r.current,
                r.min,
                r.max,
                r.unit
            ));
            shown += 1;
        }
    }
}

fn render_diskspace_tui(out: &mut String, sampler: &LinuxSystemSampler) {
    let ds = &sampler.disk_space;
    out.push_str(&format!(
        " \x1b[1m[DISK SPACE ANALYZER — Root: {}]\x1b[0m\n",
        ds.root_path
    ));
    out.push_str(&format!(
        "   Filesystem Used: {} / {} (Free: {}) | Scanned {} files ({}) in {} ms\n\n",
        format_bytes(ds.fs_used_bytes),
        format_bytes(ds.fs_total_bytes),
        format_bytes(ds.fs_free_bytes),
        ds.scanned_files,
        format_bytes(ds.scanned_bytes),
        ds.scan_duration_ms
    ));
    out.push_str("   TOP SUBDIRECTORIES:\n");
    for d in ds.top_directories.iter().take(10) {
        out.push_str(&format!(
            "     {:<28} {:>12} ({:>6} files) [{}] {:5.1}%\n",
            truncate_str(&d.name, 28),
            format_bytes(d.size_bytes),
            d.file_count,
            ascii_bar(d.pct_of_root, 18),
            d.pct_of_root
        ));
    }
    out.push_str("\n   FILE CATEGORIES:\n");
    for c in ds.categories.iter().take(6) {
        out.push_str(&format!(
            "     {:<28} {:>12} ({:>6} files) {:5.1}%\n",
            c.category,
            format_bytes(c.size_bytes),
            c.file_count,
            c.pct_of_root
        ));
    }
}

fn render_drivers_tui(out: &mut String, sampler: &LinuxSystemSampler) {
    out.push_str(&format!(
        " \x1b[1m[KERNEL DRIVERS & MODULES ({} loaded)]\x1b[0m\n",
        sampler.drivers.len()
    ));
    out.push_str(&format!(
        "   {:<24} {:>12} {:>6} {:<10} {:<10} {:<30}\n",
        "MODULE", "MEMORY", "REFS", "STATE", "IN-TREE", "USED BY"
    ));
    for m in sampler.drivers.iter().take(20) {
        out.push_str(&format!(
            "   {:<24} {:>12} {:>6} {:<10} {:<10} {:<30}\n",
            truncate_str(&m.name, 24),
            format_bytes(m.size_bytes),
            m.ref_count,
            m.state,
            if m.in_tree { "Yes" } else { "Out-of-Tree" },
            truncate_str(&m.used_by.join(","), 30)
        ));
    }
}

fn render_flight_tui(out: &mut String, sampler: &LinuxSystemSampler) {
    let tr = &sampler.flight_recorder.trace;
    out.push_str(&format!(
        " \x1b[1m[FLIGHT RECORDER — {} v{} ({} frames) — Press 'e' to export .pulsetrace]\x1b[0m\n",
        tr.schema,
        tr.version,
        tr.samples.len()
    ));
    out.push_str(&format!(
        "   {:<7} {:<15} {:>8} {:>9} {:>8} {:<10} {:>12} {:<20}\n",
        "FRAME", "EPOCH MS", "CPU%", "CLOCK", "MEM%", "PRESSURE", "NET RX/s", "TOP PROCESS"
    ));
    for s in tr.samples.iter().rev().take(18) {
        out.push_str(&format!(
            "   #{:<6} {:<15} {:>7.1}% {:>7.0}MHz {:>7.1}% {:<10} {:>10}/s {:<20}\n",
            s.index,
            s.timestamp_epoch_ms,
            s.cpu_total_pct,
            s.cpu_avg_freq_mhz,
            s.mem_used_pct,
            s.pressure_level,
            format_bytes(s.net_rx_bps as u64),
            format!(
                "{} ({:.1}%)",
                truncate_str(&s.top_process_name, 12),
                s.top_process_cpu_pct
            )
        ));
    }
}

fn render_benchmarks_tui(out: &mut String, sampler: &LinuxSystemSampler) {
    out.push_str(" \x1b[1m[HARDWARE BENCHMARKS — Press 'b' to run suite]\x1b[0m\n\n");
    if let Some(ref b) = sampler.benchmarks {
        out.push_str(&format!(
            "   COMPOSITE PULSEDECK SCORE : \x1b[1m{}\x1b[0m (Baseline = 100)\n\n",
            b.composite_score
        ));
        for sc in [
            &b.cpu_single_core,
            &b.cpu_multi_core,
            &b.memory_bandwidth,
            &b.disk_throughput,
        ] {
            out.push_str(&format!(
                "   {:<26} : Score {:>5} | {:>9.2} {:<8} ({} ms)\n",
                sc.name, sc.normalized_score, sc.metric_value, sc.metric_unit, sc.duration_ms
            ));
        }
    } else {
        out.push_str(
            "   No benchmark run in this session yet. Press 'b' to execute CPU Single-Core, Multi-Core, Memory & Disk benchmarks.\n",
        );
    }
}

fn render_catalogs_tui(out: &mut String, sampler: &LinuxSystemSampler) {
    out.push_str(&format!(
        " \x1b[1m[INSTALLED APPS: {} | SERVICES: {} | AUTOSTART: {} | USERS: {}]\x1b[0m\n\n",
        sampler.installed_apps.len(),
        sampler.services.len(),
        sampler.startup_apps.len(),
        sampler.users.len()
    ));
    if let Some(ref info) = sampler.system_info {
        let rep = format_system_info_report(info);
        for line in rep.lines().take(20) {
            out.push_str("   ");
            out.push_str(line);
            out.push('\n');
        }
    }
}

fn ascii_bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let mut s = String::with_capacity(width);
    for i in 0..width {
        if i < filled {
            s.push('█');
        } else {
            s.push('░');
        }
    }
    s
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_uptime(sec: f64) -> String {
    let total = sec as u64;
    let d = total / 86400;
    let h = (total % 86400) / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if d > 0 {
        format!("{}d {:02}:{:02}:{:02}", d, h, m, s)
    } else {
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}
