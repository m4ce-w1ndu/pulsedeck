use crate::model::{DiskDeviceStats, GpuDeviceStats, NetworkAdapterStats};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;
use sysinfo::{Disks, Networks};

#[derive(Debug, Clone, Copy, Default)]
struct RawDiskStat {
    reads_completed: u64,
    sectors_read: u64,
    writes_completed: u64,
    sectors_written: u64,
    io_time_ms: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct RawNetStat {
    rx_bytes: u64,
    rx_packets: u64,
    tx_bytes: u64,
    tx_packets: u64,
}

pub struct IoGpuCollector {
    prev_disks: BTreeMap<String, RawDiskStat>,
    prev_nets: BTreeMap<String, RawNetStat>,
    prev_instant: Instant,
    sys_disks: Disks,
    sys_nets: Networks,
}

impl IoGpuCollector {
    pub fn new() -> Self {
        Self {
            prev_disks: read_proc_diskstats(),
            prev_nets: read_proc_net_dev(),
            prev_instant: Instant::now(),
            sys_disks: Disks::new_with_refreshed_list()
                .expect("Failed to initialize sysinfo Disks"),
            sys_nets: Networks::new_with_refreshed_list()
                .expect("Failed to initialize sysinfo Networks"),
        }
    }

    pub fn sample(
        &mut self,
    ) -> (
        Vec<DiskDeviceStats>,
        Vec<NetworkAdapterStats>,
        Vec<GpuDeviceStats>,
    ) {
        if !cfg!(target_os = "linux") || !Path::new("/proc/diskstats").exists() {
            return self.sample_cross_platform();
        }

        let now = Instant::now();
        let dt = now.duration_since(self.prev_instant).as_secs_f64().max(0.05);
        self.prev_instant = now;

        let cur_disks = read_proc_diskstats();
        let mounts = read_mount_points();
        let mut disks = Vec::new();

        for (name, cur) in &cur_disks {
            let prev = self.prev_disks.get(name).copied().unwrap_or(*cur);
            let read_bps = (cur.sectors_read.saturating_sub(prev.sectors_read) * 512) as f64 / dt;
            let write_bps =
                (cur.sectors_written.saturating_sub(prev.sectors_written) * 512) as f64 / dt;
            let read_iops = cur.reads_completed.saturating_sub(prev.reads_completed) as f64 / dt;
            let write_iops = cur.writes_completed.saturating_sub(prev.writes_completed) as f64 / dt;
            let busy_pct = ((cur.io_time_ms.saturating_sub(prev.io_time_ms) as f64 / (dt * 1000.0))
                * 100.0)
                .clamp(0.0, 100.0);

            let sys_block = format!("/sys/class/block/{}", name);
            let capacity_sectors: u64 = fs::read_to_string(format!("{}/size", sys_block))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let capacity_bytes = capacity_sectors * 512;
            if capacity_bytes == 0 && name.starts_with("loop") {
                continue;
            }

            let model = fs::read_to_string(format!("{}/device/model", sys_block))
                .unwrap_or_else(|_| name.clone())
                .trim()
                .to_string();

            let rotational = fs::read_to_string(format!("{}/queue/rotational", sys_block))
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .unwrap_or(0);

            let kind = if name.starts_with("nvme") {
                "NVMe".to_string()
            } else if name.starts_with("dm-")
                || name.starts_with("zram")
                || name.starts_with("loop")
            {
                "Virtual".to_string()
            } else if rotational == 0 {
                "SSD".to_string()
            } else {
                "HDD".to_string()
            };

            let dev_mounts = mounts
                .iter()
                .filter(|(dev, _)| dev.contains(name.as_str()))
                .map(|(_, mp)| mp.clone())
                .collect();

            disks.push(DiskDeviceStats {
                name: name.clone(),
                model,
                kind,
                capacity_bytes,
                read_bytes_per_sec: read_bps,
                write_bytes_per_sec: write_bps,
                read_iops,
                write_iops,
                busy_pct,
                total_read_bytes: cur.sectors_read * 512,
                total_written_bytes: cur.sectors_written * 512,
                mount_points: dev_mounts,
            });
        }
        self.prev_disks = cur_disks;

        let cur_nets = read_proc_net_dev();
        let ip_map = read_interface_ips();
        let mut nets = Vec::new();

        for (name, cur) in &cur_nets {
            let prev = self.prev_nets.get(name).copied().unwrap_or(*cur);
            let rx_bps = cur.rx_bytes.saturating_sub(prev.rx_bytes) as f64 / dt;
            let tx_bps = cur.tx_bytes.saturating_sub(prev.tx_bytes) as f64 / dt;
            let rx_pps = cur.rx_packets.saturating_sub(prev.rx_packets) as f64 / dt;
            let tx_pps = cur.tx_packets.saturating_sub(prev.tx_packets) as f64 / dt;

            let sys_net = format!("/sys/class/net/{}", name);
            let mac = fs::read_to_string(format!("{}/address", sys_net))
                .unwrap_or_else(|_| String::from("00:00:00:00:00:00"))
                .trim()
                .to_string();
            let oper_state = fs::read_to_string(format!("{}/operstate", sys_net))
                .unwrap_or_else(|_| String::from("unknown"))
                .trim()
                .to_string();
            let speed_mbps = fs::read_to_string(format!("{}/speed", sys_net))
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .filter(|&v| v > 0)
                .map(|v| v as u64);

            let kind = if name == "lo" {
                "Loopback".to_string()
            } else if Path::new(&format!("{}/wireless", sys_net)).exists()
                || name.starts_with("wl")
            {
                "Wi-Fi".to_string()
            } else if name.starts_with("en") || name.starts_with("eth") {
                "Ethernet".to_string()
            } else {
                "Virtual".to_string()
            };

            let (ipv4, ipv6) = ip_map.get(name).cloned().unwrap_or_default();

            nets.push(NetworkAdapterStats {
                name: name.clone(),
                kind,
                mac_address: mac,
                ipv4,
                ipv6,
                oper_state,
                speed_mbps,
                rx_bytes_per_sec: rx_bps,
                tx_bytes_per_sec: tx_bps,
                rx_packets_per_sec: rx_pps,
                tx_packets_per_sec: tx_pps,
                total_rx_bytes: cur.rx_bytes,
                total_tx_bytes: cur.tx_bytes,
            });
        }
        self.prev_nets = cur_nets;

        let gpus = sample_gpus();

        (disks, nets, gpus)
    }

