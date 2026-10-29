use crate::model::{AppHistoryEntry, OpenFdEntry, ProcessEntry, ProcessInspectorData};
use anyhow::{Context, Result, anyhow};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, Users};

#[derive(Debug, Clone, Copy)]
struct PrevProcSample {
    cpu_ticks: u64,
    read_bytes: u64,
    write_bytes: u64,
    gpu_ns: u64,
}

#[derive(Debug, Clone, Default)]
struct AppHistoryAccum {
    base_cpu_sec: f64,
    active_pid_cpu: FxHashMap<u32, f64>,
    peak_rss_bytes: u64,
    base_read_bytes: u64,
    active_pid_read: FxHashMap<u32, u64>,
    base_write_bytes: u64,
    active_pid_write: FxHashMap<u32, u64>,
    current_instances: usize,
}

pub struct ProcessCollector {
    prev_procs: FxHashMap<u32, PrevProcSample>,
    prev_instant: Instant,
    uid_map: FxHashMap<u32, String>,
    clk_tck: f64,
    page_size: u64,
    app_history: BTreeMap<String, AppHistoryAccum>,
    sys: System,
    users: Users,
}

impl ProcessCollector {
    pub fn new() -> Self {
        #[cfg(unix)]
        let (clk_tck, page_size) = {
            let ct = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            (
                if ct > 0 { ct as f64 } else { 100.0 },
                if ps > 0 { ps as u64 } else { 4096 },
            )
        };
        #[cfg(not(unix))]
        let (clk_tck, page_size) = (100.0, 4096u64);

        let sys = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
        )
        .expect("Failed to initialize sysinfo System");
        let users =
            Users::new_with_refreshed_list().expect("Failed to initialize sysinfo Users");

