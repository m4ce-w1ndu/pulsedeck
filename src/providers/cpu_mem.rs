use crate::model::{
    CoreKind, CpuTelemetry, LogicalCoreStats, MemoryTelemetry, PressureLevel, SystemPressure,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;
use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[derive(Debug, Clone, Copy, Default)]
struct RawCpuCounters {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl RawCpuCounters {
    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    fn busy(&self) -> u64 {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }
}

pub struct CpuMemCollector {
    prev_total: RawCpuCounters,
    prev_cores: BTreeMap<usize, RawCpuCounters>,
    prev_ctxt: u64,
    prev_intr: u64,
    prev_pgfault: u64,
    prev_pgmajfault: u64,
    prev_rapl_uj: Option<u64>,
    prev_instant: Instant,
    model_name: String,
    vendor_id: String,
    physical_cores: usize,
    p_core_set: HashSet<usize>,
    e_core_set: HashSet<usize>,
    sys: System,
    components: Components,
}

impl CpuMemCollector {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        )
        .expect("Failed to initialize sysinfo System");
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let components =
            Components::new_with_refreshed_list().expect("Failed to initialize sysinfo Components");

        let (mut model_name, mut vendor_id, mut physical_cores) = read_cpuinfo_identity();
        if model_name == "Processor" {
            if let Some(first_cpu) = sys.cpus().first() {
                if !first_cpu.brand().is_empty() {
                    model_name = first_cpu.brand().to_string();
                }
                if !first_cpu.vendor_id().is_empty() {
                    vendor_id = first_cpu.vendor_id().to_string();
                }
            }
            physical_cores =
                System::physical_core_count().unwrap_or_else(|_| sys.cpus().len().max(1));
        }

        let p_core_set = parse_cpu_list_file("/sys/bus/event_source/devices/cpu_core/cpus");
        let e_core_set = parse_cpu_list_file("/sys/bus/event_source/devices/cpu_atom/cpus");
        let (prev_total, prev_cores, prev_ctxt, prev_intr, _, _) = read_proc_stat();
        let (prev_pgfault, prev_pgmajfault) = read_vmstat_faults();
        let prev_rapl_uj = read_rapl_package_energy_uj();

        Self {
            prev_total,
            prev_cores,
            prev_ctxt,
            prev_intr,
            prev_pgfault,
            prev_pgmajfault,
            prev_rapl_uj,
            prev_instant: Instant::now(),
            model_name,
            vendor_id,
            physical_cores,
            p_core_set,
            e_core_set,
            sys,
            components,
        }
    }

    pub fn sample(&mut self) -> (CpuTelemetry, MemoryTelemetry, SystemPressure) {
        if !cfg!(target_os = "linux") || !Path::new("/proc/stat").exists() {
            return self.sample_cross_platform();
        }

        let now = Instant::now();
        let dt = now.duration_since(self.prev_instant).as_secs_f64().max(0.05);
        self.prev_instant = now;

        let (cur_total, cur_cores, cur_ctxt, cur_intr, procs_running, procs_blocked) =
            read_proc_stat();

        let (total_pct, user_pct, system_pct, iowait_pct, irq_pct) =
            compute_cpu_deltas(&self.prev_total, &cur_total);
        self.prev_total = cur_total;

        let ctxt_per_sec = cur_ctxt.saturating_sub(self.prev_ctxt) as f64 / dt;
        let intr_per_sec = cur_intr.saturating_sub(self.prev_intr) as f64 / dt;
        self.prev_ctxt = cur_ctxt;
        self.prev_intr = cur_intr;

        let core_temps = read_core_temperatures();
        let cpuinfo_freqs = read_cpuinfo_mhz_map();

        let mut cores = Vec::with_capacity(cur_cores.len());
        let mut freq_sum = 0.0;
        let mut max_freq = 0.0_f64;

        for (&core_id, cur_c) in &cur_cores {
            let prev_c = self.prev_cores.get(&core_id).copied().unwrap_or(*cur_c);
            let (c_tot, c_usr, c_sys, c_iow, _) = compute_cpu_deltas(&prev_c, cur_c);

            let (cur_mhz, min_mhz, max_mhz) = read_cpufreq_mhz(core_id, &cpuinfo_freqs);
            freq_sum += cur_mhz;
            if max_mhz > max_freq {
                max_freq = max_mhz;
            }
            if cur_mhz > max_freq {
                max_freq = cur_mhz;
            }

            let kind = if self.p_core_set.contains(&core_id) {
                CoreKind::Performance
            } else if self.e_core_set.contains(&core_id) {
                CoreKind::Efficiency
            } else {
                CoreKind::Standard
            };

            cores.push(LogicalCoreStats {
                core_id,
                kind,
                total_pct: c_tot,
                user_pct: c_usr,
                system_pct: c_sys,
                iowait_pct: c_iow,
                freq_mhz: cur_mhz,
                min_freq_mhz: min_mhz,
                max_freq_mhz: max_mhz,
                temp_c: core_temps
                    .get(&core_id)
                    .copied()
                    .or_else(|| core_temps.get(&999).copied()),
            });
        }
        self.prev_cores = cur_cores;

        let logical_cores = cores.len().max(1);
        let avg_freq_mhz = if !cores.is_empty() {
            freq_sum / (cores.len() as f64)
        } else {
            0.0
        };

        let cur_rapl_uj = read_rapl_package_energy_uj();
        let package_power_watts = match (self.prev_rapl_uj, cur_rapl_uj) {
            (Some(prev), Some(cur)) if cur >= prev => {
                Some(((cur - prev) as f64 / 1_000_000.0) / dt)
            }
            _ => None,
        };
        self.prev_rapl_uj = cur_rapl_uj;

        let uptime_seconds = read_uptime_seconds();
        let package_temp_c = core_temps.get(&999).copied().or_else(|| {
            cores.iter().filter_map(|c| c.temp_c).fold(None, |acc, t| {
                Some(acc.map_or(t, |m: f64| m.max(t)))
            })
        });

        let cpu = CpuTelemetry {
            model_name: self.model_name.clone(),
            vendor_id: self.vendor_id.clone(),
            architecture: std::env::consts::ARCH.to_string(),
            physical_cores: self.physical_cores.max(1),
            logical_cores,
            p_cores: self.p_core_set.len(),
            e_cores: self.e_core_set.len(),
            total_pct,
            user_pct,
            system_pct,
            iowait_pct,
            irq_pct,
            avg_freq_mhz,
            max_freq_mhz: max_freq,
            package_temp_c,
            package_power_watts,
            context_switches_per_sec: ctxt_per_sec,
            interrupts_per_sec: intr_per_sec,
            processes_running: procs_running,
            processes_blocked: procs_blocked,
            uptime_seconds,
            cores,
        };

        let (cur_pgfault, cur_pgmajfault) = read_vmstat_faults();
        let pgfault_rate = cur_pgfault.saturating_sub(self.prev_pgfault) as f64 / dt;
        let pgmajfault_rate = cur_pgmajfault.saturating_sub(self.prev_pgmajfault) as f64 / dt;
        self.prev_pgfault = cur_pgfault;
        self.prev_pgmajfault = cur_pgmajfault;

        let memory = sample_memory(pgfault_rate, pgmajfault_rate);
        let pressure = sample_system_pressure(logical_cores, &memory);

        (cpu, memory, pressure)
    }

    /// Cross-platform sampler used on Windows and macOS via `sysinfo`.
    fn sample_cross_platform(&mut self) -> (CpuTelemetry, MemoryTelemetry, SystemPressure) {
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.components.refresh(false);

        let mut pkg_temp: Option<f64> = None;
        for comp in &self.components {
            if let Some(t) = comp.temperature() {
                let tf = t as f64;
                if (0.0..=150.0).contains(&tf) {
                    pkg_temp = Some(pkg_temp.map_or(tf, |m| m.max(tf)));
                }
            }
        }

        let mut cores = Vec::with_capacity(self.sys.cpus().len());
        let mut freq_sum = 0.0;
        let mut max_freq = 0.0_f64;
        for (idx, c) in self.sys.cpus().iter().enumerate() {
            let usage = (c.usage() as f64).clamp(0.0, 100.0);
            let mhz = c.frequency() as f64;
            freq_sum += mhz;
            if mhz > max_freq {
                max_freq = mhz;
            }
            cores.push(LogicalCoreStats {
                core_id: idx,
                kind: CoreKind::Standard,
                total_pct: usage,
                user_pct: usage * 0.75,
                system_pct: usage * 0.25,
                iowait_pct: 0.0,
                freq_mhz: mhz,
                min_freq_mhz: mhz,
                max_freq_mhz: mhz,
                temp_c: pkg_temp,
            });
        }
        let logical_cores = cores.len().max(1);
        let total_pct = (self.sys.global_cpu_usage() as f64).clamp(0.0, 100.0);
        let avg_freq_mhz = freq_sum / (logical_cores as f64);

        let cpu = CpuTelemetry {
            model_name: self.model_name.clone(),
            vendor_id: self.vendor_id.clone(),
            architecture: std::env::consts::ARCH.to_string(),
            physical_cores: self.physical_cores.max(1),
            logical_cores,
            p_cores: 0,
            e_cores: 0,
            total_pct,
            user_pct: total_pct * 0.75,
            system_pct: total_pct * 0.25,
            iowait_pct: 0.0,
            irq_pct: 0.0,
            avg_freq_mhz,
            max_freq_mhz: max_freq,
            package_temp_c: pkg_temp,
            package_power_watts: None,
            context_switches_per_sec: 0.0,
            interrupts_per_sec: 0.0,
            processes_running: 1,
            processes_blocked: 0,
            uptime_seconds: System::uptime().unwrap_or(0) as f64,
            cores,
        };

        let total_bytes = self.sys.total_memory();
        let used_bytes = self.sys.used_memory();
        let avail_bytes = self.sys.available_memory();
        let free_bytes = self.sys.free_memory();
        let swap_total = self.sys.total_swap();
        let swap_used = self.sys.used_swap();
        let swap_free = self.sys.free_swap();
        let used_pct = if total_bytes > 0 {
            ((used_bytes as f64 / total_bytes as f64) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let swap_pct = if swap_total > 0 {
            ((swap_used as f64 / swap_total as f64) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        let memory = MemoryTelemetry {
            total_bytes,
            used_bytes,
            available_bytes: avail_bytes,
            free_bytes,
            buffers_bytes: 0,
            cached_bytes: avail_bytes.saturating_sub(free_bytes),
            shared_bytes: 0,
            slab_bytes: 0,
            committed_bytes: used_bytes + swap_used,
            commit_limit_bytes: total_bytes + swap_total,
            swap_total_bytes: swap_total,
            swap_used_bytes: swap_used,
            swap_free_bytes: swap_free,
            zswap_enabled: false,
            used_pct,
            swap_pct,
            page_faults_per_sec: 0.0,
            major_faults_per_sec: 0.0,
        };

        let (lavg_one, lavg_five, lavg_fifteen) = System::load_average()
            .map(|l| (l.one, l.five, l.fifteen))
            .unwrap_or((0.0, 0.0, 0.0));
        let load_1m = if lavg_one > 0.0 {
            lavg_one
        } else {
            (total_pct / 100.0) * (logical_cores as f64)
        };
        let load_per_core = load_1m / (logical_cores as f64);
        let score_percent = (total_pct * 0.6 + used_pct * 0.4).clamp(0.0, 100.0);
        let level = if score_percent >= 80.0 {
            PressureLevel::Critical
        } else if score_percent >= 60.0 {
            PressureLevel::High
        } else if score_percent >= 35.0 {
            PressureLevel::Elevated
        } else {
            PressureLevel::Low
        };

        let pressure = SystemPressure {
            level,
            score_percent,
            load_1m,
            load_5m: lavg_five,
            load_15m: lavg_fifteen,
            load_per_core,
            mem_psi_some_avg10: 0.0,
            mem_psi_full_avg10: 0.0,
            cpu_psi_some_avg10: 0.0,
            io_psi_some_avg10: 0.0,
        };

        (cpu, memory, pressure)
    }
}

fn compute_cpu_deltas(prev: &RawCpuCounters, cur: &RawCpuCounters) -> (f64, f64, f64, f64, f64) {
    let total_delta = cur.total().saturating_sub(prev.total());
    if total_delta == 0 {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let td = total_delta as f64;
    let busy_delta = cur.busy().saturating_sub(prev.busy()) as f64;
    let user_delta = (cur.user + cur.nice).saturating_sub(prev.user + prev.nice) as f64;
    let sys_delta = cur.system.saturating_sub(prev.system) as f64;
    let iow_delta = cur.iowait.saturating_sub(prev.iowait) as f64;
    let irq_delta = (cur.irq + cur.softirq).saturating_sub(prev.irq + prev.softirq) as f64;

    (
        ((busy_delta / td) * 100.0).clamp(0.0, 100.0),
        ((user_delta / td) * 100.0).clamp(0.0, 100.0),
        ((sys_delta / td) * 100.0).clamp(0.0, 100.0),
        ((iow_delta / td) * 100.0).clamp(0.0, 100.0),
        ((irq_delta / td) * 100.0).clamp(0.0, 100.0),
    )
}

fn read_proc_stat() -> (
    RawCpuCounters,
    BTreeMap<usize, RawCpuCounters>,
    u64,
    u64,
    u64,
    u64,
) {
    let mut total = RawCpuCounters::default();
    let mut cores = BTreeMap::new();
    let mut ctxt = 0;
    let mut intr = 0;
    let mut procs_running = 0;
    let mut procs_blocked = 0;

    if let Ok(content) = fs::read_to_string("/proc/stat") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("cpu ") {
                total = parse_cpu_line(rest);
            } else if let Some(rest) = line.strip_prefix("cpu") {
                let mut parts = rest.split_whitespace();
                if let Some(id_str) = parts.next() {
                    if let Ok(core_id) = id_str.parse::<usize>() {
                        let remainder = rest[id_str.len()..].trim_start();
                        cores.insert(core_id, parse_cpu_line(remainder));
                    }
                }
            } else if let Some(rest) = line.strip_prefix("ctxt ") {
                ctxt = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("intr ") {
                intr = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("procs_running ") {
                procs_running = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("procs_blocked ") {
                procs_blocked = rest.trim().parse().unwrap_or(0);
            }
        }
    }
    (total, cores, ctxt, intr, procs_running, procs_blocked)
}

fn parse_cpu_line(s: &str) -> RawCpuCounters {
    let mut nums = s.split_whitespace().filter_map(|v| v.parse::<u64>().ok());
    RawCpuCounters {
        user: nums.next().unwrap_or(0),
        nice: nums.next().unwrap_or(0),
        system: nums.next().unwrap_or(0),
        idle: nums.next().unwrap_or(0),
        iowait: nums.next().unwrap_or(0),
        irq: nums.next().unwrap_or(0),
        softirq: nums.next().unwrap_or(0),
        steal: nums.next().unwrap_or(0),
    }
}

fn read_cpuinfo_identity() -> (String, String, usize) {
    let mut model = String::from("Processor");
    let mut vendor = String::from("Generic");
    let mut physical_ids = HashSet::new();
    let mut cpu_cores_field = 0usize;

    if let Ok(content) = fs::read_to_string("/proc/cpuinfo") {
        let mut cur_phys_id = 0usize;
        for line in content.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                let val = v.trim();
                match key {
                    "model name" | "Model" | "Hardware" if model == "Processor" => {
                        model = val.to_string();
                    }
                    "vendor_id" if vendor == "Generic" => {
                        vendor = val.to_string();
                    }
                    "physical id" => {
                        cur_phys_id = val.parse().unwrap_or(0);
                    }
                    "core id" => {
                        if let Ok(cid) = val.parse::<usize>() {
                            physical_ids.insert((cur_phys_id, cid));
                        }
                    }
                    "cpu cores" if cpu_cores_field == 0 => {
                        cpu_cores_field = val.parse().unwrap_or(0);
                    }
                    _ => {}
                }
            }
        }
    }
    let phys = if !physical_ids.is_empty() {
        physical_ids.len()
    } else if cpu_cores_field > 0 {
        cpu_cores_field
    } else {
        1
    };
    (model, vendor, phys)
}

fn read_cpuinfo_mhz_map() -> BTreeMap<usize, f64> {
    let mut map = BTreeMap::new();
    if let Ok(content) = fs::read_to_string("/proc/cpuinfo") {
        let mut cur_proc = 0usize;
        for line in content.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                let val = v.trim();
                if key == "processor" {
                    cur_proc = val.parse().unwrap_or(0);
                } else if key == "cpu MHz" {
                    if let Ok(mhz) = val.parse::<f64>() {
                        map.insert(cur_proc, mhz);
                    }
                }
            }
        }
    }
    map
}

fn read_cpufreq_mhz(core_id: usize, fallback_map: &BTreeMap<usize, f64>) -> (f64, f64, f64) {
    let base = format!("/sys/devices/system/cpu/cpu{}/cpufreq", core_id);
    let read_khz = |file: &str| -> Option<f64> {
        fs::read_to_string(format!("{}/{}", base, file))
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|khz| khz / 1000.0)
    };
    let cur = read_khz("scaling_cur_freq")
        .or_else(|| read_khz("cpuinfo_cur_freq"))
        .or_else(|| fallback_map.get(&core_id).copied())
        .unwrap_or(0.0);
    let min = read_khz("cpuinfo_min_freq")
        .or_else(|| read_khz("scaling_min_freq"))
        .unwrap_or(cur);
    let max = read_khz("cpuinfo_max_freq")
        .or_else(|| read_khz("scaling_max_freq"))
        .unwrap_or(cur);
    (cur, min, max)
}

pub fn parse_cpu_list_file(path: &str) -> HashSet<usize> {
    let mut set = HashSet::new();
    if let Ok(s) = fs::read_to_string(path) {
        for part in s.trim().split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((start_s, end_s)) = part.split_once('-') {
                if let (Ok(a), Ok(b)) = (start_s.parse::<usize>(), end_s.parse::<usize>()) {
                    for id in a..=b {
                        set.insert(id);
                    }
                }
            } else if let Ok(id) = part.parse::<usize>() {
                set.insert(id);
            }
        }
    }
    set
}

