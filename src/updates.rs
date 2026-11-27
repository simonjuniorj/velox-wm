use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::notifications::{
    NotificationAction, NotificationItem, NotificationService, NotificationUrgency,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapZone {
    LeftHalf,
    RightHalf,
    Maximize,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub const SEAMLESS_RELOAD_EXIT_CODE: i32 = 42;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateStatus {
    pub current_version: String,
    pub local_update_ready: bool,
    pub github_latest_tag: Option<String>,
    pub github_release_url: Option<String>,
    pub checking: bool,
    pub last_check_note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReloadWindowEntry {
    pub workspace: usize,
    pub order_index: usize,
    pub cmdline: Vec<String>,
    pub app_id: String,
    pub title: String,
    pub output_name: Option<String>,
    pub rect: Option<[i32; 4]>,
    pub restore_rect: Option<[i32; 4]>,
    pub floating: bool,
    pub always_on_top: bool,
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub show_desktop_hidden: bool,
    pub snapped: Option<SnapZone>,
    pub focused: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReloadSessionSnapshot {
    pub created_epoch: u64,
    pub active_workspace: usize,
    pub windows: Vec<ReloadWindowEntry>,
    pub notifications: Vec<NotificationItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinaryFingerprint {
    mtime_nanos: u128,
    len: u64,
    ino: u64,
}

pub enum UpdateCommand {
    SetRepo { repo: String, enabled: bool },
    CheckNow,
}

pub struct UpdateWatcher {
    status: Arc<Mutex<UpdateStatus>>,
    version: Arc<AtomicU64>,
    tx: mpsc::Sender<UpdateCommand>,
    stop: Arc<AtomicBool>,
}

impl UpdateWatcher {
    pub fn start(
        github_repo: String,
        github_enabled: bool,
        notifier: Arc<NotificationService>,
    ) -> Self {
        let boot_path = detect_monitored_binary();
        let boot_fp = read_fingerprint(&boot_path);
        let status = Arc::new(Mutex::new(UpdateStatus {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            local_update_ready: false,
            github_latest_tag: None,
            github_release_url: None,
            checking: false,
            last_check_note: "Monitoring installed binaries & releases".to_string(),
        }));
        let version = Arc::new(AtomicU64::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();

        let status_bg = status.clone();
        let ver_bg = version.clone();
        let stop_bg = stop.clone();

        thread::Builder::new()
            .name("velox-update-watch".to_string())
            .spawn(move || {
                let mut repo = github_repo;
                let mut gh_enabled = github_enabled;
                let mut last_gh_check = SystemTime::UNIX_EPOCH;
                let mut announced_local = false;
                let mut announced_gh_tag = String::new();

                while !stop_bg.load(Ordering::Acquire) {
                    let mut force_gh = false;
                    while let Ok(cmd) = rx.try_recv() {
                        match cmd {
                            UpdateCommand::SetRepo {
                                repo: next_repo,
                                enabled,
                            } => {
                                repo = next_repo;
                                gh_enabled = enabled;
                            }
                            UpdateCommand::CheckNow => {
                                force_gh = true;
                            }
                        }
                    }

                    // 1. Detect freshly installed /usr/local/bin/velox-wm (e.g. after running install.sh)
                    if let (Some(boot), Some(now_fp)) = (boot_fp, read_fingerprint(&boot_path)) {
                        if now_fp != boot {
                            {
                                let mut st = status_bg.lock().unwrap();
                                if !st.local_update_ready {
                                    st.local_update_ready = true;
                                    st.last_check_note =
                                        "New build installed on disk — ready to reload seamlessly"
                                            .to_string();
                                    ver_bg.fetch_add(1, Ordering::AcqRel);
                                }
                            }
                            if !announced_local {
                                announced_local = true;
                                notifier.push_internal(
                                    "Velox Updater",
                                    "Update Installed & Ready",
                                    "Fresh velox-wm binary detected cleanly. Reload seamlessly without logging out.",
                                    NotificationUrgency::Critical,
                                    vec![NotificationAction {
                                        key: "velox-reload".to_string(),
                                        label: "Reload WM Now".to_string(),
                                    }],
                                    true,
                                    0,
                                );
                            }
                        }
                    }

                    // 2. Check GitHub Releases cleanly when repository is configured (non-blocking 6s timeout).
                    let should_poll_gh = gh_enabled
                        && !repo.trim().is_empty()
                        && (force_gh
                            || last_gh_check
                                .elapsed()
                                .map(|e| e >= Duration::from_secs(1800))
                                .unwrap_or(true));
                    if should_poll_gh {
                        last_gh_check = SystemTime::now();
                        {
                            let mut st = status_bg.lock().unwrap();
                            st.checking = true;
                            ver_bg.fetch_add(1, Ordering::AcqRel);
                        }
                        let res = fetch_latest_github_release(&repo);
                        let mut st = status_bg.lock().unwrap();
                        st.checking = false;
                        match res {
                            Ok(Some((tag, url))) => {
                                if is_newer_version(&tag, &st.current_version) {
                                    st.github_latest_tag = Some(tag.clone());
                                    st.github_release_url = Some(url.clone());
                                    st.last_check_note =
                                        format!("GitHub release {tag} available");
                                    if announced_gh_tag != tag {
                                        announced_gh_tag = tag.clone();
                                        notifier.push_internal(
                                            "Velox Updater",
                                            &format!("Velox {tag} Available on GitHub"),
                                            "Run install.sh after pulling updates, then click Reload WM Now.",
                                            NotificationUrgency::Normal,
                                            vec![NotificationAction {
                                                key: "velox-reload".to_string(),
                                                label: "Reload WM".to_string(),
                                            }],
                                            true,
                                            0,
                                        );
                                    }
                                } else {
                                    st.github_latest_tag = None;
                                    st.last_check_note =
                                        format!("Up to date (v{})", st.current_version);
                                }
                            }
                            Ok(None) => {
                                st.last_check_note =
                                    format!("Up to date (v{})", st.current_version);
                            }
                            Err(msg) => {
                                st.last_check_note = msg;
                            }
                        }
                        ver_bg.fetch_add(1, Ordering::AcqRel);
                    }

                    for _ in 0..20 {
                        if stop_bg.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(200));
                    }
                }
            })
            .ok();

        Self {
            status,
            version,
            tx,
            stop,
        }
    }

    pub fn configure(&self, repo: String, enabled: bool) {
        let _ = self.tx.send(UpdateCommand::SetRepo { repo, enabled });
    }

    pub fn trigger_check(&self) {
        let _ = self.tx.send(UpdateCommand::CheckNow);
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn reload_state_path() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(rt).join("velox-reload-state.json");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/velox-reload-{uid}.json"))
}

pub fn external_cmd_path() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(rt).join("velox-wm-cmd");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/velox-wm-cmd-{uid}"))
}

pub fn send_external_cmd(cmd: &str) {
    let _ = fs::write(external_cmd_path(), cmd.as_bytes());
}

pub fn take_external_cmd() -> Option<String> {
    let p = external_cmd_path();
    let raw = fs::read_to_string(&p).ok()?;
    let _ = fs::remove_file(&p);
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn settings_cmd_path() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(rt).join("velox-settings-cmd");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/velox-settings-cmd-{uid}"))
}

pub fn send_settings_cmd(cmd: &str) {
    let _ = fs::write(settings_cmd_path(), cmd.as_bytes());
}

pub fn take_settings_cmd() -> Option<String> {
    let p = settings_cmd_path();
    let raw = fs::read_to_string(&p).ok()?;
    let _ = fs::remove_file(&p);
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn save_reload_snapshot(snap: &ReloadSessionSnapshot) {
    let p = reload_state_path();
    if let Ok(json) = serde_json::to_string(snap) {
        let _ = fs::write(&p, json);
    }
}

pub fn take_fresh_reload_snapshot() -> Option<ReloadSessionSnapshot> {
    let p = reload_state_path();
    let raw = fs::read_to_string(&p).ok()?;
    let _ = fs::remove_file(&p);
    let snap: ReloadSessionSnapshot = serde_json::from_str(&raw).ok()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(snap.created_epoch) <= 30 {
        Some(snap)
    } else {
        None
    }
}

pub fn read_proc_cmdline(pid: u32) -> Option<Vec<String>> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let parts: Vec<String> = bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok().map(str::to_string))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

/// Checks whether `child_pid` equals or descends from `target_pid` via `/proc/<pid>/stat` PPID links.
pub fn proc_ancestors_include(mut child_pid: u32, target_pid: u32) -> bool {
    if child_pid == 0 || target_pid == 0 {
        return false;
    }
    for _ in 0..10 {
        if child_pid == target_pid {
            return true;
        }
        let Ok(stat) = fs::read_to_string(format!("/proc/{child_pid}/stat")) else {
            break;
        };
        // Field 4 after closing `)` of comm is PPID.
        let Some(rparen) = stat.rfind(')') else {
            break;
        };
        let mut fields = stat[rparen + 1..].split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            break;
        };
        if ppid <= 1 || ppid == child_pid {
            break;
        }
        child_pid = ppid;
    }
    false
}

pub fn detect_monitored_binary() -> PathBuf {
    let system_bin = PathBuf::from("/usr/local/bin/velox-wm");
    if let Ok(cur) = std::env::current_exe() {
        if cur == system_bin {
            return system_bin;
        }
        return cur;
    }
    system_bin
}

fn read_fingerprint(path: &Path) -> Option<BinaryFingerprint> {
    let meta = fs::metadata(path).ok()?;
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(BinaryFingerprint {
        mtime_nanos,
        len: meta.len(),
        ino: meta.ino(),
    })
}

fn fetch_latest_github_release(repo: &str) -> Result<Option<(String, String)>, String> {
    let cleaned = repo.trim().trim_start_matches("https://github.com/").trim_matches('/');
    if !cleaned.contains('/') {
        return Err("Set owner/repo in config".to_string());
    }
    let url = format!("https://api.github.com/repos/{cleaned}/releases/latest");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "6",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: velox-wm",
            &url,
        ])
        .output()
        .map_err(|_| "curl not available".to_string())?;
    if !out.status.success() {
        return Err("Offline or no GitHub releases yet".to_string());
    }
    let val: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "Invalid release JSON".to_string())?;
    let Some(tag) = val.get("tag_name").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let html = val
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com")
        .to_string();
    Ok(Some((tag.to_string(), html)))
}

pub fn parse_semver(tag: &str) -> Option<(u64, u64, u64)> {
    let stripped = tag.trim().trim_start_matches('v');
    let core = stripped.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse::<u64>().ok()?;
    let minor = it.next()?.parse::<u64>().ok()?;
    let patch = it.next().and_then(|p| p.parse::<u64>().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

pub fn is_newer_version(remote_tag: &str, local_version: &str) -> bool {
    match (parse_semver(remote_tag), parse_semver(local_version)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_handles_tags() {
        assert!(is_newer_version("v0.7.0", "0.6.0"));
        assert!(is_newer_version("0.6.1", "0.6.0"));
        assert!(!is_newer_version("v0.6.0", "0.6.0"));
        assert!(!is_newer_version("v0.5.9", "0.6.0"));
    }

    #[test]
    fn reload_snapshot_roundtrip() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let snap = ReloadSessionSnapshot {
            created_epoch: now,
            active_workspace: 2,
            windows: vec![ReloadWindowEntry {
                workspace: 2,
                order_index: 1,
                cmdline: vec!["alacritty".into()],
                app_id: "Alacritty".into(),
                title: "Alacritty".into(),
                output_name: Some("DP-1".into()),
                rect: Some([10, 10, 940, 1020]),
                restore_rect: Some([100, 100, 800, 600]),
                floating: true,
                always_on_top: true,
                fullscreen: false,
                maximized: false,
                minimized: false,
                show_desktop_hidden: false,
                snapped: Some(SnapZone::LeftHalf),
                focused: true,
            }],
            notifications: vec![],
        };
        save_reload_snapshot(&snap);
        let loaded = take_fresh_reload_snapshot().expect("fresh snapshot");
        assert_eq!(loaded.active_workspace, 2);
        assert_eq!(loaded.windows.len(), 1);
        assert_eq!(loaded.windows[0].order_index, 1);
        assert_eq!(loaded.windows[0].snapped, Some(SnapZone::LeftHalf));
        assert!(loaded.windows[0].always_on_top);
        assert!(loaded.windows[0].focused);
        assert!(proc_ancestors_include(std::process::id(), std::process::id()));
    }
}
