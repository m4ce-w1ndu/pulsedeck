use std::collections::BTreeMap;

/// 4-tier System Pressure level based on composite load average and kernel PSI metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    Low,
    Elevated,
    High,
    Critical,
}

impl PressureLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            PressureLevel::Low => "LOW",
            PressureLevel::Elevated => "ELEVATED",
            PressureLevel::High => "HIGH",
            PressureLevel::Critical => "CRITICAL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SystemPressure {
    pub level: PressureLevel,
    pub score_percent: f64,
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub load_per_core: f64,
    pub mem_psi_some_avg10: f64,
    pub mem_psi_full_avg10: f64,
    pub cpu_psi_some_avg10: f64,
    pub io_psi_some_avg10: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreKind {
    Standard,
    Performance,
    Efficiency,
}

impl CoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CoreKind::Standard => "Core",
            CoreKind::Performance => "P-Core",
            CoreKind::Efficiency => "E-Core",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogicalCoreStats {
    pub core_id: usize,
    pub kind: CoreKind,
    pub total_pct: f64,
    pub user_pct: f64,
    pub system_pct: f64,
    pub iowait_pct: f64,
    pub freq_mhz: f64,
    pub min_freq_mhz: f64,
    pub max_freq_mhz: f64,
    pub temp_c: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CpuTelemetry {
    pub model_name: String,
    pub vendor_id: String,
    pub architecture: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub p_cores: usize,
    pub e_cores: usize,
    pub total_pct: f64,
    pub user_pct: f64,
    pub system_pct: f64,
    pub iowait_pct: f64,
    pub irq_pct: f64,
    pub avg_freq_mhz: f64,
    pub max_freq_mhz: f64,
    pub package_temp_c: Option<f64>,
    pub package_power_watts: Option<f64>,
    pub context_switches_per_sec: f64,
    pub interrupts_per_sec: f64,
    pub processes_running: u64,
    pub processes_blocked: u64,
    pub uptime_seconds: f64,
    pub cores: Vec<LogicalCoreStats>,
}

#[derive(Debug, Clone)]
pub struct MemoryTelemetry {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub free_bytes: u64,
    pub buffers_bytes: u64,
    pub cached_bytes: u64,
    pub shared_bytes: u64,
    pub slab_bytes: u64,
    pub committed_bytes: u64,
    pub commit_limit_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_free_bytes: u64,
    pub zswap_enabled: bool,
    pub used_pct: f64,
    pub swap_pct: f64,
    pub page_faults_per_sec: f64,
    pub major_faults_per_sec: f64,
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    pub state: char,
    pub state_desc: &'static str,
    pub cpu_pct: f64,
    pub mem_rss_bytes: u64,
    pub mem_vms_bytes: u64,
    pub mem_pct: f64,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
    pub total_read_bytes: u64,
    pub total_write_bytes: u64,
    pub threads: u32,
    pub nice: i32,
    pub start_time_ticks: u64,
    pub cpu_time_seconds: f64,
    pub is_kernel_thread: bool,
    pub gpu_pct: f64,
    pub tree_depth: usize,
}

#[derive(Debug, Clone)]
pub struct OpenFdEntry {
    pub fd: i32,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct ProcessInspectorData {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub exe_path: String,
    pub cwd: String,
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    pub state: String,
    pub threads: u32,
    pub nice: i32,
    pub cpu_affinity_hex: String,
    pub voluntary_ctxt_switches: u64,
    pub nonvoluntary_ctxt_switches: u64,
    pub rss_bytes: u64,
    pub vms_bytes: u64,
    pub shared_bytes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub environ: Vec<(String, String)>,
    pub open_fds: Vec<OpenFdEntry>,
    pub memory_maps_count: usize,
    pub top_mapped_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiskDeviceStats {
    pub name: String,
    pub model: String,
    pub kind: String, // "NVMe", "SSD", "HDD", "Virtual"
    pub capacity_bytes: u64,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
    pub read_iops: f64,
    pub write_iops: f64,
    pub busy_pct: f64,
    pub total_read_bytes: u64,
    pub total_written_bytes: u64,
    pub mount_points: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NetworkAdapterStats {
    pub name: String,
    pub kind: String, // "Ethernet", "Wi-Fi", "Loopback", "Virtual"
    pub mac_address: String,
    pub ipv4: String,
    pub ipv6: String,
    pub oper_state: String,
    pub speed_mbps: Option<u64>,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub rx_packets_per_sec: f64,
    pub tx_packets_per_sec: f64,
    pub total_rx_bytes: u64,
    pub total_tx_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct GpuDeviceStats {
    pub card_id: String,
    pub name: String,
    pub driver: String,
    pub vendor_id: String,
    pub device_id: String,
    pub pci_slot: String,
    pub pcie_link: String,
    pub busy_pct: f64,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub gtt_used_bytes: Option<u64>,
    pub clock_mhz: Option<f64>,
    pub max_clock_mhz: Option<f64>,
    pub temp_c: Option<f64>,
    pub power_watts: Option<f64>,
    pub fan_rpm: Option<u32>,
}

/// Live TCP/UDP/Unix socket connections with PID mapping
#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    pub protocol: String, // "TCP", "TCP6", "UDP", "UDP6", "UNIX"
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
    pub state: String,
    pub inode: u64,
    pub uid: u32,
    pub pid: Option<u32>,
    pub process_name: String,
    pub is_public_listener: bool,
}

/// Live Power & Frequency sensors with Current/Min/Max
#[derive(Debug, Clone)]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub category: String, // "Frequency", "Temperature", "Power", "Voltage", "Fan"
    pub unit: String,     // "MHz", "°C", "W", "V", "RPM"
    pub current: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone)]
pub struct SensorGroup {
    pub chip_name: String,
    pub adapter: String,
    pub readings: Vec<SensorReading>,
}

/// Disk Space Analyzer report
#[derive(Debug, Clone)]
pub struct DirUsageEntry {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub pct_of_root: f64,
}

#[derive(Debug, Clone)]
pub struct FileCategoryEntry {
    pub category: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub pct_of_root: f64,
}

#[derive(Debug, Clone)]
pub struct LargeFileEntry {
    pub path: String,
    pub size_bytes: u64,
    pub category: String,
}

#[derive(Debug, Clone)]
pub struct DiskSpaceReport {
    pub root_path: String,
    pub scanned_at_epoch: u64,
    pub scan_duration_ms: u64,
    pub fs_total_bytes: u64,
    pub fs_used_bytes: u64,
    pub fs_free_bytes: u64,
    pub scanned_bytes: u64,
    pub scanned_files: u64,
    pub scanned_dirs: u64,
    pub top_directories: Vec<DirUsageEntry>,
    pub categories: Vec<FileCategoryEntry>,
    pub largest_files: Vec<LargeFileEntry>,
}

/// Kernel Modules & System Drivers
#[derive(Debug, Clone)]
pub struct KernelModuleEntry {
    pub name: String,
    pub size_bytes: u64,
    pub ref_count: u32,
    pub used_by: Vec<String>,
    pub state: String, // "Live", "Loading", "Unloading"
    pub coresize: u64,
    pub srcversion: String,
    pub in_tree: bool,
    pub module_path: String,
}

/// Flight Recorder telemetry trace (.pulsetrace format)
#[derive(Debug, Clone)]
pub struct FlightTraceSample {
    pub index: usize,
    pub timestamp_epoch_ms: u64,
    pub cpu_total_pct: f64,
    pub cpu_avg_freq_mhz: f64,
    pub cpu_temp_c: f64,
    pub mem_used_pct: f64,
    pub mem_used_bytes: u64,
    pub swap_used_bytes: u64,
    pub pressure_level: String,
    pub pressure_score: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub top_process_name: String,
    pub top_process_pid: u32,
    pub top_process_cpu_pct: f64,
}

#[derive(Debug, Clone)]
pub struct FlightTraceFile {
    pub schema: String, // "io.pulsedeck.traces"
    pub version: u32,   // 1
    pub hostname: String,
    pub cpu_model: String,
    pub recorded_at_epoch: u64,
    pub samples: Vec<FlightTraceSample>,
}

/// Hardware Benchmarks & Composite PulseDeck Score
#[derive(Debug, Clone)]
pub struct BenchmarkSubScore {
    pub name: String,
    pub metric_value: f64,
    pub metric_unit: String,
    pub normalized_score: u32, // baseline = 100
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct BenchmarkReport {
    pub timestamp_epoch: u64,
    pub composite_score: u32, // PulseDeck Score (baseline = 100)
    pub cpu_single_core: BenchmarkSubScore,
    pub cpu_multi_core: BenchmarkSubScore,
    pub memory_bandwidth: BenchmarkSubScore,
    pub disk_throughput: BenchmarkSubScore,
}

/// Installed Applications Inventory
#[derive(Debug, Clone)]
pub struct InstalledAppEntry {
    pub name: String,
    pub version: String,
    pub source: String, // "dpkg", "flatpak", "snap", "desktop"
    pub section: String,
    pub installed_size_kb: u64,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ServiceEntry {
    pub unit: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct StartupEntry {
    pub name: String,
    pub exec: String,
    pub scope: String, // "User (~/.config/autostart)" or "System (/etc/xdg/autostart)"
    pub enabled: bool,
    pub comment: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct UserSessionEntry {
    pub username: String,
    pub uid: u32,
    pub tty: String,
    pub remote_host: String,
    pub login_time: String,
    pub process_count: usize,
    pub cpu_pct: f64,
    pub rss_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct AppHistoryEntry {
    pub app_name: String,
    pub cumulative_cpu_seconds: f64,
    pub peak_rss_bytes: u64,
    pub total_read_bytes: u64,
    pub total_write_bytes: u64,
    pub instance_count: usize,
}

#[derive(Debug, Clone)]
pub struct SystemInfoCatalog {
    pub hostname: String,
    pub os_pretty_name: String,
    pub kernel_release: String,
    pub architecture: String,
    pub machine_id_preview: String,
    pub desktop_env: String,
    pub session_type: String,
    pub board_vendor: String,
    pub board_name: String,
    pub bios_version: String,
    pub sections: BTreeMap<String, Vec<(String, String)>>,
}

/// Complete telemetry snapshot produced on every tick by `LinuxSystemSampler`.
#[derive(Debug, Clone)]
pub struct SystemSnapshot {
    pub seq: u64,
    pub timestamp_epoch_ms: u64,
    pub pressure: SystemPressure,
    pub cpu: CpuTelemetry,
    pub memory: MemoryTelemetry,
    pub processes: Vec<ProcessEntry>,
    pub disks: Vec<DiskDeviceStats>,
    pub networks: Vec<NetworkAdapterStats>,
    pub gpus: Vec<GpuDeviceStats>,
    pub connections: Vec<ConnectionEntry>,
    pub sensors: Vec<SensorGroup>,
    pub app_history: Vec<AppHistoryEntry>,
}
