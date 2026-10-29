use crate::model::{
    ProcessEntry, ServiceEntry, StartupEntry, SystemInfoCatalog, SystemSnapshot, UserSessionEntry,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use sysinfo::System;

pub fn collect_services() -> Vec<ServiceEntry> {
    let mut services = Vec::new();

    if let Ok(output) = Command::new("systemctl")
        .args([
            "list-units",
            "--type=service",
            "--all",
            "--no-pager",
            "--plain",
            "--no-legend",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let unit = parts[0].to_string();
                    let load_state = parts[1].to_string();
                    let active_state = parts[2].to_string();
                    let sub_state = parts[3].to_string();
                    let description = if parts.len() > 4 {
                        parts[4..].join(" ")
                    } else {
                        String::new()
                    };
                    services.push(ServiceEntry {
                        unit,
                        load_state,
                        active_state,
                        sub_state,
                        description,
                    });
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    if services.is_empty() {
        if let Ok(output) = Command::new("sc").args(["query", "state=", "all"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut cur_unit = String::new();
            let mut cur_desc = String::new();
            for line in text.lines() {
                let l = line.trim();
                if let Some(v) = l.strip_prefix("SERVICE_NAME:") {
                    cur_unit = v.trim().to_string();
                } else if let Some(v) = l.strip_prefix("DISPLAY_NAME:") {
                    cur_desc = v.trim().to_string();
                } else if let Some(v) = l.strip_prefix("STATE") {
                    let active = if v.contains("RUNNING") {
                        "active"
                    } else {
                        "inactive"
                    };
                    if !cur_unit.is_empty() {
                        services.push(ServiceEntry {
                            unit: cur_unit.clone(),
                            load_state: String::from("loaded"),
                            active_state: active.to_string(),
                            sub_state: active.to_string(),
                            description: cur_desc.clone(),
                        });
                    }
                }
            }
        }
    }

    if services.is_empty() {
        for dir in ["/etc/systemd/system", "/lib/systemd/system", "/etc/init.d"] {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".service") || dir == "/etc/init.d" {
                        services.push(ServiceEntry {
                            unit: name,
                            load_state: String::from("loaded"),
                            active_state: String::from("detected"),
                            sub_state: String::from("static"),
                            description: format!("Unit from {}", dir),
                        });
                    }
                }
            }
        }
    }

    services.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "active" => 0,
            "activating" | "reloading" => 1,
            "failed" => 2,
            _ => 3,
        };
        rank(&a.active_state)
            .cmp(&rank(&b.active_state))
            .then_with(|| a.unit.cmp(&b.unit))
    });
    if services.len() > 300 {
        services.truncate(300);
    }
    services
}

pub fn collect_startup_entries() -> Vec<StartupEntry> {
    let mut list = Vec::new();
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push((
            PathBuf::from(home).join(".config/autostart"),
            String::from("User (~/.config/autostart)"),
        ));
    }
    dirs.push((
        PathBuf::from("/etc/xdg/autostart"),
        String::from("System (/etc/xdg/autostart)"),
    ));

    for (dir, scope) in dirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let mut name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut exec = String::new();
            let mut comment = String::new();
            let mut hidden = false;
            let mut enabled = true;

            for line in content.lines() {
                if let Some(v) = line.strip_prefix("Name=") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Exec=") {
                    exec = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Comment=") {
                    comment = v.trim().to_string();
                } else if line.trim() == "Hidden=true" {
                    hidden = true;
                    enabled = false;
                } else if line.trim() == "X-GNOME-Autostart-enabled=false" {
                    enabled = false;
                }
            }
            if !hidden || !exec.is_empty() {
                list.push(StartupEntry {
                    name,
                    exec,
                    scope: scope.clone(),
                    enabled,
                    comment,
                    path: path.display().to_string(),
                });
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some((left, right)) = line.split_once("REG_SZ") {
                    let name = left.trim().to_string();
                    let exec = right.trim().to_string();
                    if !name.is_empty() {
                        list.push(StartupEntry {
                            name,
                            exec,
                            scope: String::from("HKCU\\...\\Run"),
                            enabled: true,
                            comment: String::from("Windows Registry Startup"),
                            path: String::from("HKCU"),
                        });
                    }
                }
            }
        }
    }

    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

