use crate::model::{
    BenchmarkReport, BenchmarkSubScore, ConnectionEntry, CpuTelemetry, DirUsageEntry,
    DiskSpaceReport, FileCategoryEntry, FlightTraceFile, FlightTraceSample, InstalledAppEntry,
    KernelModuleEntry, LargeFileEntry, SensorGroup, SensorReading, SystemSnapshot,
};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ============================================================================
// 1. LIVE NETWORK CONNECTIONS (/proc/net/* on Linux, netstat -ano on Windows)
// ============================================================================

pub fn collect_connections() -> Vec<ConnectionEntry> {
    if !cfg!(target_os = "linux") || !Path::new("/proc/net/tcp").exists() {
        return collect_connections_windows();
    }

    let inode_map = build_socket_inode_to_pid_map();
    let mut out = Vec::with_capacity(256);

    parse_inet_socket_table("/proc/net/tcp", "TCP", false, &inode_map, &mut out);
    parse_inet_socket_table("/proc/net/tcp6", "TCP6", true, &inode_map, &mut out);
    parse_inet_socket_table("/proc/net/udp", "UDP", false, &inode_map, &mut out);
    parse_inet_socket_table("/proc/net/udp6", "UDP6", true, &inode_map, &mut out);
    parse_unix_socket_table("/proc/net/unix", &inode_map, &mut out);

    out.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "ESTABLISHED" => 0,
            "LISTEN" => 1,
            "ACTIVE" => 2,
            _ => 3,
        };
        rank(&a.state)
            .cmp(&rank(&b.state))
            .then_with(|| b.is_public_listener.cmp(&a.is_public_listener))
            .then_with(|| a.local_port.cmp(&b.local_port))
    });

    if out.len() > 400 {
        out.truncate(400);
    }
    out
}

fn collect_connections_windows() -> Vec<ConnectionEntry> {
    let mut out = Vec::new();
    let Ok(output) = Command::new("netstat").args(["-ano"]).output() else {
        return out;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let proto = parts[0].to_uppercase();
        if proto != "TCP" && proto != "UDP" {
            continue;
        }
        let (local_addr, local_port) = split_addr_port(parts[1]);
        let (remote_addr, remote_port) = split_addr_port(parts[2]);
        let (state, pid_str) = if proto == "TCP" && parts.len() >= 5 {
            (parts[3].to_string(), parts[4])
        } else {
            (String::from("LISTEN"), parts[3])
        };
        let pid = pid_str.parse::<u32>().ok();
        let is_loopback = local_addr.starts_with("127.") || local_addr == "::1";
        let is_public_listener =
            (state == "LISTENING" || state == "LISTEN" || proto == "UDP") && !is_loopback && local_port > 0;

        out.push(ConnectionEntry {
            protocol: proto,
            local_addr,
            local_port,
            remote_addr,
            remote_port,
            state,
            inode: 0,
            uid: 0,
            pid,
            process_name: pid.map_or_else(|| String::from("-"), |p| format!("PID {}", p)),
            is_public_listener,
        });
    }
    out
}

fn split_addr_port(s: &str) -> (String, u16) {
    if let Some((addr, port_s)) = s.rsplit_once(':') {
        let port = port_s.parse::<u16>().unwrap_or(0);
        (addr.to_string(), port)
    } else {
        (s.to_string(), 0)
    }
}

fn build_socket_inode_to_pid_map() -> FxHashMap<u64, (u32, String)> {
    let mut map = FxHashMap::default();
    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return map;
    };
    for entry in proc_entries.flatten() {
        let Some(pid_s) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        let Ok(pid) = pid_s.parse::<u32>() else {
            continue;
        };
        let fd_dir = format!("/proc/{}/fd", pid);
        let Ok(fds) = fs::read_dir(&fd_dir) else {
            continue;
        };
        let mut comm_cached: Option<String> = None;
        for fd in fds.flatten() {
            let Ok(link) = fs::read_link(fd.path()) else {
                continue;
            };
            let Some(link_s) = link.to_str() else {
                continue;
            };
            let inode_opt = link_s
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(inode) = inode_opt {
                let proc_name = comm_cached
                    .get_or_insert_with(|| {
                        fs::read_to_string(format!("/proc/{}/comm", pid))
                            .unwrap_or_else(|_| format!("pid-{}", pid))
                            .trim()
                            .to_string()
                    })
                    .clone();
                map.insert(inode, (pid, proc_name));
            }
        }
    }
    map
}