fn read_core_temperatures() -> BTreeMap<usize, f64> {
    let mut map = BTreeMap::new();
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let chip_name = fs::read_to_string(path.join("name"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !matches!(
            chip_name.as_str(),
            "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "acpitz"
        ) {
            continue;
        }
        for idx in 1..=32 {
            let input_path = path.join(format!("temp{}_input", idx));
            let Ok(raw_s) = fs::read_to_string(&input_path) else {
                continue;
            };
            let Ok(milli_c) = raw_s.trim().parse::<f64>() else {
                continue;
            };
            let temp_c = milli_c / 1000.0;
            if temp_c <= 1.0 || temp_c > 140.0 {
                continue;
            }
            let label = fs::read_to_string(path.join(format!("temp{}_label", idx)))
                .unwrap_or_default()
                .trim()
                .to_string();
            if let Some(rest) = label.strip_prefix("Core ") {
                if let Ok(cid) = rest.trim().parse::<usize>() {
                    map.insert(cid, temp_c);
                }
            } else if label.contains("Package")
                || label.contains("Tctl")
                || label.contains("Tdie")
                || idx == 1
            {
                map.entry(999).or_insert(temp_c);
            }
        }
    }
    // Fallback: check /sys/class/thermal/thermal_zone* if no CPU package temp was found
    if !map.contains_key(&999) {
        if let Ok(tz_entries) = fs::read_dir("/sys/class/thermal") {
            for entry in tz_entries.flatten() {
                let p = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("thermal_zone") {
                    let z_type = fs::read_to_string(p.join("type"))
                        .unwrap_or_default()
                        .to_lowercase();
                    if z_type.contains("cpu")
                        || z_type.contains("x86_pkg_temp")
                        || z_type.contains("acpitz")
                        || z_type.contains("soc")
                    {
                        if let Ok(raw_s) = fs::read_to_string(p.join("temp")) {
                            if let Ok(milli_c) = raw_s.trim().parse::<f64>() {
                                let c = milli_c / 1000.0;
                                if c > 1.0 && c <= 140.0 {
                                    map.insert(999, c);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

fn read_rapl_package_energy_uj() -> Option<u64> {
    let candidates = [
        "/sys/class/powercap/intel-rapl:0/energy_uj",
        "/sys/class/powercap/intel-rapl/intel-rapl:0/energy_uj",
    ];
    for p in candidates {
        if let Ok(s) = fs::read_to_string(p) {
            if let Ok(v) = s.trim().parse::<u64>() {
                return Some(v);
            }
        }
    }
    None
}

fn read_uptime_seconds() -> f64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or_else(|| System::uptime().unwrap_or(0) as f64)
}

fn read_vmstat_faults() -> (u64, u64) {
    let mut pgfault = 0;
    let mut pgmajfault = 0;
    if let Ok(content) = fs::read_to_string("/proc/vmstat") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("pgfault ") {
                pgfault = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("pgmajfault ") {
                pgmajfault = rest.trim().parse().unwrap_or(0);
            }
        }
    }
    (pgfault, pgmajfault)
}

fn sample_memory(page_faults_per_sec: f64, major_faults_per_sec: f64) -> MemoryTelemetry {
    let mut mem_total = 0u64;
    let mut mem_free = 0u64;
    let mut mem_avail = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut shmem = 0u64;
    let mut slab = 0u64;
    let mut sreclaimable = 0u64;
    let mut commit_limit = 0u64;
    let mut committed_as = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            let Some((k, rest)) = line.split_once(':') else {
                continue;
            };
            let kb = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let bytes = kb * 1024;
            match k.trim() {
                "MemTotal" => mem_total = bytes,
                "MemFree" => mem_free = bytes,
                "MemAvailable" => mem_avail = bytes,
                "Buffers" => buffers = bytes,
                "Cached" => cached = bytes,
                "Shmem" => shmem = bytes,
                "Slab" => slab = bytes,
                "SReclaimable" => sreclaimable = bytes,
                "CommitLimit" => commit_limit = bytes,
                "Committed_AS" => committed_as = bytes,
                "SwapTotal" => swap_total = bytes,
                "SwapFree" => swap_free = bytes,
                _ => {}
            }
        }
    }

    if mem_avail == 0 {
        mem_avail = mem_free + buffers + cached + sreclaimable;
    }
    let used_bytes = mem_total.saturating_sub(mem_avail);
    let swap_used = swap_total.saturating_sub(swap_free);
    let used_pct = if mem_total > 0 {
        ((used_bytes as f64 / mem_total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let swap_pct = if swap_total > 0 {
        ((swap_used as f64 / swap_total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let zswap_enabled = fs::read_to_string("/sys/module/zswap/parameters/enabled")
        .map(|s| matches!(s.trim(), "Y" | "y" | "1"))
        .unwrap_or_else(|_| Path::new("/sys/module/zswap").exists());

    MemoryTelemetry {
        total_bytes: mem_total,
        used_bytes,
        available_bytes: mem_avail,
        free_bytes: mem_free,
        buffers_bytes: buffers,
        cached_bytes: cached + sreclaimable,
        shared_bytes: shmem,
        slab_bytes: slab,
        committed_bytes: committed_as,
        commit_limit_bytes: commit_limit,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used,
        swap_free_bytes: swap_free,
        zswap_enabled,
        used_pct,
        swap_pct,
        page_faults_per_sec,
        major_faults_per_sec,
    }
}

fn sample_system_pressure(logical_cores: usize, mem: &MemoryTelemetry) -> SystemPressure {
    let mut load_1m = 0.0;
    let mut load_5m = 0.0;
    let mut load_15m = 0.0;

    if let Ok(s) = fs::read_to_string("/proc/loadavg") {
        let mut parts = s.split_whitespace();
        load_1m = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        load_5m = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        load_15m = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    }

    let (mem_some10, mem_full10) = read_psi_avg10("/proc/pressure/memory");
    let (cpu_some10, _) = read_psi_avg10("/proc/pressure/cpu");
    let (io_some10, _) = read_psi_avg10("/proc/pressure/io");

    let cores_f = (logical_cores.max(1)) as f64;
    let load_per_core = load_1m / cores_f;

    let load_score = (load_per_core * 55.0).clamp(0.0, 100.0);
    let psi_score = (mem_some10 * 2.5 + mem_full10 * 4.0 + io_some10 * 0.8).clamp(0.0, 100.0);
    let mem_headroom_penalty = if mem.used_pct > 85.0 {
        (mem.used_pct - 85.0) * 3.0
    } else {
        0.0
    };
    let score_percent = (load_score.max(psi_score) + mem_headroom_penalty).clamp(0.0, 100.0);

    let level = if load_per_core >= 1.50 || mem_some10 >= 25.0 || score_percent >= 80.0 {
        PressureLevel::Critical
    } else if load_per_core >= 1.00 || mem_some10 >= 10.0 || score_percent >= 55.0 {
        PressureLevel::High
    } else if load_per_core >= 0.65 || mem_some10 >= 2.0 || score_percent >= 35.0 {
        PressureLevel::Elevated
    } else {
        PressureLevel::Low
    };

    SystemPressure {
        level,
        score_percent,
        load_1m,
        load_5m,
        load_15m,
        load_per_core,
        mem_psi_some_avg10: mem_some10,
        mem_psi_full_avg10: mem_full10,
        cpu_psi_some_avg10: cpu_some10,
        io_psi_some_avg10: io_some10,
    }
}

fn read_psi_avg10(path: &str) -> (f64, f64) {
    let mut some10 = 0.0;
    let mut full10 = 0.0;
    if let Ok(content) = fs::read_to_string(path) {
        for line in content.lines() {
            let is_some = line.starts_with("some ");
            let is_full = line.starts_with("full ");
            if !is_some && !is_full {
                continue;
            }
            for token in line.split_whitespace() {
                if let Some(val_s) = token.strip_prefix("avg10=") {
                    if let Ok(v) = val_s.parse::<f64>() {
                        if is_some {
                            some10 = v;
                        } else {
                            full10 = v;
                        }
                    }
                }
            }
        }
    }
    (some10, full10)
}