        Self {
            prev_procs: FxHashMap::default(),
            prev_instant: Instant::now(),
            uid_map: load_passwd_uid_map(),
            clk_tck,
            page_size,
            app_history: BTreeMap::new(),
            sys,
            users,
        }
    }

    pub fn sample(&mut self, total_mem_bytes: u64) -> (Vec<ProcessEntry>, Vec<AppHistoryEntry>) {
        if !cfg!(target_os = "linux") || !Path::new("/proc").exists() {
            return self.sample_cross_platform(total_mem_bytes);
        }

        let now = Instant::now();
        let dt = now.duration_since(self.prev_instant).as_secs_f64().max(0.05);
        self.prev_instant = now;

        let mut next_prev = FxHashMap::default();
        let mut list = Vec::with_capacity(512);

        for accum in self.app_history.values_mut() {
            accum.current_instances = 0;
        }

        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let Some(name_str) = file_name.to_str() else {
                    continue;
                };
                let Ok(pid) = name_str.parse::<u32>() else {
                    continue;
                };
                if let Some((mut proc_entry, raw_sample)) =
                    self.read_single_process(pid, dt, total_mem_bytes)
                {
                    if !proc_entry.is_kernel_thread {
                        let accum = self
                            .app_history
                            .entry(proc_entry.name.clone())
                            .or_default();
                        accum.current_instances += 1;
                        if proc_entry.mem_rss_bytes > accum.peak_rss_bytes {
                            accum.peak_rss_bytes = proc_entry.mem_rss_bytes;
                        }
                        accum
                            .active_pid_cpu
                            .insert(pid, proc_entry.cpu_time_seconds);
                        accum
                            .active_pid_read
                            .insert(pid, proc_entry.total_read_bytes);
                        accum
                            .active_pid_write
                            .insert(pid, proc_entry.total_write_bytes);
                    }
                    next_prev.insert(pid, raw_sample);
                    proc_entry.tree_depth = 0;
                    list.push(proc_entry);
                }
            }
        }

        for accum in self.app_history.values_mut() {
            accum.active_pid_cpu.retain(|pid, val| {
                if next_prev.contains_key(pid) {
                    true
                } else {
                    accum.base_cpu_sec += *val;
                    false
                }
            });
            accum.active_pid_read.retain(|pid, val| {
                if next_prev.contains_key(pid) {
                    true
                } else {
                    accum.base_read_bytes += *val;
                    false
                }
            });
            accum.active_pid_write.retain(|pid, val| {
                if next_prev.contains_key(pid) {
                    true
                } else {
                    accum.base_write_bytes += *val;
                    false
                }
            });
        }

        self.prev_procs = next_prev;
        self.finalize_process_list(&mut list);
        let history = self.build_history_list();
        (list, history)
    }

    fn sample_cross_platform(
        &mut self,
        total_mem_bytes: u64,
    ) -> (Vec<ProcessEntry>, Vec<AppHistoryEntry>) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_instant).as_secs_f64().max(0.05);
        self.prev_instant = now;

        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        for accum in self.app_history.values_mut() {
            accum.current_instances = 0;
        }

        let mut list = Vec::with_capacity(self.sys.processes().len());
        for (pid_obj, proc_info) in self.sys.processes() {
            let pid = pid_obj.as_u32();
            let ppid = proc_info.parent().map(|p| p.as_u32()).unwrap_or(0);
            let name = proc_info.name().to_string_lossy().to_string();
            let cmd_vec: Vec<String> = proc_info
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect();
            let cmdline = if cmd_vec.is_empty() {
                name.clone()
            } else {
                cmd_vec.join(" ")
            };
            let user = proc_info
                .user_id()
                .and_then(|uid| self.users.get_user_by_id(uid))
                .map(|u| u.name().to_string())
                .unwrap_or_else(|| String::from("user"));

            let rss = proc_info.memory();
            let vms = proc_info.virtual_memory();
            let cpu_pct = proc_info.cpu_usage() as f64;
            let du = proc_info.disk_usage();
            let read_bps = du.read_bytes as f64 / dt;
            let write_bps = du.written_bytes as f64 / dt;
            let cpu_time_seconds = proc_info.accumulated_cpu_time() as f64 / 1000.0;

            let mem_pct = if total_mem_bytes > 0 {
                ((rss as f64 / total_mem_bytes as f64) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };

            let accum = self.app_history.entry(name.clone()).or_default();
            accum.current_instances += 1;
            if rss > accum.peak_rss_bytes {
                accum.peak_rss_bytes = rss;
            }
            accum.active_pid_cpu.insert(pid, cpu_time_seconds);
            accum.active_pid_read.insert(pid, du.total_read_bytes);
            accum.active_pid_write.insert(pid, du.total_written_bytes);

            list.push(ProcessEntry {
                pid,
                ppid,
                name,
                cmdline,
                user,
                uid: 0,
                state: 'R',
                state_desc: "Running",
                cpu_pct,
                mem_rss_bytes: rss,
                mem_vms_bytes: vms,
                mem_pct,
                read_bytes_per_sec: read_bps,
                write_bytes_per_sec: write_bps,
                total_read_bytes: du.total_read_bytes,
                total_write_bytes: du.total_written_bytes,
                threads: 1,
                nice: 0,
                start_time_ticks: proc_info.start_time(),
                cpu_time_seconds,
                is_kernel_thread: false,
                gpu_pct: 0.0,
                tree_depth: 0,
            });
        }

        self.finalize_process_list(&mut list);
        let history = self.build_history_list();
        (list, history)
    }

    fn finalize_process_list(&self, list: &mut [ProcessEntry]) {
        let ppid_map: FxHashMap<u32, u32> = list.iter().map(|p| (p.pid, p.ppid)).collect();
        for proc_entry in list.iter_mut() {
            let mut depth = 0usize;
            let mut cur = proc_entry.ppid;
            while cur > 1 && depth < 12 {
                if let Some(&parent) = ppid_map.get(&cur) {
                    if parent == cur {
                        break;
                    }
                    depth += 1;
                    cur = parent;
                } else {
                    break;
                }
            }
            proc_entry.tree_depth = depth;
        }

        list.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.mem_rss_bytes.cmp(&a.mem_rss_bytes))
        });
    }

    fn build_history_list(&self) -> Vec<AppHistoryEntry> {
        let mut history: Vec<AppHistoryEntry> = self
            .app_history
            .iter()
            .map(|(app_name, acc)| {
                let cpu_sec = acc.base_cpu_sec + acc.active_pid_cpu.values().sum::<f64>();
                let read_b = acc.base_read_bytes + acc.active_pid_read.values().sum::<u64>();
                let write_b = acc.base_write_bytes + acc.active_pid_write.values().sum::<u64>();
                AppHistoryEntry {
                    app_name: app_name.clone(),
                    cumulative_cpu_seconds: cpu_sec,
                    peak_rss_bytes: acc.peak_rss_bytes,
                    total_read_bytes: read_b,
                    total_write_bytes: write_b,
                    instance_count: acc.current_instances,
                }
            })
            .collect();

        history.sort_by(|a, b| {
            b.cumulative_cpu_seconds
                .partial_cmp(&a.cumulative_cpu_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.peak_rss_bytes.cmp(&a.peak_rss_bytes))
        });
        if history.len() > 150 {
            history.truncate(150);
        }
        history
    }

    fn read_single_process(
        &self,
        pid: u32,
        dt: f64,
        total_mem_bytes: u64,
    ) -> Option<(ProcessEntry, PrevProcSample)> {
        let stat_path = format!("/proc/{}/stat", pid);
        let stat_str = fs::read_to_string(&stat_path).ok()?;

        let open_paren = stat_str.find('(')?;
        let close_paren = stat_str.rfind(')')?;
        if close_paren <= open_paren {
            return None;
        }
        let comm = stat_str[open_paren + 1..close_paren].to_string();
        let after = stat_str[close_paren + 1..].trim_start();
        let fields: Vec<&str> = after.split_whitespace().collect();
        if fields.len() < 22 {
            return None;
        }

        let state = fields[0].chars().next().unwrap_or('S');
        let ppid: u32 = fields[1].parse().unwrap_or(0);
        let utime: u64 = fields[11].parse().unwrap_or(0);
        let stime: u64 = fields[12].parse().unwrap_or(0);
        let nice: i32 = fields[16].parse().unwrap_or(0);
        let threads: u32 = fields[17].parse().unwrap_or(1);
        let starttime: u64 = fields[19].parse().unwrap_or(0);
        let vsize_bytes: u64 = fields[20].parse().unwrap_or(0);
        let rss_pages: i64 = fields[21].parse().unwrap_or(0);
        let rss_bytes = if rss_pages > 0 {
            (rss_pages as u64) * self.page_size
        } else {
            0
        };

        let cpu_ticks = utime + stime;
        let cpu_time_seconds = cpu_ticks as f64 / self.clk_tck;

        let mut uid = 0u32;
        let status_path = format!("/proc/{}/status", pid);
        if let Ok(status_str) = fs::read_to_string(&status_path) {
            for line in status_str.lines() {
                if let Some(rest) = line.strip_prefix("Uid:") {
                    if let Some(first_uid) = rest.split_whitespace().next() {
                        uid = first_uid.parse().unwrap_or(0);
                    }
                    break;
                }
            }
        }
        let user = self
            .uid_map
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string());

        let cmdline_raw = fs::read(format!("/proc/{}/cmdline", pid)).unwrap_or_default();
        let is_kernel_thread =
            cmdline_raw.is_empty() && (ppid == 2 || pid == 2 || vsize_bytes == 0);
        let cmdline = if cmdline_raw.is_empty() {
            format!("[{}]", comm)
        } else {
            String::from_utf8_lossy(&cmdline_raw)
                .replace('\0', " ")
                .trim()
                .to_string()
        };

        let mut read_bytes = 0u64;
        let mut write_bytes = 0u64;
        if let Ok(io_str) = fs::read_to_string(format!("/proc/{}/io", pid)) {
            for line in io_str.lines() {
                if let Some(r) = line.strip_prefix("read_bytes:") {
                    read_bytes = r.trim().parse().unwrap_or(0);
                } else if let Some(w) = line.strip_prefix("write_bytes:") {
                    write_bytes = w.trim().parse().unwrap_or(0);
                }
            }
        }

        let gpu_ns = 0u64;
        let (cpu_pct, read_bps, write_bps, gpu_pct) =
            if let Some(prev) = self.prev_procs.get(&pid) {
                let d_ticks = cpu_ticks.saturating_sub(prev.cpu_ticks) as f64;
                let c_pct = ((d_ticks / self.clk_tck) / dt) * 100.0;
                let r_bps = read_bytes.saturating_sub(prev.read_bytes) as f64 / dt;
                let w_bps = write_bytes.saturating_sub(prev.write_bytes) as f64 / dt;
                let g_pct = ((gpu_ns.saturating_sub(prev.gpu_ns) as f64 / 1_000_000_000.0) / dt
                    * 100.0)
                    .clamp(0.0, 100.0);
                (c_pct, r_bps, w_bps, g_pct)
            } else {
                (0.0, 0.0, 0.0, 0.0)
            };

        let mem_pct = if total_mem_bytes > 0 {
            ((rss_bytes as f64 / total_mem_bytes as f64) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        let state_desc = match state {
            'R' => "Running",
            'S' => "Sleeping",
            'D' => "Disk Sleep",
            'Z' => "Zombie",
            'T' | 't' => "Stopped",
            'I' => "Idle",
            _ => "Other",
        };

        let entry = ProcessEntry {
            pid,
            ppid,
            name: comm,
            cmdline,
            user,
            uid,
            state,
            state_desc,
            cpu_pct,
            mem_rss_bytes: rss_bytes,
            mem_vms_bytes: vsize_bytes,
            mem_pct,
            read_bytes_per_sec: read_bps,
            write_bytes_per_sec: write_bps,
            total_read_bytes: read_bytes,
            total_write_bytes: write_bytes,
            threads,
            nice,
            start_time_ticks: starttime,
            cpu_time_seconds,
            is_kernel_thread,
            gpu_pct,
            tree_depth: 0,
        };

        let sample = PrevProcSample {
            cpu_ticks,
            read_bytes,
            write_bytes,
            gpu_ns,
        };

        Some((entry, sample))
    }
}