fn parse_inet_socket_table(
    path: &str,
    proto: &str,
    is_v6: bool,
    inode_map: &FxHashMap<u64, (u32, String)>,
    out: &mut Vec<ConnectionEntry>,
) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        let (local_addr, local_port) = decode_hex_endpoint(fields[1], is_v6);
        let (remote_addr, remote_port) = decode_hex_endpoint(fields[2], is_v6);
        let state_code = u8::from_str_radix(fields[3], 16).unwrap_or(0);
        let state = tcp_state_name(state_code, proto.starts_with("UDP")).to_string();
        let uid: u32 = fields[7].parse().unwrap_or(0);
        let inode: u64 = fields[9].parse().unwrap_or(0);

        let (pid, process_name) = inode_map
            .get(&inode)
            .map(|(p, n)| (Some(*p), n.clone()))
            .unwrap_or_else(|| (None, String::from("-")));

        let is_loopback = local_addr == "127.0.0.1"
            || local_addr.starts_with("127.")
            || local_addr == "::1"
            || local_addr == "0:0:0:0:0:0:0:1";
        let is_public_listener =
            (state == "LISTEN" || proto.starts_with("UDP")) && !is_loopback && local_port > 0;

        out.push(ConnectionEntry {
            protocol: proto.to_string(),
            local_addr,
            local_port,
            remote_addr,
            remote_port,
            state,
            inode,
            uid,
            pid,
            process_name,
            is_public_listener,
        });
    }
}

fn parse_unix_socket_table(
    path: &str,
    inode_map: &FxHashMap<u64, (u32, String)>,
    out: &mut Vec<ConnectionEntry>,
) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let mut count = 0usize;
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let Some(sock_path) = fields.get(7) else {
            continue;
        };
        let inode: u64 = fields[6].parse().unwrap_or(0);
        let state_code = u8::from_str_radix(fields[5], 16).unwrap_or(0);
        let state = match state_code {
            1 => "UNCONNECTED",
            2 => "CONNECTING",
            3 => "CONNECTED",
            4 => "DISCONNECTING",
            _ => "ACTIVE",
        };
        let (pid, process_name) = inode_map
            .get(&inode)
            .map(|(p, n)| (Some(*p), n.clone()))
            .unwrap_or_else(|| (None, String::from("-")));

        out.push(ConnectionEntry {
            protocol: "UNIX".to_string(),
            local_addr: (*sock_path).to_string(),
            local_port: 0,
            remote_addr: String::from("-"),
            remote_port: 0,
            state: state.to_string(),
            inode,
            uid: 0,
            pid,
            process_name,
            is_public_listener: false,
        });
        count += 1;
        if count >= 80 {
            break;
        }
    }
}

fn decode_hex_endpoint(s: &str, is_v6: bool) -> (String, u16) {
    let Some((addr_hex, port_hex)) = s.split_once(':') else {
        return (s.to_string(), 0);
    };
    let port = u16::from_str_radix(port_hex, 16).unwrap_or(0);
    if !is_v6 && addr_hex.len() == 8 {
        if let Ok(raw) = u32::from_str_radix(addr_hex, 16) {
            let ip = Ipv4Addr::from(raw.swap_bytes());
            return (ip.to_string(), port);
        }
    } else if is_v6 && addr_hex.len() == 32 {
        let mut bytes = [0u8; 16];
        for chunk_idx in 0..4 {
            let slice = &addr_hex[chunk_idx * 8..(chunk_idx + 1) * 8];
            if let Ok(word) = u32::from_str_radix(slice, 16) {
                let le_bytes = word.to_le_bytes();
                bytes[chunk_idx * 4..chunk_idx * 4 + 4].copy_from_slice(&le_bytes);
            }
        }
        let ip6 = Ipv6Addr::from(bytes);
        return (ip6.to_string(), port);
    }
    (addr_hex.to_string(), port)
}

fn tcp_state_name(code: u8, is_udp: bool) -> &'static str {
    if is_udp && code == 7 {
        return "LISTEN";
    }
    match code {
        0x01 => "ESTABLISHED",
        0x02 => "SYN_SENT",
        0x03 => "SYN_RECV",
        0x04 => "FIN_WAIT1",
        0x05 => "FIN_WAIT2",
        0x06 => "TIME_WAIT",
        0x07 => "CLOSE",
        0x08 => "CLOSE_WAIT",
        0x09 => "LAST_ACK",
        0x0A => "LISTEN",
        0x0B => "CLOSING",
        _ => "UNKNOWN",
    }
}

// ============================================================================
// 2. POWER & FREQUENCY SENSOR TREE
// ============================================================================

