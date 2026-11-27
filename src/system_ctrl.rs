use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::config::Config;
use crate::mem_cache::{
    perform_cache_cleanup, prewarm_binaries, read_memory_telemetry, LaunchCache, MemoryTelemetry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub volume_pct: u8,
    pub muted: bool,
    pub is_default: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    pub index: u32,
    pub app_name: String,
    pub media_name: String,
    pub device_index: u32,
    pub volume_pct: u8,
    pub muted: bool,
    pub is_comms: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgProcess {
    pub pid: u32,
    pub pids: Vec<u32>,
    pub name: String,
    pub rss_mb: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSnapshot {
    pub version: u64,
    pub sinks: Vec<AudioDevice>,
    pub sources: Vec<AudioDevice>,
    pub sink_inputs: Vec<AudioStream>,
    pub source_outputs: Vec<AudioStream>,
    pub default_sink: String,
    pub default_source: String,
    pub comms_sink: Option<String>,
    pub media_sink: Option<String>,
    pub auto_route_roles: bool,
    pub show_system_processes: bool,
    pub brightness_pct: u8,
    pub brightness_supported: bool,
    pub network_label: String,
    pub wifi_enabled: bool,
    pub bluetooth_on: bool,
    pub power_profile: String,
    pub bg_apps: Vec<BgProcess>,
    pub memory: MemoryTelemetry,
}

impl Default for SystemSnapshot {
    fn default() -> Self {
        Self {
            version: 0,
            sinks: Vec::new(),
            sources: Vec::new(),
            sink_inputs: Vec::new(),
            source_outputs: Vec::new(),
            default_sink: String::new(),
            default_source: String::new(),
            comms_sink: None,
            media_sink: None,
            auto_route_roles: true,
            show_system_processes: false,
            brightness_pct: 80,
            brightness_supported: false,
            network_label: "Online".to_string(),
            wifi_enabled: true,
            bluetooth_on: false,
            power_profile: "balanced".to_string(),
            bg_apps: Vec::new(),
            memory: MemoryTelemetry::default(),
        }
    }
}

impl SystemSnapshot {
    pub fn default_output_vol(&self) -> (u8, bool) {
        self.sinks
            .iter()
            .find(|s| s.is_default && !s.disabled)
            .or_else(|| self.sinks.iter().find(|s| !s.disabled))
            .or_else(|| self.sinks.first())
            .map(|s| (s.volume_pct, s.muted))
            .unwrap_or((100, false))
    }

    pub fn default_input_vol(&self) -> (u8, bool) {
        self.sources
            .iter()
            .find(|s| s.is_default && !s.disabled)
            .or_else(|| self.sources.iter().find(|s| !s.disabled))
            .or_else(|| self.sources.first())
            .map(|s| (s.volume_pct, s.muted))
            .unwrap_or((100, false))
    }
}

#[derive(Debug, Clone)]
pub enum SysCommand {
    RefreshNow,
    SetSinkVolume { name: String, pct: u8 },
    ToggleSinkMute { name: String },
    SetSinkMute { name: String, muted: bool },
    SetDefaultSink { name: String },
    SetSourceVolume { name: String, pct: u8 },
    ToggleSourceMute { name: String },
    SetSourceMute { name: String, muted: bool },
    SetDefaultSource { name: String },
    SetAudioDeviceDisabled { is_input: bool, name: String, disabled: bool },
    SetDisabledAudioLists { sinks: Vec<String>, sources: Vec<String> },
    SetStreamVolume { is_input: bool, index: u32, pct: u8 },
    ToggleStreamMute { is_input: bool, index: u32 },
    MoveStream { is_input: bool, index: u32, target: String },
    RouteRole { is_comms: bool, target_sink: Option<String> },
    SetAutoRouteRoles { enabled: bool },
    SetShowSystemProcesses { enabled: bool },
    SetBrightness { pct: u8 },
    SetPowerProfile { profile: String },
    ToggleWifi,
    ToggleBluetooth,
    KillProcess { pids: Vec<u32>, force: bool },
    CleanRamCache { deep_kernel: bool },
    ConfigureMemoryPolicy {
        auto_clean: bool,
        threshold_pct: u8,
        prewarm: bool,
        prewarm_bins: Vec<String>,
    },
}

/// Asynchronous background service managing PipeWire (`pactl`), `ddcutil`/backlight,
/// NetworkManager (`nmcli`), BlueZ (`bluetoothctl`), `powerprofilesctl`, and `/proc` background apps.
pub struct SystemController {
    snapshot: Arc<Mutex<SystemSnapshot>>,
    version_atomic: Arc<AtomicU64>,
    tx: mpsc::Sender<SysCommand>,
}

impl SystemController {
    pub fn start(
        comms_sink: Option<String>,
        media_sink: Option<String>,
        auto_route_roles: bool,
    ) -> Self {
        let mut init = SystemSnapshot::default();
        init.comms_sink = comms_sink;
        init.media_sink = media_sink;
        init.auto_route_roles = auto_route_roles;

        let snapshot = Arc::new(Mutex::new(init));
        let version_atomic = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel::<SysCommand>();
        let snap_bg = Arc::clone(&snapshot);
        let ver_bg = Arc::clone(&version_atomic);

        thread::Builder::new()
            .name("velox-sysctrl".to_string())
            .spawn(move || worker_loop(snap_bg, ver_bg, rx))
            .ok();

        Self {
            snapshot,
            version_atomic,
            tx,
        }
    }

    /// Restores persisted audio levels/mutes, monitor brightness, and power profile from config.
    pub fn restore_saved_hardware(&self, cfg: &Config) {
        if let Some(pct) = cfg.saved_volume_pct {
            self.send(SysCommand::SetSinkVolume {
                name: "@DEFAULT_SINK@".to_string(),
                pct: pct.min(150),
            });
        }
        if let Some(muted) = cfg.saved_muted {
            self.send(SysCommand::SetSinkMute {
                name: "@DEFAULT_SINK@".to_string(),
                muted,
            });
        }
        if let Some(mic_pct) = cfg.saved_mic_volume_pct {
            self.send(SysCommand::SetSourceVolume {
                name: "@DEFAULT_SOURCE@".to_string(),
                pct: mic_pct.min(150),
            });
        }
        if let Some(mic_muted) = cfg.saved_mic_muted {
            self.send(SysCommand::SetSourceMute {
                name: "@DEFAULT_SOURCE@".to_string(),
                muted: mic_muted,
            });
        }
        if let Some(bri) = cfg.saved_brightness_pct {
            self.send(SysCommand::SetBrightness {
                pct: bri.clamp(5, 100),
            });
        }
        if let Some(ref prof) = cfg.saved_power_profile {
            self.send(SysCommand::SetPowerProfile {
                profile: prof.clone(),
            });
        }
        self.send(SysCommand::SetDisabledAudioLists {
            sinks: cfg.disabled_audio_sinks.clone(),
            sources: cfg.disabled_audio_sources.clone(),
        });
        for name in &cfg.disabled_audio_sinks {
            self.send(SysCommand::SetAudioDeviceDisabled {
                is_input: false,
                name: name.clone(),
                disabled: true,
            });
        }
        for name in &cfg.disabled_audio_sources {
            self.send(SysCommand::SetAudioDeviceDisabled {
                is_input: true,
                name: name.clone(),
                disabled: true,
            });
        }
        self.send(SysCommand::ConfigureMemoryPolicy {
            auto_clean: cfg.auto_clean_ram_cache,
            threshold_pct: cfg.ram_cache_threshold_pct,
            prewarm: cfg.app_prewarm_cache,
            prewarm_bins: vec![
                cfg.terminal_cmd.clone(),
                cfg.browser_cmd.clone(),
                cfg.files_cmd.clone(),
                cfg.settings_cmd.clone(),
            ],
        });
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    /// Lock-free fast-path check so compositor frames never block or stall on mutex contention.
    pub fn snapshot_if_changed(&self, cur_version: u64) -> Option<SystemSnapshot> {
        if self.version_atomic.load(Ordering::Acquire) == cur_version {
            return None;
        }
        let guard = self.snapshot.try_lock().ok()?;
        if guard.version == cur_version {
            None
        } else {
            Some(guard.clone())
        }
    }

    pub fn send(&self, cmd: SysCommand) {
        {
            let mut snap = self.snapshot.lock().unwrap();
            match &cmd {
                SysCommand::SetSinkVolume { name, pct } => {
                    for s in &mut snap.sinks {
                        if &s.name == name || (name == "@DEFAULT_SINK@" && s.is_default) {
                            s.volume_pct = *pct;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::ToggleSinkMute { name } => {
                    for s in &mut snap.sinks {
                        if &s.name == name || (name == "@DEFAULT_SINK@" && s.is_default) {
                            s.muted = !s.muted;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::SetSinkMute { name, muted } => {
                    for s in &mut snap.sinks {
                        if &s.name == name || (name == "@DEFAULT_SINK@" && s.is_default) {
                            s.muted = *muted;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::SetDefaultSink { name } => {
                    for s in &mut snap.sinks {
                        s.is_default = &s.name == name;
                    }
                    snap.default_sink = name.clone();
                    snap.version += 1;
                }
                SysCommand::SetSourceVolume { name, pct } => {
                    for s in &mut snap.sources {
                        if &s.name == name || (name == "@DEFAULT_SOURCE@" && s.is_default) {
                            s.volume_pct = *pct;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::ToggleSourceMute { name } => {
                    for s in &mut snap.sources {
                        if &s.name == name || (name == "@DEFAULT_SOURCE@" && s.is_default) {
                            s.muted = !s.muted;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::SetSourceMute { name, muted } => {
                    for s in &mut snap.sources {
                        if &s.name == name || (name == "@DEFAULT_SOURCE@" && s.is_default) {
                            s.muted = *muted;
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::SetDefaultSource { name } => {
                    for s in &mut snap.sources {
                        s.is_default = &s.name == name;
                    }
                    snap.default_source = name.clone();
                    snap.version += 1;
                }
                SysCommand::SetAudioDeviceDisabled {
                    is_input,
                    name,
                    disabled,
                } => {
                    let list = if *is_input {
                        &mut snap.sources
                    } else {
                        &mut snap.sinks
                    };
                    for d in list.iter_mut() {
                        if &d.name == name {
                            d.disabled = *disabled;
                            if *disabled {
                                d.muted = true;
                                d.is_default = false;
                            } else {
                                d.muted = false;
                            }
                        }
                    }
                    if *disabled {
                        if !is_input && snap.default_sink == *name {
                            if let Some(next_name) = snap.sinks.iter().find(|s| !s.disabled).map(|s| s.name.clone()) {
                                snap.default_sink = next_name.clone();
                                for s in &mut snap.sinks {
                                    s.is_default = s.name == next_name;
                                }
                            }
                        } else if *is_input && snap.default_source == *name {
                            if let Some(next_name) = snap.sources.iter().find(|s| !s.disabled).map(|s| s.name.clone()) {
                                snap.default_source = next_name.clone();
                                for s in &mut snap.sources {
                                    s.is_default = s.name == next_name;
                                }
                            }
                        }
                    }
                    snap.version += 1;
                }
                SysCommand::SetDisabledAudioLists { sinks, sources } => {
                    for d in &mut snap.sinks {
                        d.disabled = sinks.contains(&d.name);
                    }
                    for d in &mut snap.sources {
                        d.disabled = sources.contains(&d.name);
                    }
                    snap.version += 1;
                }
                SysCommand::SetStreamVolume {
                    is_input,
                    index,
                    pct,
                } => {
                    let list = if *is_input {
                        &mut snap.source_outputs
                    } else {
                        &mut snap.sink_inputs
                    };
                    if let Some(st) = list.iter_mut().find(|s| s.index == *index) {
                        st.volume_pct = *pct;
                    }
                    snap.version += 1;
                }
                SysCommand::ToggleStreamMute { is_input, index } => {
                    let list = if *is_input {
                        &mut snap.source_outputs
                    } else {
                        &mut snap.sink_inputs
                    };
                    if let Some(st) = list.iter_mut().find(|s| s.index == *index) {
                        st.muted = !st.muted;
                    }
                    snap.version += 1;
                }
                SysCommand::SetBrightness { pct } => {
                    snap.brightness_pct = (*pct).clamp(5, 100);
                    snap.version += 1;
                }
                SysCommand::RouteRole {
                    is_comms,
                    target_sink,
                } => {
                    if *is_comms {
                        snap.comms_sink = target_sink.clone();
                    } else {
                        snap.media_sink = target_sink.clone();
                    }
                    snap.version += 1;
                }
                SysCommand::SetAutoRouteRoles { enabled } => {
                    snap.auto_route_roles = *enabled;
                    snap.version += 1;
                }
                SysCommand::SetShowSystemProcesses { enabled } => {
                    snap.show_system_processes = *enabled;
                    snap.version += 1;
                }
                SysCommand::KillProcess { pids, .. } => {
                    let tree = collect_process_tree(pids, true);
                    snap.bg_apps
                        .retain(|p| !p.pids.iter().any(|id| pids.contains(id) || tree.contains(id)));
                    snap.version += 1;
                }
                _ => {}
            }
            self.version_atomic.store(snap.version, Ordering::Release);
        }
        let _ = self.tx.send(cmd);
    }
}

fn worker_loop(
    snapshot: Arc<Mutex<SystemSnapshot>>,
    version_atomic: Arc<AtomicU64>,
    rx: mpsc::Receiver<SysCommand>,
) {
    let mut last_audio_poll = Instant::now() - Duration::from_secs(60);
    let mut last_brightness_poll = Instant::now() - Duration::from_secs(60);
    let mut last_sys_poll = Instant::now() - Duration::from_secs(60);
    let mut last_proc_poll = Instant::now() - Duration::from_secs(60);
    let mut last_mem_poll = Instant::now() - Duration::from_secs(60);
    let mut last_auto_clean = Instant::now() - Duration::from_secs(60);
    let mut last_prewarm = Instant::now() - Duration::from_secs(60);
    let mut cached_audio = SystemSnapshot::default();
    let mut cached_brightness: (u8, bool) = (80, false);
    let mut cached_net = ("Online".to_string(), true);
    let mut cached_bt = false;
    let mut cached_power = "balanced".to_string();
    let mut cached_procs: Vec<BgProcess> = Vec::new();
    let mut cached_mem = read_memory_telemetry();
    let mut auto_clean_mem = true;
    let mut ram_threshold_pct = 78u8;
    let mut prewarm_enabled = true;
    let mut prewarm_defaults: Vec<String> = Vec::new();
    let mut disabled_sinks: Vec<String> = Vec::new();
    let mut disabled_sources: Vec<String> = Vec::new();
    let mut cleanups_done = 0u32;
    let mut last_reclaimed_mb = 0u32;
    let mut prewarmed_count = 0u16;
    let mut seen_stream_ids: Vec<u32> = Vec::new();
    let mut pending_sigkill: Vec<(u32, Instant)> = Vec::new();
    let mut recently_killed: Vec<(u32, Instant)> = Vec::new();
    let mut queued_first: Option<SysCommand> = None;

    loop {
        let mut pending_cmds = Vec::new();
        if let Some(first) = queued_first.take() {
            pending_cmds.push(first);
        }
        while let Ok(cmd) = rx.try_recv() {
            pending_cmds.push(cmd);
        }

        let mut latest_brightness: Option<u8> = None;
        let mut did_audio_cmd = false;
        let mut did_sys_cmd = false;
        let mut did_proc_cmd = false;
        let mut did_mem_cmd = false;

        for cmd in pending_cmds {
            match cmd {
                SysCommand::RefreshNow => {
                    did_audio_cmd = true;
                    did_sys_cmd = true;
                    did_proc_cmd = true;
                    did_mem_cmd = true;
                }
                SysCommand::SetBrightness { pct } => {
                    latest_brightness = Some(pct.clamp(5, 100));
                }
                SysCommand::SetShowSystemProcesses { .. } => {
                    did_proc_cmd = true;
                }
                SysCommand::KillProcess { pids, force } => {
                    kill_pids(&pids, force, &mut pending_sigkill, &mut recently_killed);
                    did_proc_cmd = true;
                }
                SysCommand::CleanRamCache { deep_kernel } => {
                    let (reclaimed, _dropped) = perform_cache_cleanup(deep_kernel);
                    cleanups_done = cleanups_done.saturating_add(1);
                    last_reclaimed_mb = reclaimed;
                    last_auto_clean = Instant::now();
                    did_mem_cmd = true;
                }
                SysCommand::ConfigureMemoryPolicy {
                    auto_clean,
                    threshold_pct,
                    prewarm,
                    prewarm_bins,
                } => {
                    auto_clean_mem = auto_clean;
                    ram_threshold_pct = threshold_pct.clamp(50, 95);
                    prewarm_enabled = prewarm;
                    prewarm_defaults = prewarm_bins;
                    did_mem_cmd = true;
                }
                SysCommand::SetDisabledAudioLists { sinks, sources } => {
                    disabled_sinks = sinks;
                    disabled_sources = sources;
                    did_audio_cmd = true;
                }
                SysCommand::SetAudioDeviceDisabled {
                    is_input,
                    name,
                    disabled,
                } => {
                    let target_list = if is_input {
                        &mut disabled_sources
                    } else {
                        &mut disabled_sinks
                    };
                    if disabled {
                        if !target_list.contains(&name) {
                            target_list.push(name.clone());
                        }
                    } else {
                        target_list.retain(|n| n != &name);
                    }
                    did_audio_cmd = true;
                    run_sys_command(
                        &snapshot,
                        SysCommand::SetAudioDeviceDisabled {
                            is_input,
                            name,
                            disabled,
                        },
                    );
                }
                other => {
                    if matches!(
                        other,
                        SysCommand::SetPowerProfile { .. }
                            | SysCommand::ToggleWifi
                            | SysCommand::ToggleBluetooth
                    ) {
                        did_sys_cmd = true;
                    } else {
                        did_audio_cmd = true;
                    }
                    run_sys_command(&snapshot, other);
                }
            }
        }

        if let Some(pct) = latest_brightness {
            apply_brightness(pct);
            cached_brightness = (pct, true);
        }

        let now = Instant::now();
        // Auto-escalate stubborn processes and any newly spawned children to SIGKILL on 1 click.
        if !pending_sigkill.is_empty() {
            let mut still_pending = Vec::new();
            let mut due_roots = Vec::new();
            for (pid, deadline) in pending_sigkill.drain(..) {
                if now >= deadline {
                    due_roots.push(pid);
                } else {
                    still_pending.push((pid, deadline));
                }
            }
            if !due_roots.is_empty() {
                let mut tree = collect_process_tree(&due_roots, true);
                tree.extend(due_roots);
                tree.sort_unstable();
                tree.dedup();
                for pid in tree {
                    if is_live_non_zombie(pid) {
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        }
                        if !recently_killed.iter().any(|(p, _)| *p == pid) {
                            recently_killed.push((pid, now + Duration::from_millis(3000)));
                        }
                        did_proc_cmd = true;
                    }
                }
            }
            pending_sigkill = still_pending;
        }
        recently_killed.retain(|(_, exp)| *exp > now);

        let (comms_sink, media_sink, auto_route_roles, show_sys_procs) = {
            let g = snapshot.lock().unwrap();
            (
                g.comms_sink.clone(),
                g.media_sink.clone(),
                g.auto_route_roles,
                g.show_system_processes,
            )
        };

        if did_audio_cmd || last_audio_poll.elapsed() >= Duration::from_secs(4) {
            cached_audio = query_audio_state(comms_sink.clone(), media_sink.clone(), auto_route_roles);
            for s in &mut cached_audio.sinks {
                s.disabled = disabled_sinks.contains(&s.name);
            }
            for s in &mut cached_audio.sources {
                s.disabled = disabled_sources.contains(&s.name);
            }

            if auto_route_roles {
                for st in &cached_audio.sink_inputs {
                    if !seen_stream_ids.contains(&st.index) {
                        let target = if st.is_comms {
                            comms_sink.as_deref()
                        } else {
                            media_sink.as_deref()
                        };
                        if let Some(sink_name) = target {
                            let target_idx = cached_audio
                                .sinks
                                .iter()
                                .find(|s| s.name == sink_name && !s.disabled)
                                .map(|s| s.index);
                            if target_idx.is_some() && target_idx != Some(st.device_index) {
                                let idx_str = st.index.to_string();
                                let _ = run_ok("pactl", &["move-sink-input", &idx_str, sink_name]);
                            }
                        }
                    }
                }
            }
            seen_stream_ids = cached_audio.sink_inputs.iter().map(|s| s.index).collect();
            last_audio_poll = now;
        }

        if last_brightness_poll.elapsed() >= Duration::from_secs(20) {
            cached_brightness = query_brightness();
            last_brightness_poll = now;
        }
        if did_sys_cmd || last_sys_poll.elapsed() >= Duration::from_secs(5) {
            cached_net = query_network();
            cached_bt = query_bluetooth();
            cached_power = query_power_profile();
            last_sys_poll = now;
        }
        if did_proc_cmd || last_proc_poll.elapsed() >= Duration::from_secs(3) {
            cached_procs = scan_user_background_apps(show_sys_procs);
            cached_procs.retain(|p| {
                !p.pids
                    .iter()
                    .any(|pid| recently_killed.iter().any(|(kp, _)| kp == pid))
            });
            last_proc_poll = now;
        }

        if did_mem_cmd || last_mem_poll.elapsed() >= Duration::from_secs(5) {
            cached_mem = read_memory_telemetry();
            if auto_clean_mem
                && cached_mem.used_pct >= ram_threshold_pct
                && last_auto_clean.elapsed() >= Duration::from_secs(20)
            {
                let (reclaimed, _dropped) = perform_cache_cleanup(false);
                cleanups_done = cleanups_done.saturating_add(1);
                last_reclaimed_mb = reclaimed;
                last_auto_clean = now;
                cached_mem = read_memory_telemetry();
            } else if prewarm_enabled && last_prewarm.elapsed() >= Duration::from_secs(25) {
                let stats = LaunchCache::load_stats();
                let bins = stats.top_frequent_bins(&prewarm_defaults, 5);
                if !bins.is_empty() {
                    prewarmed_count = prewarm_binaries(&bins, ram_threshold_pct);
                }
                last_prewarm = now;
            }
            cached_mem.cleanups_done = cleanups_done;
            cached_mem.last_reclaimed_mb = last_reclaimed_mb;
            cached_mem.prewarmed_count = prewarmed_count;
            last_mem_poll = now;
        }

        let mut next = cached_audio.clone();
        next.comms_sink = comms_sink;
        next.media_sink = media_sink;
        next.auto_route_roles = auto_route_roles;
        next.show_system_processes = show_sys_procs;
        next.brightness_pct = cached_brightness.0;
        next.brightness_supported = cached_brightness.1;
        next.network_label = cached_net.0.clone();
        next.wifi_enabled = cached_net.1;
        next.bluetooth_on = cached_bt;
        next.power_profile = cached_power.clone();
        next.bg_apps = cached_procs.clone();
        next.memory = cached_mem.clone();

        {
            let mut cur = snapshot.lock().unwrap();
            next.version = cur.version;
            if snapshots_materially_differ(&cur, &next) {
                next.version = cur.version + 1;
                version_atomic.store(next.version, Ordering::Release);
                *cur = next;
            }
        }

        let wait = if !pending_sigkill.is_empty() {
            Duration::from_millis(60)
        } else if did_audio_cmd || did_proc_cmd || latest_brightness.is_some() {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(2500)
        };
        if let Ok(first) = rx.recv_timeout(wait) {
            queued_first = Some(first);
        }
    }
}

fn run_sys_command(snapshot: &Arc<Mutex<SystemSnapshot>>, cmd: SysCommand) {
    match cmd {
        SysCommand::SetSinkVolume { name, pct } => {
            let v = format!("{}%", pct.min(150));
            let _ = run_ok("pactl", &["set-sink-volume", &name, &v]);
        }
        SysCommand::ToggleSinkMute { name } => {
            let _ = run_ok("pactl", &["set-sink-mute", &name, "toggle"]);
        }
        SysCommand::SetSinkMute { name, muted } => {
            let val = if muted { "1" } else { "0" };
            let _ = run_ok("pactl", &["set-sink-mute", &name, val]);
        }
        SysCommand::SetDefaultSink { name } => {
            let _ = run_ok("pactl", &["set-default-sink", &name]);
        }
        SysCommand::SetSourceVolume { name, pct } => {
            let v = format!("{}%", pct.min(150));
            let _ = run_ok("pactl", &["set-source-volume", &name, &v]);
        }
        SysCommand::ToggleSourceMute { name } => {
            let _ = run_ok("pactl", &["set-source-mute", &name, "toggle"]);
        }
        SysCommand::SetSourceMute { name, muted } => {
            let val = if muted { "1" } else { "0" };
            let _ = run_ok("pactl", &["set-source-mute", &name, val]);
        }
        SysCommand::SetDefaultSource { name } => {
            let _ = run_ok("pactl", &["set-default-source", &name]);
        }
        SysCommand::SetAudioDeviceDisabled {
            is_input,
            name,
            disabled,
        } => {
            let mute_cmd = if is_input {
                "set-source-mute"
            } else {
                "set-sink-mute"
            };
            let susp_cmd = if is_input {
                "suspend-source"
            } else {
                "suspend-sink"
            };
            if disabled {
                // Switch default & move active streams to a healthy sibling before muting/suspending.
                let fallback = {
                    let g = snapshot.lock().unwrap();
                    let list = if is_input { &g.sources } else { &g.sinks };
                    list.iter()
                        .find(|d| d.name != name && !d.disabled)
                        .map(|d| d.name.clone())
                };
                if let Some(fb) = fallback {
                    let default_cmd = if is_input {
                        "set-default-source"
                    } else {
                        "set-default-sink"
                    };
                    let cur_is_default = {
                        let g = snapshot.lock().unwrap();
                        if is_input {
                            g.default_source == name
                        } else {
                            g.default_sink == name
                        }
                    };
                    if cur_is_default {
                        let _ = run_ok("pactl", &[default_cmd, &fb]);
                    }
                    if !is_input {
                        let (target_idx, streams) = {
                            let g = snapshot.lock().unwrap();
                            (
                                g.sinks.iter().find(|s| s.name == name).map(|s| s.index),
                                g.sink_inputs.clone(),
                            )
                        };
                        if let Some(dev_idx) = target_idx {
                            for st in streams {
                                if st.device_index == dev_idx {
                                    let idx_str = st.index.to_string();
                                    let _ = run_ok("pactl", &["move-sink-input", &idx_str, &fb]);
                                }
                            }
                        }
                    }
                }
                let _ = run_ok("pactl", &[mute_cmd, &name, "1"]);
                let _ = run_ok("pactl", &[susp_cmd, &name, "1"]);
            } else {
                let _ = run_ok("pactl", &[susp_cmd, &name, "0"]);
                let _ = run_ok("pactl", &[mute_cmd, &name, "0"]);
            }
        }
        SysCommand::SetStreamVolume {
            is_input,
            index,
            pct,
        } => {
            let sub = if is_input {
                "set-source-output-volume"
            } else {
                "set-sink-input-volume"
            };
            let idx = index.to_string();
            let v = format!("{}%", pct.min(150));
            let _ = run_ok("pactl", &[sub, &idx, &v]);
        }
        SysCommand::ToggleStreamMute { is_input, index } => {
            let sub = if is_input {
                "set-source-output-mute"
            } else {
                "set-sink-input-mute"
            };
            let idx = index.to_string();
            let _ = run_ok("pactl", &[sub, &idx, "toggle"]);
        }
        SysCommand::MoveStream {
            is_input,
            index,
            target,
        } => {
            let sub = if is_input {
                "move-source-output"
            } else {
                "move-sink-input"
            };
            let idx = index.to_string();
            let _ = run_ok("pactl", &[sub, &idx, &target]);
        }
        SysCommand::RouteRole {
            is_comms,
            target_sink: Some(target_sink),
        } => {
            let streams = snapshot.lock().unwrap().sink_inputs.clone();
            for st in streams {
                if st.is_comms == is_comms {
                    let idx = st.index.to_string();
                    let _ = run_ok("pactl", &["move-sink-input", &idx, &target_sink]);
                }
            }
        }
        SysCommand::RouteRole {
            target_sink: None, ..
        } => {}
        SysCommand::SetPowerProfile { profile } => {
            let _ = run_ok("powerprofilesctl", &["set", &profile]);
        }
        SysCommand::ToggleWifi => {
            let state = run_stdout("nmcli", &["radio", "wifi"]).unwrap_or_default();
            let next = if state.trim() == "enabled" {
                "off"
            } else {
                "on"
            };
            let _ = run_ok("nmcli", &["radio", "wifi", next]);
        }
        SysCommand::ToggleBluetooth => {
            let on = query_bluetooth();
            let arg = if on { "off" } else { "on" };
            let _ = run_ok("bluetoothctl", &["power", arg]);
        }
        SysCommand::RefreshNow
        | SysCommand::SetBrightness { .. }
        | SysCommand::SetAutoRouteRoles { .. }
        | SysCommand::SetShowSystemProcesses { .. }
        | SysCommand::KillProcess { .. }
        | SysCommand::CleanRamCache { .. }
        | SysCommand::ConfigureMemoryPolicy { .. }
        | SysCommand::SetDisabledAudioLists { .. } => {}
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcStatMeta {
    pid: u32,
    state: char,
    ppid: u32,
    pgrp: u32,
    uid: u32,
}

fn parse_proc_stat(pid: u32) -> Option<(ProcStatMeta, String)> {
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let name = raw[open + 1..close].to_string();
    let rest = raw.get(close + 2..)?;
    let mut parts = rest.split_whitespace();
    let state = parts.next()?.chars().next().unwrap_or('S');
    let ppid = parts.next()?.parse::<u32>().ok()?;
    let pgrp = parts.next()?.parse::<u32>().ok()?;
    let _sid = parts.next()?.parse::<u32>().ok()?;

    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(u32::MAX);

    Some((
        ProcStatMeta {
            pid,
            state,
            ppid,
            pgrp,
            uid,
        },
        name,
    ))
}

fn is_live_non_zombie(pid: u32) -> bool {
    parse_proc_stat(pid)
        .map(|(m, _)| m.state != 'Z' && m.state != 'X')
        .unwrap_or(false)
}

/// Collects all live descendant child PIDs (and same-UID isolated process groups) for `roots`.
pub fn collect_process_tree(roots: &[u32], include_safe_pgrp: bool) -> Vec<u32> {
    let my_pid = std::process::id();
    let my_uid = unsafe { libc::getuid() };
    let my_pgrp = unsafe { libc::getpgrp() } as u32;
    let my_sid = unsafe { libc::getsid(0) } as u32;

    let Ok(entries) = fs::read_dir("/proc") else {
        return roots.to_vec();
    };
    let mut children_map: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    let mut pgrp_map: std::collections::HashMap<u32, Vec<(u32, String)>> =
        std::collections::HashMap::new();
    let mut root_pgrps = Vec::new();

    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid <= 1 || pid == my_pid {
            continue;
        }
        let Some((meta, name)) = parse_proc_stat(pid) else {
            continue;
        };
        if meta.uid != my_uid || meta.state == 'Z' || meta.state == 'X' {
            continue;
        }
        children_map.entry(meta.ppid).or_default().push(meta.pid);
        pgrp_map.entry(meta.pgrp).or_default().push((meta.pid, name));
        if roots.contains(&meta.pid)
            && meta.pgrp > 1
            && meta.pgrp != my_pgrp
            && meta.pgrp != my_sid
            && !root_pgrps.contains(&meta.pgrp)
        {
            root_pgrps.push(meta.pgrp);
        }
    }

    let mut targets: Vec<u32> = roots
        .iter()
        .copied()
        .filter(|&p| p > 1 && p != my_pid)
        .collect();
    let mut queue = targets.clone();
    while let Some(cur) = queue.pop() {
        if let Some(kids) = children_map.get(&cur) {
            for &kid in kids {
                if !targets.contains(&kid) {
                    targets.push(kid);
                    queue.push(kid);
                }
            }
        }
    }

    if include_safe_pgrp {
        for pgrp in root_pgrps {
            if let Some(members) = pgrp_map.get(&pgrp) {
                let protected = members.iter().any(|(_, n)| {
                    n.starts_with("velox-")
                        || n == "Xwayland"
                        || n.starts_with("dbus")
                        || n.starts_with("pipewire")
                        || n.starts_with("systemd")
                });
                if !protected {
                    for (mpid, _) in members {
                        if !targets.contains(mpid) && *mpid > 1 && *mpid != my_pid {
                            targets.push(*mpid);
                        }
                    }
                }
            }
        }
    }

    targets
}

fn kill_pids(
    pids: &[u32],
    force: bool,
    pending_sigkill: &mut Vec<(u32, Instant)>,
    recently_killed: &mut Vec<(u32, Instant)>,
) {
    let my_pid = std::process::id();
    let tree = collect_process_tree(pids, true);
    let now = Instant::now();
    let deadline = now + Duration::from_millis(140);
    let suppress_until = now + Duration::from_millis(3500);
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };

    for pid in tree {
        if pid <= 1 || pid == my_pid {
            continue;
        }
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
        if !force && !pending_sigkill.iter().any(|(p, _)| *p == pid) {
            pending_sigkill.push((pid, deadline));
        }
        if let Some(entry) = recently_killed.iter_mut().find(|(p, _)| *p == pid) {
            entry.1 = suppress_until;
        } else {
            recently_killed.push((pid, suppress_until));
        }
    }
}

fn snapshots_materially_differ(a: &SystemSnapshot, b: &SystemSnapshot) -> bool {
    if a.sinks != b.sinks
        || a.sources != b.sources
        || a.sink_inputs != b.sink_inputs
        || a.source_outputs != b.source_outputs
        || a.default_sink != b.default_sink
        || a.default_source != b.default_source
        || a.comms_sink != b.comms_sink
        || a.media_sink != b.media_sink
        || a.auto_route_roles != b.auto_route_roles
        || a.show_system_processes != b.show_system_processes
        || a.brightness_pct != b.brightness_pct
        || a.brightness_supported != b.brightness_supported
        || a.network_label != b.network_label
        || a.wifi_enabled != b.wifi_enabled
        || a.bluetooth_on != b.bluetooth_on
        || a.power_profile != b.power_profile
        || a.bg_apps.len() != b.bg_apps.len()
        || a.memory.used_pct.abs_diff(b.memory.used_pct) >= 3
        || a.memory.cleanups_done != b.memory.cleanups_done
        || a.memory.prewarmed_count != b.memory.prewarmed_count
        || a.memory.wm_rss_mb.abs_diff(b.memory.wm_rss_mb) >= 16
    {
        return true;
    }
    for (pa, pb) in a.bg_apps.iter().zip(b.bg_apps.iter()) {
        if pa.pid != pb.pid || pa.name != pb.name || pa.rss_mb.abs_diff(pb.rss_mb) >= 16 {
            return true;
        }
    }
    false
}

/// Scans `/proc` for user-owned desktop applications, filtering out system/session infrastructure
/// unless `include_system` is enabled, and grouping multi-process helpers into one closable entry.
pub fn scan_user_background_apps(include_system: bool) -> Vec<BgProcess> {
    let my_pid = std::process::id();
    let my_uid = unsafe { libc::getuid() };
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };

    const IGNORED_COMMS: &[&str] = &[
        "velox-",
        "systemd",
        "(sd-",
        "dbus-",
        "pipewire",
        "wireplumber",
        "pulseaudio",
        "xdg-",
        "at-spi",
        "gvfs",
        "fusermount",
        "ssh-agent",
        "gpg-agent",
        "gnome-keyring",
        "gnome-session",
        "gnome-shell",
        "gsd-",
        "kwallet",
        "kded",
        "kactivity",
        "kdeinit",
        "plasma-",
        "polkit",
        "dconf",
        "gconf",
        "goa-",
        "evolution-",
        "xsettingsd",
        "xfconfd",
        "xfsettingsd",
        "wl-paste",
        "wl-copy",
        "cliphist",
        "udisksd",
        "upowerd",
        "obexd",
        "mpris-proxy",
        "rtkit",
        "tracker-",
        "localsearch",
        "geoclue",
        "orca",
        "uresourced",
        "gamemoded",
        "ibus",
        "fcitx",
        "bash",
        "zsh",
        "fish",
        "sh",
        "Xwayland",
        "lightdm",
        "sddm",
        "gdm",
    ];

    let mut grouped: std::collections::BTreeMap<String, BgProcess> = std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(pid_str) = fname.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == my_pid || pid <= 1 {
            continue;
        }
        let proc_dir = entry.path();
        let Ok(status) = fs::read_to_string(proc_dir.join("status")) else {
            continue;
        };

        let mut uid_ok = false;
        let mut is_zombie = false;
        let mut name = String::new();
        let mut rss_kb = 0u32;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Name:") {
                name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("State:") {
                let st = rest.trim_start();
                is_zombie = st.starts_with('Z') || st.starts_with('X');
            } else if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(first) = rest.split_whitespace().next() {
                    uid_ok = first.parse::<u32>().ok() == Some(my_uid);
                }
            } else if let Some(rest) = line.strip_prefix("VmRSS:") {
                if let Some(kb) = rest.split_whitespace().next() {
                    rss_kb = kb.parse::<u32>().unwrap_or(0);
                }
            }
        }
        if !uid_ok || is_zombie || name.is_empty() || rss_kb < 4096 {
            continue;
        }

        // Always hide Velox internals and Xwayland server from user kill list.
        if name.starts_with("velox-") || name == "Xwayland" {
            continue;
        }

        if !include_system {
            if IGNORED_COMMS.iter().any(|ignored| name.starts_with(ignored)) {
                continue;
            }
            if let Ok(cgroup) = fs::read_to_string(proc_dir.join("cgroup")) {
                // Skip systemd background/session units unless launched inside app.slice / interactive scope.
                let is_service = cgroup.contains(".service") && !cgroup.contains("app.slice");
                if is_service {
                    continue;
                }
            }
        }

        if let Ok(cmdline) = fs::read_to_string(proc_dir.join("cmdline")) {
            if cmdline.contains("--type=renderer")
                || cmdline.contains("--type=gpu-process")
                || cmdline.contains("--type=utility")
                || cmdline.contains("--type=zygote")
                || cmdline.contains("chrome_crashpad")
            {
                continue;
            }
            if !include_system && cmdline.contains("--daemon") {
                continue;
            }
        }

        let key = name.to_ascii_lowercase();
        let rss_mb = (rss_kb / 1024).max(1);
        grouped
            .entry(key)
            .and_modify(|e| {
                e.rss_mb = e.rss_mb.saturating_add(rss_mb);
                e.pids.push(pid);
            })
            .or_insert_with(|| BgProcess {
                pid,
                pids: vec![pid],
                name,
                rss_mb,
            });
    }

    let mut procs: Vec<BgProcess> = grouped.into_values().collect();
    procs.sort_by(|a, b| b.rss_mb.cmp(&a.rss_mb));
    procs.truncate(14);
    procs
}

fn query_audio_state(
    comms_sink: Option<String>,
    media_sink: Option<String>,
    auto_route_roles: bool,
) -> SystemSnapshot {
    let default_sink = run_stdout("pactl", &["get-default-sink"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let default_source = run_stdout("pactl", &["get-default-source"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let sinks_json = run_stdout("pactl", &["-f", "json", "list", "sinks"]).unwrap_or_default();
    let sources_json = run_stdout("pactl", &["-f", "json", "list", "sources"]).unwrap_or_default();
    let inputs_json =
        run_stdout("pactl", &["-f", "json", "list", "sink-inputs"]).unwrap_or_default();
    let outputs_json =
        run_stdout("pactl", &["-f", "json", "list", "source-outputs"]).unwrap_or_default();

    let sinks = parse_pactl_devices(&sinks_json, &default_sink, false);
    let sources = parse_pactl_devices(&sources_json, &default_source, true);
    let sink_inputs = parse_pactl_streams(&inputs_json, "sink");
    let source_outputs = parse_pactl_streams(&outputs_json, "source");

    SystemSnapshot {
        version: 0,
        sinks,
        sources,
        sink_inputs,
        source_outputs,
        default_sink,
        default_source,
        comms_sink,
        media_sink,
        auto_route_roles,
        ..SystemSnapshot::default()
    }
}

pub fn parse_pactl_devices(
    json_str: &str,
    default_name: &str,
    filter_monitors: bool,
) -> Vec<AudioDevice> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json_str) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || (filter_monitors && name.ends_with(".monitor")) {
            continue;
        }
        let index = item.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let desc = item
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&name)
            .to_string();
        let muted = item.get("mute").and_then(|v| v.as_bool()).unwrap_or(false);
        let volume_pct = extract_volume_pct(&item);
        let is_default = !default_name.is_empty() && name == default_name;
        out.push(AudioDevice {
            index,
            name,
            description: desc,
            volume_pct,
            muted,
            is_default,
            disabled: false,
        });
    }
    out
}

pub fn parse_pactl_streams(json_str: &str, device_key: &str) -> Vec<AudioStream> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json_str) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let index = item.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let device_index = item.get(device_key).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let muted = item.get("mute").and_then(|v| v.as_bool()).unwrap_or(false);
        let volume_pct = extract_volume_pct(&item);

        let props = item.get("properties");
        let app_name = props
            .and_then(|p| p.get("application.name").or_else(|| p.get("node.name")))
            .and_then(|v| v.as_str())
            .unwrap_or("App")
            .to_string();
        let media_name = props
            .and_then(|p| p.get("media.name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let role = props
            .and_then(|p| p.get("media.role"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let lower_app = app_name.to_ascii_lowercase();
        let is_comms = role.contains("comm")
            || role.contains("phone")
            || lower_app.contains("discord")
            || lower_app.contains("teamspeak")
            || lower_app.contains("zoom")
            || lower_app.contains("slack")
            || lower_app.contains("skype")
            || lower_app.contains("mumble")
            || lower_app.contains("webrtc");

        out.push(AudioStream {
            index,
            app_name,
            media_name,
            device_index,
            volume_pct,
            muted,
            is_comms,
        });
    }
    out
}

fn extract_volume_pct(item: &Value) -> u8 {
    let Some(Value::Object(vol_map)) = item.get("volume") else {
        return 100;
    };
    let mut sum = 0u32;
    let mut count = 0u32;
    for ch in vol_map.values() {
        if let Some(pct_str) = ch.get("value_percent").and_then(|v| v.as_str()) {
            let trimmed = pct_str.trim().trim_end_matches('%');
            if let Ok(val) = trimmed.parse::<u32>() {
                sum += val;
                count += 1;
            }
        }
    }
    if count > 0 {
        (sum / count).min(150) as u8
    } else {
        100
    }
}

fn backlight_dir() -> Option<PathBuf> {
    fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .next()
}

fn query_brightness() -> (u8, bool) {
    if let Some(dir) = backlight_dir() {
        if let (Ok(cur), Ok(max)) = (
            fs::read_to_string(dir.join("brightness")),
            fs::read_to_string(dir.join("max_brightness")),
        ) {
            if let (Ok(c), Ok(m)) = (cur.trim().parse::<u64>(), max.trim().parse::<u64>()) {
                if m > 0 {
                    return (((c * 100) / m).clamp(5, 100) as u8, true);
                }
            }
        }
    }
    if let Some(out) = run_stdout("ddcutil", &["getvcp", "10", "--brief"]) {
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() >= 4 {
            if let Ok(val) = parts[3].parse::<u8>() {
                return (val.clamp(5, 100), true);
            }
        }
    }
    (80, true)
}

fn apply_brightness(pct: u8) {
    if let Some(dir) = backlight_dir() {
        if let Ok(max_str) = fs::read_to_string(dir.join("max_brightness")) {
            if let Ok(max_val) = max_str.trim().parse::<u64>() {
                let target = ((max_val * pct as u64) / 100).max(1);
                if fs::write(dir.join("brightness"), target.to_string()).is_ok() {
                    return;
                }
            }
        }
    }
    let val = pct.to_string();
    let _ = run_ok("ddcutil", &["setvcp", "10", &val, "--noverify"]);
}

fn query_network() -> (String, bool) {
    let wifi_enabled = run_stdout("nmcli", &["radio", "wifi"])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(true);
    let label = run_stdout(
        "nmcli",
        &["-t", "-f", "NAME,TYPE,STATE", "connection", "show", "--active"],
    )
    .and_then(|raw| {
        raw.lines().find(|l| !l.contains(":loopback:")).map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            parts.first().copied().unwrap_or("Online").to_string()
        })
    })
    .unwrap_or_else(|| {
        if wifi_enabled {
            "Offline".to_string()
        } else {
            "Net Off".to_string()
        }
    });
    (label, wifi_enabled)
}

fn query_bluetooth() -> bool {
    run_stdout("bluetoothctl", &["show"])
        .map(|out| out.lines().any(|l| l.trim() == "Powered: yes"))
        .unwrap_or(false)
}

fn query_power_profile() -> String {
    run_stdout("powerprofilesctl", &["get"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "balanced".to_string())
}

fn run_stdout(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

fn run_ok(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pactl_json_devices_and_streams() {
        let sinks_json = r#"[
          {
            "index": 42,
            "name": "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "description": "Built-in Audio Analog Stereo",
            "mute": false,
            "volume": {
              "front-left": { "value_percent": "74%" },
              "front-right": { "value_percent": "74%" }
            }
          }
        ]"#;
        let sinks = parse_pactl_devices(
            sinks_json,
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            false,
        );
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].volume_pct, 74);
        assert!(sinks[0].is_default);
        assert!(!sinks[0].disabled);

        let streams_json = r#"[
          {
            "index": 91,
            "sink": 42,
            "mute": false,
            "volume": { "front-left": { "value_percent": "85%" } },
            "properties": {
              "application.name": "Discord",
              "media.name": "Voice Playback"
            }
          }
        ]"#;
        let streams = parse_pactl_streams(streams_json, "sink");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].app_name, "Discord");
        assert_eq!(streams[0].volume_pct, 85);
        assert!(streams[0].is_comms);
    }
}