    fn sample_cross_platform(
        &mut self,
    ) -> (
        Vec<DiskDeviceStats>,
        Vec<NetworkAdapterStats>,
        Vec<GpuDeviceStats>,
    ) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_instant).as_secs_f64().max(0.05);
        self.prev_instant = now;

        self.sys_disks.refresh(false);
        self.sys_nets.refresh(false);

        let mut disks = Vec::new();
        for d in &self.sys_disks {
            let usage = d.usage();
            disks.push(DiskDeviceStats {
                name: d.name().to_string_lossy().to_string(),
                model: d.file_system().to_string_lossy().to_string(),
                kind: format!("{:?}", d.kind()),
                capacity_bytes: d.total_space(),
                read_bytes_per_sec: usage.read_bytes as f64 / dt,
                write_bytes_per_sec: usage.written_bytes as f64 / dt,
                read_iops: 0.0,
                write_iops: 0.0,
                busy_pct: 0.0,
                total_read_bytes: usage.total_read_bytes,
                total_written_bytes: usage.total_written_bytes,
                mount_points: vec![d.mount_point().display().to_string()],
            });
        }

        let mut nets = Vec::new();
        for (iface_name, net_data) in &self.sys_nets {
            let mut ipv4 = String::new();
            let mut ipv6 = String::new();
            for ip in net_data.ip_networks() {
                if ip.addr.is_ipv4() && ipv4.is_empty() {
                    ipv4 = ip.addr.to_string();
                } else if ip.addr.is_ipv6() && ipv6.is_empty() {
                    ipv6 = ip.addr.to_string();
                }
            }
            nets.push(NetworkAdapterStats {
                name: iface_name.to_string_lossy().to_string(),
                kind: String::from("Adapter"),
                mac_address: net_data.mac_address().to_string(),
                ipv4,
                ipv6,
                oper_state: String::from("up"),
                speed_mbps: None,
                rx_bytes_per_sec: net_data.received() as f64 / dt,
                tx_bytes_per_sec: net_data.transmitted() as f64 / dt,
                rx_packets_per_sec: net_data.packets_received() as f64 / dt,
                tx_packets_per_sec: net_data.packets_transmitted() as f64 / dt,
                total_rx_bytes: net_data.total_received(),
                total_tx_bytes: net_data.total_transmitted(),
            });
        }

        (disks, nets, Vec::new())
    }
}

fn read_proc_diskstats() -> BTreeMap<String, RawDiskStat> {
    let mut map = BTreeMap::new();
    if let Ok(content) = fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 14 {
                continue;
            }
            let dev_name = fields[2];
            if is_partition_device(dev_name)
                || dev_name.starts_with("loop")
                || dev_name.starts_with("ram")
            {
                continue;
            }
            let stat = RawDiskStat {
                reads_completed: fields[3].parse().unwrap_or(0),
                sectors_read: fields[5].parse().unwrap_or(0),
                writes_completed: fields[7].parse().unwrap_or(0),
                sectors_written: fields[9].parse().unwrap_or(0),
                io_time_ms: fields[12].parse().unwrap_or(0),
            };
            map.insert(dev_name.to_string(), stat);
        }
    }
    map
}