pub struct SensorCollector {
    min_max: BTreeMap<String, (f64, f64)>,
}

impl SensorCollector {
    pub fn new() -> Self {
        Self {
            min_max: BTreeMap::new(),
        }
    }

    pub fn sample(&mut self, cpu: &CpuTelemetry) -> Vec<SensorGroup> {
        let mut groups = Vec::new();

        let mut cpu_readings = Vec::new();
        if let Some(pkg_w) = cpu.package_power_watts {
            cpu_readings.push(self.record_reading(
                "cpu:pkg_power",
                "CPU Package Power (RAPL)",
                "Power",
                "W",
                pkg_w,
            ));
        }
        if let Some(pkg_t) = cpu.package_temp_c {
            cpu_readings.push(self.record_reading(
                "cpu:pkg_temp",
                "CPU Package Temperature",
                "Temperature",
                "°C",
                pkg_t,
            ));
        }
        cpu_readings.push(self.record_reading(
            "cpu:avg_clock",
            "Average Core Clock",
            "Frequency",
            "MHz",
            cpu.avg_freq_mhz,
        ));
        for core in &cpu.cores {
            let id = format!("cpu:core{}_mhz", core.core_id);
            let label = format!("Core {} ({}) Clock", core.core_id, core.kind.as_str());
            let mut r = self.record_reading(&id, &label, "Frequency", "MHz", core.freq_mhz);
            if core.min_freq_mhz > 0.0 && core.min_freq_mhz < r.min {
                r.min = core.min_freq_mhz;
            }
            if core.max_freq_mhz > r.max {
                r.max = core.max_freq_mhz;
            }
            cpu_readings.push(r);
        }
        groups.push(SensorGroup {
            chip_name: cpu.model_name.clone(),
            adapter: String::from("CPU Telemetry"),
            readings: cpu_readings,
        });

        if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
            for entry in entries.flatten() {
                let path = entry.path();
                let hwmon_id = entry.file_name().to_string_lossy().to_string();
                let chip_name = fs::read_to_string(path.join("name"))
                    .unwrap_or_else(|_| hwmon_id.clone())
                    .trim()
                    .to_string();
                let mut readings = Vec::new();

                for i in 1..=16 {
                    if let Ok(raw_s) = fs::read_to_string(path.join(format!("temp{}_input", i))) {
                        if let Ok(val) = raw_s.trim().parse::<f64>() {
                            let c = val / 1000.0;
                            let is_fault = fs::read_to_string(path.join(format!("temp{}_fault", i)))
                                .map(|s| s.trim() == "1")
                                .unwrap_or(false);
                            // Filter out 0.0°C or inactive/disconnected probes (common on ThinkPad / ACPI unused channels)
                            if !is_fault && c > 1.0 && c <= 140.0 {
                                let mut label =
                                    fs::read_to_string(path.join(format!("temp{}_label", i)))
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                if label.is_empty() {
                                    label = format!("Sensor Temp #{}", i);
                                }
                                let id = format!("{}:temp{}", hwmon_id, i);
                                readings.push(self.record_reading(
                                    &id,
                                    &label,
                                    "Temperature",
                                    "°C",
                                    c,
                                ));
                            }
                        }
                    }
                }

                for i in 1..=8 {
                    if let Ok(raw_s) = fs::read_to_string(path.join(format!("fan{}_input", i))) {
                        if let Ok(rpm) = raw_s.trim().parse::<f64>() {
                            let label = fs::read_to_string(path.join(format!("fan{}_label", i)))
                                .unwrap_or_else(|_| format!("Fan #{}", i))
                                .trim()
                                .to_string();
                            let id = format!("{}:fan{}", hwmon_id, i);
                            readings.push(self.record_reading(&id, &label, "Fan", "RPM", rpm));
                        }
                    }
                }

                for i in 0..=8 {
                    if let Ok(raw_s) = fs::read_to_string(path.join(format!("in{}_input", i))) {
                        if let Ok(mv) = raw_s.trim().parse::<f64>() {
                            let v = mv / 1000.0;
                            let label = fs::read_to_string(path.join(format!("in{}_label", i)))
                                .unwrap_or_else(|_| format!("Voltage in{}", i))
                                .trim()
                                .to_string();
                            let id = format!("{}:in{}", hwmon_id, i);
                            readings.push(self.record_reading(&id, &label, "Voltage", "V", v));
                        }
                    }
                }

                for i in 1..=4 {
                    let p_file = if path.join(format!("power{}_average", i)).exists() {
                        format!("power{}_average", i)
                    } else {
                        format!("power{}_input", i)
                    };
                    if let Ok(raw_s) = fs::read_to_string(path.join(p_file)) {
                        if let Ok(uw) = raw_s.trim().parse::<f64>() {
                            let w = uw / 1_000_000.0;
                            let label = fs::read_to_string(path.join(format!("power{}_label", i)))
                                .unwrap_or_else(|_| format!("Power Rail #{}", i))
                                .trim()
                                .to_string();
                            let id = format!("{}:power{}", hwmon_id, i);
                            readings.push(self.record_reading(&id, &label, "Power", "W", w));
                        }
                    }
                }

                if !readings.is_empty() {
                    groups.push(SensorGroup {
                        chip_name: format!("{} ({})", chip_name, hwmon_id),
                        adapter: String::from("Hardware Sensor"),
                        readings,
                    });
                }
            }
        }

