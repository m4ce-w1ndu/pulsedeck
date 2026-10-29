pub mod cpu_mem;
pub mod diagnostics;
pub mod io_gpu;
pub mod processes;
pub mod system_catalog;

use crate::model::{
    BenchmarkReport, ConnectionEntry, DiskSpaceReport, FlightTraceFile, InstalledAppEntry,
    KernelModuleEntry, ServiceEntry, StartupEntry, SystemInfoCatalog, SystemSnapshot,
    UserSessionEntry,
};
use cpu_mem::CpuMemCollector;
use diagnostics::{
    FlightRecorder, SensorCollector, analyze_disk_space, collect_connections,
    collect_installed_apps, collect_kernel_modules, run_benchmarks,
};
use io_gpu::IoGpuCollector;
use processes::ProcessCollector;
use std::time::{SystemTime, UNIX_EPOCH};
use system_catalog::{
    build_system_info_catalog, collect_services, collect_startup_entries, collect_user_sessions,
};

pub struct LinuxSystemSampler {
    seq: u64,
    cpu_mem: CpuMemCollector,
    procs: ProcessCollector,
    io_gpu: IoGpuCollector,
    sensors: SensorCollector,
    pub flight_recorder: FlightRecorder,
    cached_connections: Vec<ConnectionEntry>,
    pub drivers: Vec<KernelModuleEntry>,
    pub disk_space: DiskSpaceReport,
    pub disk_space_history: Vec<DiskSpaceReport>,
    pub benchmarks: Option<BenchmarkReport>,
    pub installed_apps: Vec<InstalledAppEntry>,
    pub services: Vec<ServiceEntry>,
    pub startup_apps: Vec<StartupEntry>,
    pub users: Vec<UserSessionEntry>,
    pub system_info: Option<SystemInfoCatalog>,
}

impl LinuxSystemSampler {
    pub fn new() -> Self {
        let mut cpu_mem = CpuMemCollector::new();
        let (init_cpu, _, _) = cpu_mem.sample();
        let hostname = std::fs::read_to_string("/etc/hostname")
            .unwrap_or_else(|_| String::from("linux-host"))
            .trim()
            .to_string();

        let default_scan_root = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| String::from("/home"));
        let initial_disk_space = analyze_disk_space(&default_scan_root, 15_000);

        Self {
            seq: 0,
            cpu_mem,
            procs: ProcessCollector::new(),
            io_gpu: IoGpuCollector::new(),
            sensors: SensorCollector::new(),
            flight_recorder: FlightRecorder::new(hostname, init_cpu.model_name),
            cached_connections: collect_connections(),
            drivers: collect_kernel_modules(),
            disk_space_history: vec![initial_disk_space.clone()],
            disk_space: initial_disk_space,
            benchmarks: None,
            installed_apps: collect_installed_apps(),
            services: collect_services(),
            startup_apps: collect_startup_entries(),
            users: Vec::new(),
            system_info: None,
        }
    }

    pub fn sample_tick(&mut self) -> SystemSnapshot {
        self.seq += 1;
        let timestamp_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let (cpu, memory, pressure) = self.cpu_mem.sample();
        let (processes, app_history) = self.procs.sample(memory.total_bytes);
        let (disks, networks, gpus) = self.io_gpu.sample();
        let sensors = self.sensors.sample(&cpu);

        // Refresh live socket connections every 2 ticks (or on first tick)
        if self.seq == 1 || self.seq.is_multiple_of(2) {
            self.cached_connections = collect_connections();
        }
        // Update active user sessions from current processes
        if self.seq == 1 || self.seq.is_multiple_of(3) {
            self.users = collect_user_sessions(&processes);
        }

        let snap = SystemSnapshot {
            seq: self.seq,
            timestamp_epoch_ms,
            pressure,
            cpu,
            memory,
            processes,
            disks,
            networks,
            gpus,
            connections: self.cached_connections.clone(),
            sensors,
            app_history,
        };

        if self.system_info.is_none() || self.seq.is_multiple_of(10) {
            self.system_info = Some(build_system_info_catalog(&snap));
        }

        self.flight_recorder.push_snapshot(&snap);
        snap
    }

    pub fn rescan_disk_space(&mut self, path: &str) -> &DiskSpaceReport {
        let report = analyze_disk_space(path, 25_000);
        self.disk_space_history.push(report.clone());
        if self.disk_space_history.len() > 10 {
            self.disk_space_history.remove(0);
        }
        self.disk_space = report;
        &self.disk_space
    }

    pub fn run_all_benchmarks(&mut self, logical_cores: usize) -> &BenchmarkReport {
        let rep = run_benchmarks(logical_cores);
        self.benchmarks = Some(rep);
        self.benchmarks.as_ref().unwrap()
    }

    pub fn refresh_catalogs(&mut self) {
        self.drivers = collect_kernel_modules();
        self.installed_apps = collect_installed_apps();
        self.services = collect_services();
        self.startup_apps = collect_startup_entries();
    }

    pub fn export_flight_trace(&self) -> &FlightTraceFile {
        &self.flight_recorder.trace
    }
}