pub fn load_passwd_uid_map() -> FxHashMap<u32, String> {
    let mut map = FxHashMap::default();
    if let Ok(content) = fs::read_to_string("/etc/passwd") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 {
                if let Ok(uid) = parts[2].parse::<u32>() {
                    map.insert(uid, parts[0].to_string());
                }
            }
        }
    }
    map
}

pub fn inspect_process(pid: u32) -> Result<ProcessInspectorData> {
    let proc_dir = format!("/proc/{}", pid);
    if !Path::new(&proc_dir).exists() {
        return inspect_process_cross_platform(pid);
    }

    let uid_map = load_passwd_uid_map();
    let mut name = String::new();
    let mut ppid = 0u32;
    let mut uid = 0u32;
    let mut state = String::from("Unknown");
    let mut threads = 1u32;
    let mut vol_ctxt = 0u64;
    let mut nonvol_ctxt = 0u64;
    let mut rss_bytes = 0u64;
    let mut vms_bytes = 0u64;
    let mut shared_bytes = 0u64;
    let mut cpu_affinity_hex = String::from("all");

    if let Ok(status_str) = fs::read_to_string(format!("{}/status", proc_dir)) {
        for line in status_str.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let val = v.trim();
            match k.trim() {
                "Name" => name = val.to_string(),
                "State" => state = val.to_string(),
                "PPid" => ppid = val.parse().unwrap_or(0),
                "Uid" => {
                    uid = val
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                }
                "Threads" => threads = val.parse().unwrap_or(1),
                "VmSize" => {
                    vms_bytes = val
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0)
                        * 1024;
                }
                "VmRSS" => {
                    rss_bytes = val
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0)
                        * 1024;
                }
                "RssFile" | "RssShmem" => {
                    shared_bytes += val
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0)
                        * 1024;
                }
                "Cpus_allowed" => cpu_affinity_hex = val.to_string(),
                "voluntary_ctxt_switches" => vol_ctxt = val.parse().unwrap_or(0),
                "nonvoluntary_ctxt_switches" => nonvol_ctxt = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    let mut nice = 0i32;
    if let Ok(stat_str) = fs::read_to_string(format!("{}/stat", proc_dir)) {
        if let Some(close_paren) = stat_str.rfind(')') {
            let fields: Vec<&str> = stat_str[close_paren + 1..].split_whitespace().collect();
            if fields.len() > 16 {
                nice = fields[16].parse().unwrap_or(0);
            }
        }
    }

    let exe_path = fs::read_link(format!("{}/exe", proc_dir))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| String::from("<permission denied or kernel thread>"));

    let cwd = fs::read_link(format!("{}/cwd", proc_dir))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| String::from("<permission denied>"));

    let cmdline_raw = fs::read(format!("{}/cmdline", proc_dir)).unwrap_or_default();
    let cmdline = if cmdline_raw.is_empty() {
        format!("[{}]", name)
    } else {
        String::from_utf8_lossy(&cmdline_raw)
            .replace('\0', " ")
            .trim()
            .to_string()
    };

    let mut read_bytes = 0u64;
    let mut write_bytes = 0u64;
    if let Ok(io_str) = fs::read_to_string(format!("{}/io", proc_dir)) {
        for line in io_str.lines() {
            if let Some(r) = line.strip_prefix("read_bytes:") {
                read_bytes = r.trim().parse().unwrap_or(0);
            } else if let Some(w) = line.strip_prefix("write_bytes:") {
                write_bytes = w.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut environ = Vec::new();
    if let Ok(env_raw) = fs::read(format!("{}/environ", proc_dir)) {
        for item in env_raw.split(|&b| b == 0) {
            if item.is_empty() {
                continue;
            }
            let s = String::from_utf8_lossy(item);
            if let Some((k, v)) = s.split_once('=') {
                environ.push((k.to_string(), v.to_string()));
            }
            if environ.len() >= 100 {
                break;
            }
        }
    }

    let mut open_fds = Vec::new();
    if let Ok(fd_entries) = fs::read_dir(format!("{}/fd", proc_dir)) {
        for entry in fd_entries.flatten() {
            if let Some(fd_str) = entry.file_name().to_str() {
                if let Ok(fd_num) = fd_str.parse::<i32>() {
                    let target = fs::read_link(entry.path())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| String::from("?"));
                    open_fds.push(OpenFdEntry { fd: fd_num, target });
                    if open_fds.len() >= 150 {
                        break;
                    }
                }
            }
        }
    }
    open_fds.sort_by_key(|f| f.fd);

    let mut memory_maps_count = 0usize;
    let mut mapped_set = BTreeMap::new();
    if let Ok(maps_str) = fs::read_to_string(format!("{}/maps", proc_dir)) {
        for line in maps_str.lines() {
            memory_maps_count += 1;
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let pathname = parts[5..].join(" ");
                if pathname.starts_with('/') {
                    *mapped_set.entry(pathname).or_insert(0usize) += 1;
                }
            }
        }
    }
    let top_mapped_files: Vec<String> = mapped_set.into_keys().take(40).collect();

    let user = uid_map.get(&uid).cloned().unwrap_or_else(|| uid.to_string());

    Ok(ProcessInspectorData {
        pid,
        ppid,
        name,
        exe_path,
        cwd,
        cmdline,
        user,
        uid,
        state,
        threads,
        nice,
        cpu_affinity_hex,
        voluntary_ctxt_switches: vol_ctxt,
        nonvoluntary_ctxt_switches: nonvol_ctxt,
        rss_bytes,
        vms_bytes,
        shared_bytes,
        read_bytes,
        write_bytes,
        environ,
        open_fds,
        memory_maps_count,
        top_mapped_files,
    })
}