        groups
    }

    fn record_reading(
        &mut self,
        id: &str,
        label: &str,
        category: &str,
        unit: &str,
        current: f64,
    ) -> SensorReading {
        let entry = self
            .min_max
            .entry(id.to_string())
            .or_insert((current, current));
        if current < entry.0 {
            entry.0 = current;
        }
        if current > entry.1 {
            entry.1 = current;
        }
        SensorReading {
            id: id.to_string(),
            label: label.to_string(),
            category: category.to_string(),
            unit: unit.to_string(),
            current,
            min: entry.0,
            max: entry.1,
        }
    }
}

// ============================================================================
// 3. DISK SPACE ANALYZER
// ============================================================================

pub fn analyze_disk_space(target_path: &str, max_files: usize) -> DiskSpaceReport {
    let start = Instant::now();
    let root = PathBuf::from(target_path);
    let canonical_root = fs::canonicalize(&root).unwrap_or(root);
    let root_str = canonical_root.display().to_string();

    let (fs_total, fs_used, fs_free) = statvfs_bytes(&canonical_root);

    let mut top_dir_sizes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut category_sizes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut largest_files: Vec<LargeFileEntry> = Vec::new();
    let mut scanned_bytes = 0u64;
    let mut scanned_files = 0u64;
    let mut scanned_dirs = 0u64;

    let mut stack: Vec<(PathBuf, Option<String>, usize)> = vec![(canonical_root.clone(), None, 0)];
    while let Some((dir_path, top_bucket, depth)) = stack.pop() {
        if scanned_files as usize >= max_files || depth > 10 {
            continue;
        }
        scanned_dirs += 1;
        let Ok(entries) = fs::read_dir(&dir_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let p = entry.path();
            let bucket = top_bucket
                .clone()
                .or_else(|| p.file_name().map(|n| n.to_string_lossy().to_string()));

            if ft.is_dir() {
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if matches!(name.as_str(), "proc" | "sys" | "dev" | "run") && depth == 0 {
                    continue;
                }
                stack.push((p, bucket, depth + 1));
            } else if ft.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                scanned_bytes += size;
                scanned_files += 1;

                if let Some(ref b) = bucket {
                    let e = top_dir_sizes.entry(b.clone()).or_insert((0, 0));
                    e.0 += size;
                    e.1 += 1;
                }

                let cat = classify_file_category(&p);
                let ce = category_sizes.entry(cat.to_string()).or_insert((0, 0));
                ce.0 += size;
                ce.1 += 1;

                if largest_files.len() < 25
                    || size > largest_files.last().map_or(0, |f| f.size_bytes)
                {
                    largest_files.push(LargeFileEntry {
                        path: p.display().to_string(),
                        size_bytes: size,
                        category: cat.to_string(),
                    });
                    largest_files.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
                    if largest_files.len() > 25 {
                        largest_files.truncate(25);
                    }
                }
            }
        }
    }

    let denom = (scanned_bytes.max(1)) as f64;
    let mut top_directories: Vec<DirUsageEntry> = top_dir_sizes
        .into_iter()
        .map(|(name, (sz, cnt))| DirUsageEntry {
            path: canonical_root.join(&name).display().to_string(),
            name,
            size_bytes: sz,
            file_count: cnt,
            pct_of_root: (sz as f64 / denom) * 100.0,
        })
        .collect();
    top_directories.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    if top_directories.len() > 20 {
        top_directories.truncate(20);
    }

    let mut categories: Vec<FileCategoryEntry> = category_sizes
        .into_iter()
        .map(|(category, (sz, cnt))| FileCategoryEntry {
            category,
            size_bytes: sz,
            file_count: cnt,
            pct_of_root: (sz as f64 / denom) * 100.0,
        })
        .collect();
    categories.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    let scanned_at_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    DiskSpaceReport {
        root_path: root_str,
        scanned_at_epoch,
        scan_duration_ms: start.elapsed().as_millis() as u64,
        fs_total_bytes: fs_total,
        fs_used_bytes: fs_used,
        fs_free_bytes: fs_free,
        scanned_bytes,
        scanned_files,
        scanned_dirs,
        top_directories,
        categories,
        largest_files,
    }
}

