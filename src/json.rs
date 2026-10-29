use crate::model::{
    BenchmarkReport, DiskSpaceReport, FlightTraceFile, FlightTraceSample, ProcessInspectorData,
    SystemSnapshot,
};
use crate::providers::LinuxSystemSampler;
use crate::providers::system_catalog::format_system_info_report;
use anyhow::{Result, anyhow};

pub fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

pub fn serialize_full_state_json(snap: &SystemSnapshot, sampler: &LinuxSystemSampler) -> String {
    let mut s = String::with_capacity(128 * 1024);
    s.push('{');
    s.push_str(&format!(
        "\"seq\":{},\"timestamp_epoch_ms\":{},",
        snap.seq, snap.timestamp_epoch_ms
    ));

    // Pressure
    s.push_str(&format!(
        "\"pressure\":{{\"level\":\"{}\",\"score_percent\":{:.2},\"load_1m\":{:.2},\"load_5m\":{:.2},\"load_15m\":{:.2},\"load_per_core\":{:.3},\"mem_psi_some_avg10\":{:.2},\"mem_psi_full_avg10\":{:.2},\"cpu_psi_some_avg10\":{:.2},\"io_psi_some_avg10\":{:.2}}},",
        snap.pressure.level.as_str(),
        snap.pressure.score_percent,
        snap.pressure.load_1m,
        snap.pressure.load_5m,
        snap.pressure.load_15m,
        snap.pressure.load_per_core,
        snap.pressure.mem_psi_some_avg10,
        snap.pressure.mem_psi_full_avg10,
        snap.pressure.cpu_psi_some_avg10,
        snap.pressure.io_psi_some_avg10,
    ));

    // CPU
    s.push_str(&format!(
        "\"cpu\":{{\"model_name\":\"{}\",\"vendor_id\":\"{}\",\"architecture\":\"{}\",\"physical_cores\":{},\"logical_cores\":{},\"p_cores\":{},\"e_cores\":{},\"total_pct\":{:.2},\"user_pct\":{:.2},\"system_pct\":{:.2},\"iowait_pct\":{:.2},\"irq_pct\":{:.2},\"avg_freq_mhz\":{:.1},\"max_freq_mhz\":{:.1},\"package_temp_c\":{},\"package_power_watts\":{},\"context_switches_per_sec\":{:.1},\"interrupts_per_sec\":{:.1},\"processes_running\":{},\"processes_blocked\":{},\"uptime_seconds\":{:.1},\"cores\":[",
        escape_json(&snap.cpu.model_name),
        escape_json(&snap.cpu.vendor_id),
        escape_json(&snap.cpu.architecture),
        snap.cpu.physical_cores,
        snap.cpu.logical_cores,
        snap.cpu.p_cores,
        snap.cpu.e_cores,
        snap.cpu.total_pct,
        snap.cpu.user_pct,
        snap.cpu.system_pct,
        snap.cpu.iowait_pct,
        snap.cpu.irq_pct,
        snap.cpu.avg_freq_mhz,
        snap.cpu.max_freq_mhz,
        snap.cpu.package_temp_c.map_or(String::from("null"), |v| format!("{:.1}", v)),
        snap.cpu.package_power_watts.map_or(String::from("null"), |v| format!("{:.2}", v)),
        snap.cpu.context_switches_per_sec,
        snap.cpu.interrupts_per_sec,
        snap.cpu.processes_running,
        snap.cpu.processes_blocked,
        snap.cpu.uptime_seconds,
    ));
    for (i, c) in snap.cpu.cores.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"core_id\":{},\"kind\":\"{}\",\"total_pct\":{:.1},\"user_pct\":{:.1},\"system_pct\":{:.1},\"iowait_pct\":{:.1},\"freq_mhz\":{:.0},\"min_freq_mhz\":{:.0},\"max_freq_mhz\":{:.0},\"temp_c\":{}}}",
            c.core_id,
            c.kind.as_str(),
            c.total_pct,
            c.user_pct,
            c.system_pct,
            c.iowait_pct,
            c.freq_mhz,
            c.min_freq_mhz,
            c.max_freq_mhz,
            c.temp_c.map_or(String::from("null"), |v| format!("{:.1}", v))
        ));
    }
    s.push_str("]},");

    // Memory
    let m = &snap.memory;
    s.push_str(&format!(
        "\"memory\":{{\"total_bytes\":{},\"used_bytes\":{},\"available_bytes\":{},\"free_bytes\":{},\"buffers_bytes\":{},\"cached_bytes\":{},\"shared_bytes\":{},\"slab_bytes\":{},\"committed_bytes\":{},\"commit_limit_bytes\":{},\"swap_total_bytes\":{},\"swap_used_bytes\":{},\"swap_free_bytes\":{},\"zswap_enabled\":{},\"used_pct\":{:.2},\"swap_pct\":{:.2},\"page_faults_per_sec\":{:.1},\"major_faults_per_sec\":{:.2}}},",
        m.total_bytes,
        m.used_bytes,
        m.available_bytes,
        m.free_bytes,
        m.buffers_bytes,
        m.cached_bytes,
        m.shared_bytes,
        m.slab_bytes,
        m.committed_bytes,
        m.commit_limit_bytes,
        m.swap_total_bytes,
        m.swap_used_bytes,
        m.swap_free_bytes,
        m.zswap_enabled,
        m.used_pct,
        m.swap_pct,
        m.page_faults_per_sec,
        m.major_faults_per_sec,
    ));

    // Processes (top 250 for UI responsiveness)
    s.push_str("\"processes\":[");
    for (i, p) in snap.processes.iter().take(250).enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"pid\":{},\"ppid\":{},\"name\":\"{}\",\"cmdline\":\"{}\",\"user\":\"{}\",\"uid\":{},\"state\":\"{}\",\"state_desc\":\"{}\",\"cpu_pct\":{:.2},\"mem_rss_bytes\":{},\"mem_vms_bytes\":{},\"mem_pct\":{:.2},\"read_bps\":{:.1},\"write_bps\":{:.1},\"threads\":{},\"nice\":{},\"cpu_time_seconds\":{:.2},\"is_kernel_thread\":{},\"tree_depth\":{}}}",
            p.pid,
            p.ppid,
            escape_json(&p.name),
            escape_json(&p.cmdline),
            escape_json(&p.user),
            p.uid,
            p.state,
            p.state_desc,
            p.cpu_pct,
            p.mem_rss_bytes,
            p.mem_vms_bytes,
            p.mem_pct,
            p.read_bytes_per_sec,
            p.write_bytes_per_sec,
            p.threads,
            p.nice,
            p.cpu_time_seconds,
            p.is_kernel_thread,
            p.tree_depth,
        ));
    }
    s.push_str(&format!("],\"total_process_count\":{},", snap.processes.len()));

    // Disks
    s.push_str("\"disks\":[");
    for (i, d) in snap.disks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let mounts_str = d.mount_points.join(", ");
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"model\":\"{}\",\"kind\":\"{}\",\"capacity_bytes\":{},\"read_bps\":{:.1},\"write_bps\":{:.1},\"read_iops\":{:.1},\"write_iops\":{:.1},\"busy_pct\":{:.1},\"total_read_bytes\":{},\"total_written_bytes\":{},\"mounts\":\"{}\"}}",
            escape_json(&d.name),
            escape_json(&d.model),
            escape_json(&d.kind),
            d.capacity_bytes,
            d.read_bytes_per_sec,
            d.write_bytes_per_sec,
            d.read_iops,
            d.write_iops,
            d.busy_pct,
            d.total_read_bytes,
            d.total_written_bytes,
            escape_json(&mounts_str),
        ));
    }
    s.push_str("],");

    // Networks
    s.push_str("\"networks\":[");
    for (i, n) in snap.networks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"kind\":\"{}\",\"mac\":\"{}\",\"ipv4\":\"{}\",\"ipv6\":\"{}\",\"oper_state\":\"{}\",\"speed_mbps\":{},\"rx_bps\":{:.1},\"tx_bps\":{:.1},\"rx_pps\":{:.1},\"tx_pps\":{:.1},\"total_rx_bytes\":{},\"total_tx_bytes\":{}}}",
            escape_json(&n.name),
            escape_json(&n.kind),
            escape_json(&n.mac_address),
            escape_json(&n.ipv4),
            escape_json(&n.ipv6),
            escape_json(&n.oper_state),
            n.speed_mbps.map_or(String::from("null"), |v| v.to_string()),
            n.rx_bytes_per_sec,
            n.tx_bytes_per_sec,
            n.rx_packets_per_sec,
            n.tx_packets_per_sec,
            n.total_rx_bytes,
            n.total_tx_bytes,
        ));
    }
    s.push_str("],");

    // GPUs
    s.push_str("\"gpus\":[");
    for (i, g) in snap.gpus.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"card_id\":\"{}\",\"name\":\"{}\",\"driver\":\"{}\",\"busy_pct\":{:.1},\"vram_used_bytes\":{},\"vram_total_bytes\":{},\"clock_mhz\":{},\"temp_c\":{},\"power_watts\":{}}}",
            escape_json(&g.card_id),
            escape_json(&g.name),
            escape_json(&g.driver),
            g.busy_pct,
            g.vram_used_bytes.map_or(String::from("null"), |v| v.to_string()),
            g.vram_total_bytes.map_or(String::from("null"), |v| v.to_string()),
            g.clock_mhz.map_or(String::from("null"), |v| format!("{:.0}", v)),
            g.temp_c.map_or(String::from("null"), |v| format!("{:.1}", v)),
            g.power_watts.map_or(String::from("null"), |v| format!("{:.1}", v)),
        ));
    }
    s.push_str("],");

    // Network Connections
    s.push_str("\"connections\":[");
    for (i, c) in snap.connections.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"protocol\":\"{}\",\"local_addr\":\"{}\",\"local_port\":{},\"remote_addr\":\"{}\",\"remote_port\":{},\"state\":\"{}\",\"inode\":{},\"uid\":{},\"pid\":{},\"process_name\":\"{}\",\"is_public_listener\":{}}}",
            escape_json(&c.protocol),
            escape_json(&c.local_addr),
            c.local_port,
            escape_json(&c.remote_addr),
            c.remote_port,
            escape_json(&c.state),
            c.inode,
            c.uid,
            c.pid.map_or(String::from("null"), |v| v.to_string()),
            escape_json(&c.process_name),
            c.is_public_listener,
        ));
    }
    s.push_str("],");

    // Hardware Sensors
    s.push_str("\"sensors\":[");
    for (i, grp) in snap.sensors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"chip_name\":\"{}\",\"adapter\":\"{}\",\"readings\":[",
            escape_json(&grp.chip_name),
            escape_json(&grp.adapter)
        ));
        for (j, r) in grp.readings.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{\"id\":\"{}\",\"label\":\"{}\",\"category\":\"{}\",\"unit\":\"{}\",\"current\":{:.2},\"min\":{:.2},\"max\":{:.2}}}",
                escape_json(&r.id),
                escape_json(&r.label),
                escape_json(&r.category),
                escape_json(&r.unit),
                r.current,
                r.min,
                r.max,
            ));
        }
        s.push_str("]}");
    }
    s.push_str("],");

    // Disk Space
    s.push_str("\"disk_space\":");
    s.push_str(&serialize_disk_space_json(&sampler.disk_space));
    s.push(',');

    // Drivers & Kernel Modules
    s.push_str("\"drivers\":[");
    for (i, drv) in sampler.drivers.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let used = drv.used_by.join(", ");
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"size_bytes\":{},\"ref_count\":{},\"used_by\":\"{}\",\"state\":\"{}\",\"srcversion\":\"{}\",\"in_tree\":{},\"module_path\":\"{}\"}}",
            escape_json(&drv.name),
            drv.size_bytes,
            drv.ref_count,
            escape_json(&used),
            escape_json(&drv.state),
            escape_json(&drv.srcversion),
            drv.in_tree,
            escape_json(&drv.module_path),
        ));
    }
    s.push_str("],");

    // Flight Recorder
    s.push_str(&format!(
        "\"flight_recorder\":{{\"recording\":{},\"trace\":{}}},",
        sampler.flight_recorder.recording,
        serialize_flight_trace_json(&sampler.flight_recorder.trace)
    ));

    // Hardware Benchmarks
    s.push_str("\"benchmarks\":");
    if let Some(ref b) = sampler.benchmarks {
        s.push_str(&serialize_benchmark_json(b));
    } else {
        s.push_str("null");
    }
    s.push(',');

    // Installed Apps
    s.push_str("\"installed_apps\":[");
    for (i, app) in sampler.installed_apps.iter().take(250).enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"version\":\"{}\",\"source\":\"{}\",\"section\":\"{}\",\"installed_size_kb\":{},\"description\":\"{}\"}}",
            escape_json(&app.name),
            escape_json(&app.version),
            escape_json(&app.source),
            escape_json(&app.section),
            app.installed_size_kb,
            escape_json(&app.description),
        ));
    }
    s.push_str("],");

    // App History
    s.push_str("\"app_history\":[");
    for (i, h) in snap.app_history.iter().take(100).enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"app_name\":\"{}\",\"cumulative_cpu_seconds\":{:.2},\"peak_rss_bytes\":{},\"total_read_bytes\":{},\"total_write_bytes\":{},\"instance_count\":{}}}",
            escape_json(&h.app_name),
            h.cumulative_cpu_seconds,
            h.peak_rss_bytes,
            h.total_read_bytes,
            h.total_write_bytes,
            h.instance_count,
        ));
    }
    s.push_str("],");

    // Services
    s.push_str("\"services\":[");
    for (i, sv) in sampler.services.iter().take(200).enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"unit\":\"{}\",\"load_state\":\"{}\",\"active_state\":\"{}\",\"sub_state\":\"{}\",\"description\":\"{}\"}}",
            escape_json(&sv.unit),
            escape_json(&sv.load_state),
            escape_json(&sv.active_state),
            escape_json(&sv.sub_state),
            escape_json(&sv.description),
        ));
    }
    s.push_str("],");

    // Startup Apps
    s.push_str("\"startup_apps\":[");
    for (i, st) in sampler.startup_apps.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"exec\":\"{}\",\"scope\":\"{}\",\"enabled\":{},\"comment\":\"{}\",\"path\":\"{}\"}}",
            escape_json(&st.name),
            escape_json(&st.exec),
            escape_json(&st.scope),
            st.enabled,
            escape_json(&st.comment),
            escape_json(&st.path),
        ));
    }
    s.push_str("],");

    // Users
    s.push_str("\"users\":[");
    for (i, u) in sampler.users.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"username\":\"{}\",\"uid\":{},\"tty\":\"{}\",\"remote_host\":\"{}\",\"login_time\":\"{}\",\"process_count\":{},\"cpu_pct\":{:.2},\"rss_bytes\":{}}}",
            escape_json(&u.username),
            u.uid,
            escape_json(&u.tty),
            escape_json(&u.remote_host),
            escape_json(&u.login_time),
            u.process_count,
            u.cpu_pct,
            u.rss_bytes,
        ));
    }
    s.push_str("],");

    // System Info
    s.push_str("\"system_info\":");
    if let Some(ref info) = sampler.system_info {
        let report_txt = format_system_info_report(info);
        s.push_str(&format!(
            "{{\"hostname\":\"{}\",\"os_pretty_name\":\"{}\",\"kernel_release\":\"{}\",\"architecture\":\"{}\",\"board_vendor\":\"{}\",\"board_name\":\"{}\",\"bios_version\":\"{}\",\"desktop_env\":\"{}\",\"session_type\":\"{}\",\"report_text\":\"{}\",\"sections\":[",
            escape_json(&info.hostname),
            escape_json(&info.os_pretty_name),
            escape_json(&info.kernel_release),
            escape_json(&info.architecture),
            escape_json(&info.board_vendor),
            escape_json(&info.board_name),
            escape_json(&info.bios_version),
            escape_json(&info.desktop_env),
            escape_json(&info.session_type),
            escape_json(&report_txt),
        ));
        for (idx, (sec_title, rows)) in info.sections.iter().enumerate() {
            if idx > 0 {
                s.push(',');
            }
            s.push_str(&format!("{{\"title\":\"{}\",\"rows\":[", escape_json(sec_title)));
            for (r_idx, (k, v)) in rows.iter().enumerate() {
                if r_idx > 0 {
                    s.push(',');
                }
                s.push_str(&format!(
                    "{{\"key\":\"{}\",\"value\":\"{}\"}}",
                    escape_json(k),
                    escape_json(v)
                ));
            }
            s.push_str("]}");
        }
        s.push_str("]}");
    } else {
        s.push_str("null");
    }

    s.push('}');
    s
}