fn inspect_process_cross_platform(pid: u32) -> Result<ProcessInspectorData> {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    )
    .map_err(|e| anyhow!("sysinfo init error: {}", e))?;
    let pid_obj = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid_obj]), true);
    let proc_info = sys
        .process(pid_obj)
        .ok_or_else(|| anyhow!("Process {} no longer exists", pid))?;

    let mut environ = Vec::new();
    for env_os in proc_info.environ() {
        let s = env_os.to_string_lossy();
        if let Some((k, v)) = s.split_once('=') {
            environ.push((k.to_string(), v.to_string()));
        }
    }
    let exe_path = proc_info
        .exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| String::from("N/A"));
    let cwd = proc_info
        .cwd()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| String::from("N/A"));
    let cmd_vec: Vec<String> = proc_info
        .cmd()
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    let du = proc_info.disk_usage();

    Ok(ProcessInspectorData {
        pid,
        ppid: proc_info.parent().map(|p| p.as_u32()).unwrap_or(0),
        name: proc_info.name().to_string_lossy().to_string(),
        exe_path: exe_path.clone(),
        cwd,
        cmdline: cmd_vec.join(" "),
        user: String::from("user"),
        uid: 0,
        state: format!("{:?}", proc_info.status()),
        threads: 1,
        nice: 0,
        cpu_affinity_hex: String::from("all"),
        voluntary_ctxt_switches: 0,
        nonvoluntary_ctxt_switches: 0,
        rss_bytes: proc_info.memory(),
        vms_bytes: proc_info.virtual_memory(),
        shared_bytes: 0,
        read_bytes: du.total_read_bytes,
        write_bytes: du.total_written_bytes,
        environ,
        open_fds: vec![OpenFdEntry {
            fd: 0,
            target: exe_path,
        }],
        memory_maps_count: 1,
        top_mapped_files: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy)]