fn is_partition_device(name: &str) -> bool {
    if !Path::new(&format!("/sys/class/block/{}", name)).exists() {
        return true;
    }
    Path::new(&format!("/sys/class/block/{}/partition", name)).exists()
}

fn read_mount_points() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(content) = fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let mut it = line.split_whitespace();
            if let (Some(dev), Some(mp)) = (it.next(), it.next()) {
                if dev.starts_with("/dev/") {
                    out.push((dev.to_string(), mp.to_string()));
                }
            }
        }
    }
    out
}

fn read_proc_net_dev() -> BTreeMap<String, RawNetStat> {
    let mut map = BTreeMap::new();
    if let Ok(content) = fs::read_to_string("/proc/net/dev") {
        for line in content.lines().skip(2) {
            let Some((iface, rest)) = line.split_once(':') else {
                continue;
            };
            let fields: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|v| v.parse::<u64>().ok())
                .collect();
            if fields.len() >= 10 {
                map.insert(
                    iface.trim().to_string(),
                    RawNetStat {
                        rx_bytes: fields[0],
                        rx_packets: fields[1],
                        tx_bytes: fields[8],
                        tx_packets: fields[9],
                    },
                );
            }
        }
    }
    map
}

#[cfg(unix)]
fn read_interface_ips() -> BTreeMap<String, (String, String)> {
    use std::ffi::CStr;
    let mut map: BTreeMap<String, (String, String)> = BTreeMap::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) == 0 {
            let mut cur = ifap;
            while !cur.is_null() {
                let ifa = &*cur;
                if !ifa.ifa_name.is_null() && !ifa.ifa_addr.is_null() {
                    let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().to_string();
                    let family = (*ifa.ifa_addr).sa_family as i32;
                    let entry = map.entry(name).or_default();
                    if family == libc::AF_INET && entry.0.is_empty() {
                        let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                        let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                        entry.0 = ip.to_string();
                    } else if family == libc::AF_INET6 && entry.1.is_empty() {
                        let sin6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                        let ip6 = std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                        entry.1 = ip6.to_string();
                    }
                }
                cur = ifa.ifa_next;
            }
            libc::freeifaddrs(ifap);
        }
    }
    map
}

#[cfg(not(unix))]
fn read_interface_ips() -> BTreeMap<String, (String, String)> {
    BTreeMap::new()
}

fn resolve_gpu_marketing_name(vendor_id: &str, device_id: &str, revision: &str) -> Option<String> {
    let dev_hex = device_id.trim_start_matches("0x").to_uppercase();
    let rev_hex = revision.trim_start_matches("0x").to_uppercase();

    // 1. Check /usr/share/libdrm/amdgpu.ids for exact AMD marketing name (e.g., "AMD Radeon RX 9070 XT")
    if vendor_id.eq_ignore_ascii_case("0x1002") {
        if let Ok(content) = fs::read_to_string("/usr/share/libdrm/amdgpu.ids") {
            let mut fallback_match = None;
            for line in content.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = t.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 3 && parts[0].eq_ignore_ascii_case(&dev_hex) {
                    if parts[1].eq_ignore_ascii_case(&rev_hex) {
                        return Some(parts[2].to_string());
                    }
                    if fallback_match.is_none() {
                        fallback_match = Some(parts[2].to_string());
                    }
                }
            }
            if fallback_match.is_some() {
                return fallback_match;
            }
        }
    }

    // 2. Check /usr/share/hwdata/pci.ids or /usr/share/misc/pci.ids
    let vend_hex = vendor_id.trim_start_matches("0x").to_lowercase();
    let dev_low = dev_hex.to_lowercase();
    for path in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
        if let Ok(content) = fs::read_to_string(path) {
            let mut in_vendor = false;
            for line in content.lines() {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                if !line.starts_with('\t') {
                    in_vendor = line.len() >= 4 && line[..4].eq_ignore_ascii_case(&vend_hex);
                } else if in_vendor && !line.starts_with("\t\t") {
                    let trimmed = line.trim_start_matches('\t');
                    if trimmed.len() > 5 && trimmed[..4].eq_ignore_ascii_case(&dev_low) {
                        let desc = trimmed[4..].trim();
                        if let (Some(l), Some(r)) = (desc.find('['), desc.rfind(']')) {
                            if r > l + 1 {
                                return Some(desc[l + 1..r].to_string());
                            }
                        }
                        return Some(desc.to_string());
                    }
                }
            }
        }
    }
    None
}