#[cfg(unix)]
fn statvfs_bytes(path: &Path) -> (u64, u64, u64) {
    use std::ffi::CString;
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return (0, 0, 0);
    };
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut st) == 0 {
            let frsize = if st.f_frsize > 0 {
                st.f_frsize as u64
            } else {
                st.f_bsize as u64
            };
            let total = st.f_blocks * frsize;
            let avail = st.f_bavail * frsize;
            let free = st.f_bfree * frsize;
            let used = total.saturating_sub(free);
            (total, used, avail)
        } else {
            (0, 0, 0)
        }
    }
}

#[cfg(not(unix))]
fn statvfs_bytes(path: &Path) -> (u64, u64, u64) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut best_match: Option<(usize, u64, u64)> = None;
    for d in &disks {
        let mp = d.mount_point();
        if path.starts_with(mp) {
            let len = mp.as_os_str().len();
            if best_match.map_or(true, |(b_len, _, _)| len > b_len) {
                best_match = Some((len, d.total_space(), d.available_space()));
            }
        }
    }
    if let Some((_, total, avail)) = best_match {
        (total, total.saturating_sub(avail), avail)
    } else if let Some(d) = disks.list().first() {
        let total = d.total_space();
        let avail = d.available_space();
        (total, total.saturating_sub(avail), avail)
    } else {
        (0, 0, 0)
    }
}

fn classify_file_category(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "c" | "cpp" | "cc" | "h" | "hpp" | "py" | "js" | "ts" | "tsx" | "go" | "java"
        | "sh" | "html" | "css" | "slint" => "Source Code",
        "so" | "a" | "o" | "rlib" | "rmeta" | "bin" | "elf" | "appimage" | "exe" | "dll"
        | "wasm" => "Binaries & Libraries",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "deb" | "rpm" | "iso" | "squashfs"
        | "crate" => "Archives & Packages",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "mp4" | "webm" | "mkv" | "mp3"
        | "wav" | "ogg" | "ttf" | "otf" => "Media & Fonts",
        "md" | "txt" | "pdf" | "doc" | "docx" | "rst" | "adoc" => "Documents",
        "json" | "jsonl" | "yaml" | "yml" | "toml" | "xml" | "db" | "sqlite" | "log" | "csv" => {
            "Data, Config & Logs"
        }
        _ => "Other Files",
    }
}

// ============================================================================
// 4. KERNEL DRIVERS / MODULES (/proc/modules on Linux, driverquery on Windows)
// ============================================================================

pub fn collect_kernel_modules() -> Vec<KernelModuleEntry> {
    if !cfg!(target_os = "linux") || !Path::new("/proc/modules").exists() {
        return collect_kernel_modules_windows();
    }

    let mut modules = Vec::new();
    let kernel_release = read_kernel_release();
    let dep_map = read_modules_dep_paths(&kernel_release);

    let Ok(content) = fs::read_to_string("/proc/modules") else {
        return modules;
    };

    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let name = fields[0].to_string();
        let size_bytes: u64 = fields[1].parse().unwrap_or(0);
        let ref_count: u32 = fields[2].parse().unwrap_or(0);
        let used_by: Vec<String> = if fields[3] == "-" {
            Vec::new()
        } else {
            fields[3]
                .trim_matches(',')
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        };
        let state = fields.get(4).copied().unwrap_or("Live").to_string();

        let sys_mod = format!("/sys/module/{}", name);
        let coresize = fs::read_to_string(format!("{}/coresize", sys_mod))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(size_bytes);
        let srcversion = fs::read_to_string(format!("{}/srcversion", sys_mod))
            .unwrap_or_default()
            .trim()
            .to_string();
        let taint = fs::read_to_string(format!("{}/taint", sys_mod))
            .unwrap_or_default()
            .trim()
            .to_string();
        let in_tree = !taint.contains('O');
        let module_path = dep_map
            .get(&name)
            .cloned()
            .unwrap_or_else(|| format!("/sys/module/{}", name));

        modules.push(KernelModuleEntry {
            name,
            size_bytes,
            ref_count,
            used_by,
            state,
            coresize,
            srcversion,
            in_tree,
            module_path,
        });
    }

    modules.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    modules
}