pub fn serialize_disk_space_json(ds: &DiskSpaceReport) -> String {
    let mut s = String::with_capacity(8192);
    s.push_str(&format!(
        "{{\"root_path\":\"{}\",\"scanned_at_epoch\":{},\"scan_duration_ms\":{},\"fs_total_bytes\":{},\"fs_used_bytes\":{},\"fs_free_bytes\":{},\"scanned_bytes\":{},\"scanned_files\":{},\"scanned_dirs\":{},\"top_directories\":[",
        escape_json(&ds.root_path),
        ds.scanned_at_epoch,
        ds.scan_duration_ms,
        ds.fs_total_bytes,
        ds.fs_used_bytes,
        ds.fs_free_bytes,
        ds.scanned_bytes,
        ds.scanned_files,
        ds.scanned_dirs,
    ));
    for (i, d) in ds.top_directories.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"path\":\"{}\",\"name\":\"{}\",\"size_bytes\":{},\"file_count\":{},\"pct_of_root\":{:.2}}}",
            escape_json(&d.path),
            escape_json(&d.name),
            d.size_bytes,
            d.file_count,
            d.pct_of_root,
        ));
    }
    s.push_str("],\"categories\":[");
    for (i, c) in ds.categories.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"category\":\"{}\",\"size_bytes\":{},\"file_count\":{},\"pct_of_root\":{:.2}}}",
            escape_json(&c.category),
            c.size_bytes,
            c.file_count,
            c.pct_of_root,
        ));
    }
    s.push_str("],\"largest_files\":[");
    for (i, f) in ds.largest_files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"path\":\"{}\",\"size_bytes\":{},\"category\":\"{}\"}}",
            escape_json(&f.path),
            f.size_bytes,
            escape_json(&f.category),
        ));
    }
    s.push_str("]}");
    s
}