pub enum ProcessAction {
    Terminate,
    Kill,
    Stop,
    Continue,
    Interrupt,
    Hangup,
    TerminateTree,
    SetNice(i32),
}

pub fn control_process(pid: u32, action: ProcessAction) -> Result<String> {
    if pid <= 1 {
        return Err(anyhow!("Refusing to signal PID {}", pid));
    }
    #[cfg(unix)]
    {
        match action {
            ProcessAction::Terminate => send_signal(pid, libc::SIGTERM, "SIGTERM"),
            ProcessAction::Kill => send_signal(pid, libc::SIGKILL, "SIGKILL"),
            ProcessAction::Stop => send_signal(pid, libc::SIGSTOP, "SIGSTOP"),
            ProcessAction::Continue => send_signal(pid, libc::SIGCONT, "SIGCONT"),
            ProcessAction::Interrupt => send_signal(pid, libc::SIGINT, "SIGINT"),
            ProcessAction::Hangup => send_signal(pid, libc::SIGHUP, "SIGHUP"),
            ProcessAction::TerminateTree => {
                let children = collect_descendant_pids(pid);
                let mut count = 0usize;
                for child_pid in children {
                    let _ = send_signal(child_pid, libc::SIGTERM, "SIGTERM");
                    count += 1;
                }
                send_signal(pid, libc::SIGTERM, "SIGTERM")?;
                Ok(format!(
                    "Sent SIGTERM to PID {} and {} descendant processes",
                    pid, count
                ))
            }
            ProcessAction::SetNice(nice_val) => {
                let clamped = nice_val.clamp(-20, 19);
                let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid, clamped) };
                if rc == 0 {
                    Ok(format!("Updated PID {} nice priority to {}", pid, clamped))
                } else {
                    Err(std::io::Error::last_os_error())
                        .with_context(|| format!("Failed to set nice={} on PID {}", clamped, pid))
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
        )
        .map_err(|e| anyhow!("sysinfo init error: {}", e))?;
        let pid_obj = Pid::from_u32(pid);
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid_obj]), true);
        if let Some(p) = sys.process(pid_obj) {
            if matches!(
                action,
                ProcessAction::Terminate | ProcessAction::Kill | ProcessAction::TerminateTree
            ) {
                if p.kill() {
                    return Ok(format!("Terminated process PID {}", pid));
                }
            }
        }
        Err(anyhow!("Operation not supported or permission denied for PID {}", pid))
    }
}

#[cfg(unix)]
fn send_signal(pid: u32, sig: i32, sig_name: &str) -> Result<String> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(format!("Sent {} to PID {}", sig_name, pid))
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to send {} to PID {}", sig_name, pid))
    }
}

#[cfg(unix)]
fn collect_descendant_pids(root_pid: u32) -> Vec<u32> {
    let mut children_map: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(s) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            let Ok(pid) = s.parse::<u32>() else {
                continue;
            };
            if let Ok(stat_str) = fs::read_to_string(format!("/proc/{}/stat", pid)) {
                if let Some(close_paren) = stat_str.rfind(')') {
                    let mut it = stat_str[close_paren + 1..].split_whitespace();
                    let _state = it.next();
                    if let Some(ppid_s) = it.next() {
                        if let Ok(ppid) = ppid_s.parse::<u32>() {
                            children_map.entry(ppid).or_default().push(pid);
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root_pid];
    while let Some(cur) = stack.pop() {
        if let Some(kids) = children_map.get(&cur) {
            for &k in kids {
                if k != root_pid && !out.contains(&k) {
                    out.push(k);
                    stack.push(k);
                }
            }
        }
    }
    out
}