pub fn collect_user_sessions(processes: &[ProcessEntry]) -> Vec<UserSessionEntry> {
    let mut per_user: BTreeMap<String, (u32, usize, f64, u64)> = BTreeMap::new();
    for p in processes {
        if p.is_kernel_thread {
            continue;
        }
        let entry = per_user.entry(p.user.clone()).or_insert((p.uid, 0, 0.0, 0));
        entry.1 += 1;
        entry.2 += p.cpu_pct;
        entry.3 += p.mem_rss_bytes;
    }

    let mut sessions = Vec::new();
    #[cfg(unix)]
    unsafe {
        libc::setutxent();
        loop {
            let ut = libc::getutxent();
            if ut.is_null() {
                break;
            }
            let ut_ref = &*ut;
            if ut_ref.ut_type == libc::USER_PROCESS {
                let user = c_char_slice_to_string(&ut_ref.ut_user);
                let tty = c_char_slice_to_string(&ut_ref.ut_line);
                let host = c_char_slice_to_string(&ut_ref.ut_host);
                let sec = ut_ref.ut_tv.tv_sec as u64;
                let (uid, cnt, cpu, rss) =
                    per_user.get(&user).copied().unwrap_or((1000, 0, 0.0, 0));
                sessions.push(UserSessionEntry {
                    username: user,
                    uid,
                    tty: if tty.is_empty() {
                        String::from("seat0")
                    } else {
                        tty
                    },
                    remote_host: if host.is_empty() {
                        String::from("local")
                    } else {
                        host
                    },
                    login_time: format!("epoch {}", sec),
                    process_count: cnt,
                    cpu_pct: cpu,
                    rss_bytes: rss,
                });
            }
        }
        libc::endutxent();
    }

    if sessions.is_empty() {
        for (user, (uid, cnt, cpu, rss)) in per_user {
            if uid >= 1000 || user == "root" || !cfg!(unix) {
                sessions.push(UserSessionEntry {
                    username: user,
                    uid,
                    tty: String::from("active"),
                    remote_host: String::from("local"),
                    login_time: String::from("running"),
                    process_count: cnt,
                    cpu_pct: cpu,
                    rss_bytes: rss,
                });
            }
        }
    }

    sessions.sort_by(|a, b| b.process_count.cmp(&a.process_count));
    sessions
}