pub fn serialize_benchmark_json(b: &BenchmarkReport) -> String {
    let sub = |sc: &crate::model::BenchmarkSubScore| -> String {
        format!(
            "{{\"name\":\"{}\",\"metric_value\":{:.2},\"metric_unit\":\"{}\",\"normalized_score\":{},\"duration_ms\":{}}}",
            escape_json(&sc.name),
            sc.metric_value,
            escape_json(&sc.metric_unit),
            sc.normalized_score,
            sc.duration_ms
        )
    };
    format!(
        "{{\"timestamp_epoch\":{},\"composite_score\":{},\"cpu_single_core\":{},\"cpu_multi_core\":{},\"memory_bandwidth\":{},\"disk_throughput\":{}}}",
        b.timestamp_epoch,
        b.composite_score,
        sub(&b.cpu_single_core),
        sub(&b.cpu_multi_core),
        sub(&b.memory_bandwidth),
        sub(&b.disk_throughput),
    )
}

pub fn serialize_process_inspector_json(info: &ProcessInspectorData) -> String {
    let mut s = String::with_capacity(8192);
    s.push_str(&format!(
        "{{\"pid\":{},\"ppid\":{},\"name\":\"{}\",\"exe_path\":\"{}\",\"cwd\":\"{}\",\"cmdline\":\"{}\",\"user\":\"{}\",\"uid\":{},\"state\":\"{}\",\"threads\":{},\"nice\":{},\"cpu_affinity_hex\":\"{}\",\"voluntary_ctxt_switches\":{},\"nonvoluntary_ctxt_switches\":{},\"rss_bytes\":{},\"vms_bytes\":{},\"shared_bytes\":{},\"read_bytes\":{},\"write_bytes\":{},\"memory_maps_count\":{},\"environ\":[",
        info.pid,
        info.ppid,
        escape_json(&info.name),
        escape_json(&info.exe_path),
        escape_json(&info.cwd),
        escape_json(&info.cmdline),
        escape_json(&info.user),
        info.uid,
        escape_json(&info.state),
        info.threads,
        info.nice,
        escape_json(&info.cpu_affinity_hex),
        info.voluntary_ctxt_switches,
        info.nonvoluntary_ctxt_switches,
        info.rss_bytes,
        info.vms_bytes,
        info.shared_bytes,
        info.read_bytes,
        info.write_bytes,
        info.memory_maps_count,
    ));
    for (i, (k, v)) in info.environ.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"key\":\"{}\",\"value\":\"{}\"}}",
            escape_json(k),
            escape_json(v)
        ));
    }
    s.push_str("],\"open_fds\":[");
    for (i, fd) in info.open_fds.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"fd\":{},\"target\":\"{}\"}}",
            fd.fd,
            escape_json(&fd.target)
        ));
    }
    s.push_str("],\"top_mapped_files\":[");
    for (i, m) in info.top_mapped_files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{}\"", escape_json(m)));
    }
    s.push_str("]}");
    s
}