fn collect_kernel_modules_windows() -> Vec<KernelModuleEntry> {
    let mut modules = Vec::new();
    let Ok(output) = Command::new("driverquery")
        .args(["/fo", "csv", "/nh"])
        .output()
    else {
        return modules;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let cols: Vec<&str> = line.split("\",\"").collect();
        if cols.len() >= 3 {
            let name = cols[0].trim_matches('"').to_string();
            let display = cols[1].trim_matches('"').to_string();
            let drv_type = cols[2].trim_matches('"').to_string();
            modules.push(KernelModuleEntry {
                name,
                size_bytes: 0,
                ref_count: 1,
                used_by: vec![drv_type],
                state: String::from("Running"),
                coresize: 0,
                srcversion: String::new(),
                in_tree: true,
                module_path: display,
            });
        }
    }
    modules
}

fn read_kernel_release() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| String::from("linux"))
        .trim()
        .to_string()
}

fn read_modules_dep_paths(release: &str) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();
    let base = format!("/lib/modules/{}", release);
    let dep_path = format!("{}/modules.dep", base);
    if let Ok(content) = fs::read_to_string(&dep_path) {
        for line in content.lines() {
            if let Some((rel_path, _)) = line.split_once(':') {
                let file_stem = Path::new(rel_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .replace('-', "_");
                if !file_stem.is_empty() {
                    map.insert(file_stem, format!("{}/{}", base, rel_path.trim()));
                }
            }
        }
    }
    map
}

// ============================================================================
// 5. FLIGHT RECORDER (.pulsetrace Capture & Replay)
// ============================================================================

pub struct FlightRecorder {
    pub recording: bool,
    pub trace: FlightTraceFile,
}

impl FlightRecorder {
    pub fn new(hostname: String, cpu_model: String) -> Self {
        let now_sec = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            recording: true,
            trace: FlightTraceFile {
                schema: String::from("io.pulsedeck.traces"),
                version: 1,
                hostname,
                cpu_model,
                recorded_at_epoch: now_sec,
                samples: Vec::with_capacity(600),
            },
        }
    }

    pub fn push_snapshot(&mut self, snap: &SystemSnapshot) {
        if !self.recording {
            return;
        }
        let disk_r: f64 = snap.disks.iter().map(|d| d.read_bytes_per_sec).sum();
        let disk_w: f64 = snap.disks.iter().map(|d| d.write_bytes_per_sec).sum();
        let net_rx: f64 = snap.networks.iter().map(|n| n.rx_bytes_per_sec).sum();
        let net_tx: f64 = snap.networks.iter().map(|n| n.tx_bytes_per_sec).sum();
        let top = snap.processes.first();

        let idx = self.trace.samples.len();
        self.trace.samples.push(FlightTraceSample {
            index: idx,
            timestamp_epoch_ms: snap.timestamp_epoch_ms,
            cpu_total_pct: snap.cpu.total_pct,
            cpu_avg_freq_mhz: snap.cpu.avg_freq_mhz,
            cpu_temp_c: snap.cpu.package_temp_c.unwrap_or(0.0),
            mem_used_pct: snap.memory.used_pct,
            mem_used_bytes: snap.memory.used_bytes,
            swap_used_bytes: snap.memory.swap_used_bytes,
            pressure_level: snap.pressure.level.as_str().to_string(),
            pressure_score: snap.pressure.score_percent,
            disk_read_bps: disk_r,
            disk_write_bps: disk_w,
            net_rx_bps: net_rx,
            net_tx_bps: net_tx,
            top_process_name: top.map(|p| p.name.clone()).unwrap_or_default(),
            top_process_pid: top.map(|p| p.pid).unwrap_or(0),
            top_process_cpu_pct: top.map(|p| p.cpu_pct).unwrap_or(0.0),
        });

        if self.trace.samples.len() > 600 {
            self.trace.samples.remove(0);
            for (i, s) in self.trace.samples.iter_mut().enumerate() {
                s.index = i;
            }
        }
    }
}

// ============================================================================
// 6. HARDWARE BENCHMARKS & COMPOSITE SCORE
// ============================================================================