fn sample_gpus() -> Vec<GpuDeviceStats> {
    let mut gpus = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return gpus;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev_dir = entry.path().join("device");
        if !dev_dir.exists() {
            continue;
        }

        let vendor_id = fs::read_to_string(dev_dir.join("vendor"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let device_id = fs::read_to_string(dev_dir.join("device"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let revision = fs::read_to_string(dev_dir.join("revision"))
            .unwrap_or_default()
            .trim()
            .to_string();

        let mut driver = String::from("drm");
        let mut pci_slot = String::new();
        if let Ok(uevent) = fs::read_to_string(dev_dir.join("uevent")) {
            for line in uevent.lines() {
                if let Some(d) = line.strip_prefix("DRIVER=") {
                    driver = d.trim().to_string();
                } else if let Some(s) = line.strip_prefix("PCI_SLOT_NAME=") {
                    pci_slot = s.trim().to_string();
                }
            }
        }

        let vendor_label = match vendor_id.as_str() {
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD Radeon",
            "0x8086" => "Intel Graphics",
            "0x1af4" => "VirtIO GPU",
            _ => "DRM Graphics",
        };
        let gpu_name = resolve_gpu_marketing_name(&vendor_id, &device_id, &revision)
            .unwrap_or_else(|| format!("{} {}", vendor_label, device_id));

        let link_speed = fs::read_to_string(dev_dir.join("current_link_speed"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let link_width = fs::read_to_string(dev_dir.join("current_link_width"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let pcie_link = if !link_speed.is_empty() && !link_width.is_empty() {
            format!("{} x{}", link_speed, link_width)
        } else if !link_speed.is_empty() {
            link_speed
        } else {
            String::from("PCIe")
        };

        let busy_pct = fs::read_to_string(dev_dir.join("gpu_busy_percent"))
            .or_else(|_| fs::read_to_string(entry.path().join("gt_busy_percent")))
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0);

        let vram_used_bytes = fs::read_to_string(dev_dir.join("mem_info_vram_used"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let vram_total_bytes = fs::read_to_string(dev_dir.join("mem_info_vram_total"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let gtt_used_bytes = fs::read_to_string(dev_dir.join("mem_info_gtt_used"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());

        let mut clock_mhz = fs::read_to_string(entry.path().join("gt_cur_freq_mhz"))
            .or_else(|_| fs::read_to_string(entry.path().join("gt/gt0/rps_cur_freq_mhz")))
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok());
        let mut max_clock_mhz = None;
        if let Ok(dpm) = fs::read_to_string(dev_dir.join("pp_dpm_sclk")) {
            for line in dpm.lines() {
                for tok in line.split_whitespace() {
                    if let Some(mhz_str) = tok.strip_suffix("Mhz").or_else(|| tok.strip_suffix("MHz")) {
                        if let Ok(v) = mhz_str.parse::<f64>() {
                            max_clock_mhz = Some(max_clock_mhz.map_or(v, |m: f64| m.max(v)));
                            if line.contains('*') && clock_mhz.is_none() {
                                clock_mhz = Some(v);
                            }
                        }
                    }
                }
            }
        }

        let mut temp_c = None;
        let mut power_watts = None;
        let mut fan_rpm = None;
        if let Ok(hwmons) = fs::read_dir(dev_dir.join("hwmon")) {
            for hm in hwmons.flatten() {
                let hp = hm.path();
                if let Ok(t_s) = fs::read_to_string(hp.join("temp1_input")) {
                    if let Ok(mc) = t_s.trim().parse::<f64>() {
                        temp_c = Some(mc / 1000.0);
                    }
                }
                if let Ok(p_s) = fs::read_to_string(hp.join("power1_average"))
                    .or_else(|_| fs::read_to_string(hp.join("power1_input")))
                {
                    if let Ok(uw) = p_s.trim().parse::<f64>() {
                        power_watts = Some(uw / 1_000_000.0);
                    }
                }
                if let Ok(f_s) = fs::read_to_string(hp.join("fan1_input")) {
                    if let Ok(rpm) = f_s.trim().parse::<u32>() {
                        fan_rpm = Some(rpm);
                    }
                }
                if let Ok(fr_s) = fs::read_to_string(hp.join("freq1_input")) {
                    if let Ok(hz) = fr_s.trim().parse::<f64>() {
                        clock_mhz = Some(hz / 1_000_000.0);
                    }
                }
            }
        }

        gpus.push(GpuDeviceStats {
            card_id: name,
            name: gpu_name,
            driver,
            vendor_id,
            device_id,
            pci_slot,
            pcie_link,
            busy_pct,
            vram_used_bytes,
            vram_total_bytes,
            gtt_used_bytes,
            clock_mhz,
            max_clock_mhz,
            temp_c,
            power_watts,
            fan_rpm,
        });
    }
    gpus
}