pub fn serialize_flight_trace_json(trace: &FlightTraceFile) -> String {
    let mut s = String::with_capacity(32 * 1024);
    s.push_str(&format!(
        "{{\"schema\":\"{}\",\"version\":{},\"hostname\":\"{}\",\"cpu_model\":\"{}\",\"recorded_at_epoch\":{},\"samples\":[",
        escape_json(&trace.schema),
        trace.version,
        escape_json(&trace.hostname),
        escape_json(&trace.cpu_model),
        trace.recorded_at_epoch,
    ));
    for (i, smp) in trace.samples.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"index\":{},\"timestamp_epoch_ms\":{},\"cpu_total_pct\":{:.2},\"cpu_avg_freq_mhz\":{:.1},\"cpu_temp_c\":{:.1},\"mem_used_pct\":{:.2},\"mem_used_bytes\":{},\"swap_used_bytes\":{},\"pressure_level\":\"{}\",\"pressure_score\":{:.2},\"disk_read_bps\":{:.1},\"disk_write_bps\":{:.1},\"net_rx_bps\":{:.1},\"net_tx_bps\":{:.1},\"top_process_name\":\"{}\",\"top_process_pid\":{},\"top_process_cpu_pct\":{:.2}}}",
            smp.index,
            smp.timestamp_epoch_ms,
            smp.cpu_total_pct,
            smp.cpu_avg_freq_mhz,
            smp.cpu_temp_c,
            smp.mem_used_pct,
            smp.mem_used_bytes,
            smp.swap_used_bytes,
            escape_json(&smp.pressure_level),
            smp.pressure_score,
            smp.disk_read_bps,
            smp.disk_write_bps,
            smp.net_rx_bps,
            smp.net_tx_bps,
            escape_json(&smp.top_process_name),
            smp.top_process_pid,
            smp.top_process_cpu_pct,
        ));
    }
    s.push_str("]}");
    s
}