#[cfg(unix)]
fn c_char_slice_to_string(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

pub fn build_system_info_catalog(snap: &SystemSnapshot) -> SystemInfoCatalog {
    let hostname = System::host_name()
        .ok()
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_else(|| String::from("workstation"))
        .trim()
        .to_string();

    let mut os_pretty_name = System::long_os_version()
        .ok()
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    if let Ok(os_rel) = fs::read_to_string("/etc/os-release") {
        for line in os_rel.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                os_pretty_name = v.trim_matches('"').to_string();
                break;
            }
        }
    }

    let kernel_release = System::kernel_version()
        .ok()
        .or_else(|| fs::read_to_string("/proc/sys/kernel/osrelease").ok())
        .unwrap_or_else(|| String::from("unknown"))
        .trim()
        .to_string();

    let machine_id_raw = fs::read_to_string("/etc/machine-id")
        .or_else(|_| fs::read_to_string("/var/lib/dbus/machine-id"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let machine_id_preview = if machine_id_raw.len() >= 12 {
        format!("{}...", &machine_id_raw[..12])
    } else {
        String::from("Standard Host ID")
    };

    let desktop_env = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                String::from("Windows Desktop (Win32/DWM)")
            } else {
                String::from("Desktop Session")
            }
        });
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            String::from("win32")
        } else {
            String::from("native")
        }
    });

    let read_dmi = |file: &str| -> String {
        fs::read_to_string(format!("/sys/class/dmi/id/{}", file))
            .unwrap_or_else(|_| String::from("N/A"))
            .trim()
            .to_string()
    };
    let board_vendor = read_dmi("sys_vendor");
    let board_name = read_dmi("product_name");
    let bios_version = read_dmi("bios_version");

    let mut sections: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

    sections.insert(
        String::from("1. Hardware & System Overview"),
        vec![
            (String::from("Hostname"), hostname.clone()),
            (String::from("Operating System"), os_pretty_name.clone()),
            (String::from("Kernel Release"), kernel_release.clone()),
            (String::from("Architecture"), snap.cpu.architecture.clone()),
            (String::from("System Vendor"), board_vendor.clone()),
            (String::from("Product / Board"), board_name.clone()),
            (String::from("BIOS / Firmware"), bios_version.clone()),
            (String::from("Desktop Environment"), desktop_env.clone()),
            (String::from("Session Type"), session_type.clone()),
            (String::from("UI Toolkit"), String::from("Slint Native GUI")),
        ],
    );

    sections.insert(
        String::from("2. Processor & Hybrid Topology"),
        vec![
            (String::from("Model"), snap.cpu.model_name.clone()),
            (String::from("Vendor ID"), snap.cpu.vendor_id.clone()),
            (
                String::from("Physical Cores"),
                snap.cpu.physical_cores.to_string(),
            ),
            (
                String::from("Logical Cores"),
                snap.cpu.logical_cores.to_string(),
            ),
            (
                String::from("Hybrid P-Cores / E-Cores"),
                format!("{} P / {} E", snap.cpu.p_cores, snap.cpu.e_cores),
            ),
            (
                String::from("Current Average Clock"),
                format!("{:.0} MHz", snap.cpu.avg_freq_mhz),
            ),
            (
                String::from("Max Rated Clock"),
                format!("{:.0} MHz", snap.cpu.max_freq_mhz),
            ),
        ],
    );

    sections.insert(
        String::from("3. Memory & Virtual Memory"),
        vec![
            (
                String::from("Total Physical RAM"),
                format_bytes(snap.memory.total_bytes),
            ),
            (
                String::from("Available RAM"),
                format_bytes(snap.memory.available_bytes),
            ),
            (
                String::from("Cached + Reclaimable"),
                format_bytes(snap.memory.cached_bytes),
            ),
            (
                String::from("Total Swap"),
                format_bytes(snap.memory.swap_total_bytes),
            ),
            (
                String::from("Zswap Kernel Compression"),
                if snap.memory.zswap_enabled {
                    String::from("Enabled")
                } else {
                    String::from("Disabled")
                },
            ),
        ],
    );

    let mut storage_rows = Vec::new();
    for d in &snap.disks {
        storage_rows.push((
            format!("{} ({})", d.name, d.kind),
            format!(
                "{} — {} — mounts: {}",
                d.model,
                format_bytes(d.capacity_bytes),
                if d.mount_points.is_empty() {
                    String::from("-")
                } else {
                    d.mount_points.join(", ")
                }
            ),
        ));
    }
    sections.insert(String::from("4. Storage Block Devices"), storage_rows);

    let mut gpu_rows = Vec::new();
    for g in &snap.gpus {
        gpu_rows.push((
            g.card_id.clone(),
            format!("{} (driver: {})", g.name, g.driver),
        ));
    }
    if gpu_rows.is_empty() {
        gpu_rows.push((
            String::from("Display Subsystem"),
            String::from("Standard Graphics Adapter"),
        ));
    }
    sections.insert(String::from("5. Graphics & Display Adapters"), gpu_rows);

    sections.insert(
        String::from("6. Native Telemetry Providers Status"),
        vec![
            (
                String::from("CpuProvider"),
                String::from("Active (Per-core load, clocks, thermals, power)"),
            ),
            (
                String::from("MemoryProvider"),
                String::from("Active (RAM, cache, swap, pressure metrics)"),
            ),
            (
                String::from("ProcessProvider"),
                format!("Active ({} live processes tracked)", snap.processes.len()),
            ),
            (
                String::from("ConnectionsProvider"),
                format!(
                    "Active ({} live sockets mapped to PIDs)",
                    snap.connections.len()
                ),
            ),
            (
                String::from("SensorProvider"),
                format!("Active ({} sensor groups)", snap.sensors.len()),
            ),
            (
                String::from("DriverProvider"),
                String::from("Active (Kernel modules & drivers inventory)"),
            ),
            (
                String::from("FlightRecorder"),
                String::from("Active (.pulsetrace JSON capture & replay)"),
            ),
        ],
    );

    SystemInfoCatalog {
        hostname,
        os_pretty_name,
        kernel_release,
        architecture: snap.cpu.architecture.clone(),
        machine_id_preview,
        desktop_env,
        session_type,
        board_vendor,
        board_name,
        bios_version,
        sections,
    }
}

pub fn format_system_info_report(cat: &SystemInfoCatalog) -> String {
    let mut out = String::new();
    out.push_str("========================================================================\n");
    out.push_str("  PULSEDECK SYSTEM INFORMATION & TELEMETRY DIAGNOSTIC REPORT\n");
    out.push_str("========================================================================\n\n");
    for (section, rows) in &cat.sections {
        out.push_str(&format!("[{}]\n", section));
        for (k, v) in rows {
            out.push_str(&format!("  {:<34} : {}\n", k, v));
        }
        out.push('\n');
    }
    out
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TiB", b / TB)
    } else if b >= GB {
        format!("{:.2} GiB", b / GB)
    } else if b >= MB {
        format!("{:.1} MiB", b / MB)
    } else if b >= KB {
        format!("{:.1} KiB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}