pub fn run_benchmarks(logical_cores: usize) -> BenchmarkReport {
    let threads = logical_cores.max(1);

    let sc_start = Instant::now();
    let sc_iters: u64 = 12_000_000;
    let mut acc: u64 = 0xcbf29ce484222325;
    let mut fp: f64 = 1.000001;
    for i in 1..=sc_iters {
        acc ^= i.wrapping_mul(0x9e3779b97f4a7c15);
        acc = acc.rotate_left(17).wrapping_mul(0x100000001b3);
        fp = (fp * 1.0000003 + (acc & 0xff) as f64 * 0.00001).fract() + 1.0;
    }
    std::hint::black_box((acc, fp));
    let sc_dur = sc_start.elapsed();
    let sc_mops = (sc_iters as f64 / sc_dur.as_secs_f64()) / 1_000_000.0;
    let sc_score = ((sc_mops / 180.0) * 100.0).round().clamp(10.0, 2000.0) as u32;

    let mc_start = Instant::now();
    let per_thread_iters: u64 = 8_000_000;
    let sink = Arc::new(AtomicU64::new(0));
    std::thread::scope(|s| {
        for tid in 0..threads {
            let sink_c = Arc::clone(&sink);
            s.spawn(move || {
                let mut x: u64 = 0x9e3779b97f4a7c15_u64.wrapping_add(tid as u64);
                for i in 1..=per_thread_iters {
                    x ^= i.wrapping_mul(0xbf58476d1ce4e5b9);
                    x = x.rotate_left(23).wrapping_mul(0x94d049bb133111eb);
                }
                sink_c.fetch_add(x, Ordering::Relaxed);
            });
        }
    });
    std::hint::black_box(sink.load(Ordering::Relaxed));
    let mc_dur = mc_start.elapsed();
    let mc_mops =
        ((per_thread_iters * threads as u64) as f64 / mc_dur.as_secs_f64()) / 1_000_000.0;
    let mc_score = ((mc_mops / 1200.0) * 100.0).round().clamp(10.0, 4000.0) as u32;

    let mem_start = Instant::now();
    let len = 8 * 1024 * 1024;
    let src: Vec<u64> = (0..len as u64).map(|i| i.wrapping_mul(7)).collect();
    let mut dst: Vec<u64> = vec![0u64; len];
    let passes = 4u64;
    let mut checksum = 0u64;
    for p in 0..passes {
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            *d = s.wrapping_add(p);
        }
        checksum = checksum.wrapping_add(dst[(p as usize * 4099) % len]);
    }
    std::hint::black_box(checksum);
    let mem_dur = mem_start.elapsed();
    let total_gb = ((len * 8 * 2) as f64 * passes as f64) / (1024.0 * 1024.0 * 1024.0);
    let mem_gbps = total_gb / mem_dur.as_secs_f64();
    let mem_score = ((mem_gbps / 12.0) * 100.0).round().clamp(10.0, 2000.0) as u32;

    let disk_start = Instant::now();
    let tmp_path = std::env::temp_dir().join(format!("pulsedeck_bench_{}.tmp", std::process::id()));
    let block = vec![0x5a_u8; 64 * 1024];
    let blocks = 256usize;
    let mut io_bytes = 0u64;
    if let Ok(mut f) = fs::File::create(&tmp_path) {
        for _ in 0..blocks {
            if f.write_all(&block).is_ok() {
                io_bytes += block.len() as u64;
            }
        }
        let _ = f.flush();
    }
    if let Ok(mut f) = fs::File::open(&tmp_path) {
        let mut rbuf = vec![0u8; 64 * 1024];
        while let Ok(n) = f.read(&mut rbuf) {
            if n == 0 {
                break;
            }
            io_bytes += n as u64;
        }
    }
    let _ = fs::remove_file(&tmp_path);
    let disk_dur = disk_start.elapsed();
    let disk_mbps = (io_bytes as f64 / (1024.0 * 1024.0)) / disk_dur.as_secs_f64().max(0.0001);
    let disk_score = ((disk_mbps / 800.0) * 100.0).round().clamp(10.0, 3000.0) as u32;

    let composite_score = ((sc_score as f64 * 0.30)
        + (mc_score as f64 * 0.35)
        + (mem_score as f64 * 0.20)
        + (disk_score as f64 * 0.15))
        .round() as u32;

    let timestamp_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    BenchmarkReport {
        timestamp_epoch,
        composite_score,
        cpu_single_core: BenchmarkSubScore {
            name: String::from("CPU Single-Core"),
            metric_value: sc_mops,
            metric_unit: String::from("Mops/s"),
            normalized_score: sc_score,
            duration_ms: sc_dur.as_millis() as u64,
        },
        cpu_multi_core: BenchmarkSubScore {
            name: format!("CPU Multi-Core ({}T)", threads),
            metric_value: mc_mops,
            metric_unit: String::from("Mops/s"),
            normalized_score: mc_score,
            duration_ms: mc_dur.as_millis() as u64,
        },
        memory_bandwidth: BenchmarkSubScore {
            name: String::from("Memory Bandwidth"),
            metric_value: mem_gbps,
            metric_unit: String::from("GB/s"),
            normalized_score: mem_score,
            duration_ms: mem_dur.as_millis() as u64,
        },
        disk_throughput: BenchmarkSubScore {
            name: String::from("Storage I/O Throughput"),
            metric_value: disk_mbps,
            metric_unit: String::from("MB/s"),
            normalized_score: disk_score,
            duration_ms: disk_dur.as_millis() as u64,
        },
    }
}