/// Parses a `.pulsetrace` JSON file back into a `FlightTraceFile`.
pub fn parse_flight_trace_json(input: &str) -> Result<FlightTraceFile> {
    if !input.contains("io.pulsedeck.traces") {
        return Err(anyhow!(
            "Invalid trace schema: expected 'io.pulsedeck.traces'"
        ));
    }
    let hostname = extract_json_str(input, "hostname").unwrap_or_else(|| String::from("host"));
    let cpu_model = extract_json_str(input, "cpu_model").unwrap_or_else(|| String::from("CPU"));
    let recorded_at_epoch = extract_json_u64(input, "recorded_at_epoch").unwrap_or(0);

    let mut samples = Vec::new();
    if let Some(arr_start) = input.find("\"samples\":[") {
        let rest = &input[arr_start + "\"samples\":[".len()..];
        for obj_chunk in rest.split("},{") {
            let chunk = obj_chunk.trim_matches(|c| c == '[' || c == ']' || c == '{' || c == '}');
            if chunk.is_empty() || !chunk.contains("\"timestamp_epoch_ms\"") {
                continue;
            }
            let index = extract_json_u64(chunk, "index").unwrap_or(samples.len() as u64) as usize;
            let timestamp_epoch_ms = extract_json_u64(chunk, "timestamp_epoch_ms").unwrap_or(0);
            let cpu_total_pct = extract_json_f64(chunk, "cpu_total_pct").unwrap_or(0.0);
            let cpu_avg_freq_mhz = extract_json_f64(chunk, "cpu_avg_freq_mhz").unwrap_or(0.0);
            let cpu_temp_c = extract_json_f64(chunk, "cpu_temp_c").unwrap_or(0.0);
            let mem_used_pct = extract_json_f64(chunk, "mem_used_pct").unwrap_or(0.0);
            let mem_used_bytes = extract_json_u64(chunk, "mem_used_bytes").unwrap_or(0);
            let swap_used_bytes = extract_json_u64(chunk, "swap_used_bytes").unwrap_or(0);
            let pressure_level =
                extract_json_str(chunk, "pressure_level").unwrap_or_else(|| String::from("LOW"));
            let pressure_score = extract_json_f64(chunk, "pressure_score").unwrap_or(0.0);
            let disk_read_bps = extract_json_f64(chunk, "disk_read_bps").unwrap_or(0.0);
            let disk_write_bps = extract_json_f64(chunk, "disk_write_bps").unwrap_or(0.0);
            let net_rx_bps = extract_json_f64(chunk, "net_rx_bps").unwrap_or(0.0);
            let net_tx_bps = extract_json_f64(chunk, "net_tx_bps").unwrap_or(0.0);
            let top_process_name =
                extract_json_str(chunk, "top_process_name").unwrap_or_default();
            let top_process_pid = extract_json_u64(chunk, "top_process_pid").unwrap_or(0) as u32;
            let top_process_cpu_pct =
                extract_json_f64(chunk, "top_process_cpu_pct").unwrap_or(0.0);

            samples.push(FlightTraceSample {
                index,
                timestamp_epoch_ms,
                cpu_total_pct,
                cpu_avg_freq_mhz,
                cpu_temp_c,
                mem_used_pct,
                mem_used_bytes,
                swap_used_bytes,
                pressure_level,
                pressure_score,
                disk_read_bps,
                disk_write_bps,
                net_rx_bps,
                net_tx_bps,
                top_process_name,
                top_process_pid,
                top_process_cpu_pct,
            });
        }
    }

    Ok(FlightTraceFile {
        schema: String::from("io.pulsedeck.traces"),
        version: 1,
        hostname,
        cpu_model,
        recorded_at_epoch,
        samples,
    })
}

pub fn extract_json_str(src: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", key);
    let start = src.find(&needle)? + needle.len();
    let rest = &src[start..];
    let mut out = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                c => out.push(c),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

pub fn extract_json_u64(src: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = src.find(&needle)? + needle.len();
    let rest = src[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse::<u64>().ok()
}

pub fn extract_json_f64(src: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{}\":", key);
    let start = src.find(&needle)? + needle.len();
    let rest = src[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}