// ============================================================================
// 7. INSTALLED APPLICATIONS INVENTORY
// ============================================================================

pub fn collect_installed_apps() -> Vec<InstalledAppEntry> {
    let mut apps = Vec::new();

    // 1. Desktop applications (/usr/share/applications & ~/.local/share/applications)
    let mut desktop_dirs = vec![PathBuf::from("/usr/share/applications")];
    if let Ok(home) = std::env::var("HOME") {
        desktop_dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    for dir in desktop_dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&p) {
                    let mut name = String::new();
                    let mut comment = String::new();
                    let mut categories = String::from("Application");
                    let mut no_display = false;
                    for line in content.lines() {
                        if let Some(v) = line.strip_prefix("Name=") {
                            if name.is_empty() {
                                name = v.trim().to_string();
                            }
                        } else if let Some(v) = line.strip_prefix("Comment=") {
                            if comment.is_empty() {
                                comment = v.trim().to_string();
                            }
                        } else if let Some(v) = line.strip_prefix("Categories=") {
                            categories = v.trim_matches(';').replace(';', ", ");
                        } else if line.trim() == "NoDisplay=true" {
                            no_display = true;
                        }
                    }
                    if !name.is_empty() && !no_display {
                        apps.push(InstalledAppEntry {
                            name,
                            version: String::from("desktop"),
                            source: String::from("XDG Desktop"),
                            section: categories,
                            installed_size_kb: 0,
                            description: comment,
                        });
                    }
                }
            }
        }
    }

    // 2. Debian/Ubuntu dpkg database (/var/lib/dpkg/status)
    if let Ok(status_content) = fs::read_to_string("/var/lib/dpkg/status") {
        for stanza in status_content.split("\n\n") {
            let mut pkg = String::new();
            let mut version = String::new();
            let mut section = String::from("system");
            let mut size_kb = 0u64;
            let mut desc = String::new();
            let mut installed = false;

            for line in stanza.lines() {
                if let Some(v) = line.strip_prefix("Package: ") {
                    pkg = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Status: ") {
                    installed = v.contains("install ok installed");
                } else if let Some(v) = line.strip_prefix("Version: ") {
                    version = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Section: ") {
                    section = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("Installed-Size: ") {
                    size_kb = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("Description: ") {
                    desc = v.trim().to_string();
                }
            }
            if installed && !pkg.is_empty() {
                apps.push(InstalledAppEntry {
                    name: pkg,
                    version,
                    source: String::from("dpkg"),
                    section,
                    installed_size_kb: size_kb,
                    description: desc,
                });
            }
        }
    }

    // 3. Flatpak apps (/var/lib/flatpak/app)
    if let Ok(entries) = fs::read_dir("/var/lib/flatpak/app") {
        for entry in entries.flatten() {
            let app_id = entry.file_name().to_string_lossy().to_string();
            apps.push(InstalledAppEntry {
                name: app_id,
                version: String::from("current"),
                source: String::from("flatpak"),
                section: String::from("flatpak"),
                installed_size_kb: 0,
                description: String::from("Flatpak Sandboxed Application"),
            });
        }
    }

    // 4. Windows Registry Uninstall keys fallback when on Windows
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall",
                "/s",
                "/v",
                "DisplayName",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some((_, val)) = line.split_once("REG_SZ") {
                    let name = val.trim().to_string();
                    if !name.is_empty() {
                        apps.push(InstalledAppEntry {
                            name,
                            version: String::from("installed"),
                            source: String::from("Win32 Registry"),
                            section: String::from("Application"),
                            installed_size_kb: 0,
                            description: String::from("Windows Installed Program"),
                        });
                    }
                }
            }
        }
    }

    apps.sort_by(|a, b| {
        b.installed_size_kb
            .cmp(&a.installed_size_kb)
            .then_with(|| a.name.cmp(&b.name))
    });
    if apps.len() > 500 {
        apps.truncate(500);
    }
    apps
}
