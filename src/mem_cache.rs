use std::{
    collections::HashMap,
    fs,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryTelemetry {
    pub total_mb: u32,
    pub available_mb: u32,
    pub cached_mb: u32,
    pub reclaimable_mb: u32,
    pub used_pct: u8,
    pub wm_rss_mb: u32,
    pub cleanups_done: u32,
    pub last_reclaimed_mb: u32,
    pub prewarmed_count: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchCache {
    pub dir_mtimes: Vec<(String, u64)>,
    pub launch_counts: HashMap<String, u32>,
}

impl LaunchCache {
    pub fn cache_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|s| !s.is_empty())
                    .map(|h| PathBuf::from(h).join(".cache"))
            })
            .map(|p| p.join("velox-wm"))
    }

    pub fn index_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("apps-index.json"))
    }

    pub fn stats_path() -> Option<PathBuf> {
        Self::cache_dir().map(|d| d.join("launch-cache.json"))
    }

    pub fn load_stats() -> Self {
        Self::stats_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save_stats(&self) {
        let Some(path) = Self::stats_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(raw) = serde_json::to_string(self) {
            if fs::write(&tmp, raw).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }

    pub fn record_launch(bin: &str) {
        if bin.is_empty() {
            return;
        }
        let mut stats = Self::load_stats();
        let entry = stats.launch_counts.entry(bin.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
        stats.save_stats();
    }

    pub fn top_frequent_bins(&self, defaults: &[String], max_count: usize) -> Vec<String> {
        let mut ranked: Vec<(&String, &u32)> = self.launch_counts.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1));
        let mut out: Vec<String> = ranked
            .into_iter()
            .take(max_count)
            .map(|(k, _)| k.clone())
            .collect();
        for def in defaults {
            if out.len() >= max_count {
                break;
            }
            if !def.is_empty() && !out.contains(def) {
                out.push(def.clone());
            }
        }
        out
    }
}

pub fn current_dir_mtimes(dirs: &[PathBuf]) -> Vec<(String, u64)> {
    dirs.iter()
        .map(|d| {
            let mtime = fs::metadata(d)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|dur| dur.as_secs())
                .unwrap_or(0);
            (d.to_string_lossy().into_owned(), mtime)
        })
        .collect()
}

pub fn read_memory_telemetry() -> MemoryTelemetry {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    parse_meminfo_and_rss(&meminfo, &status)
}

pub fn parse_meminfo_and_rss(meminfo: &str, self_status: &str) -> MemoryTelemetry {
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    let mut cached_kb = 0u64;
    let mut reclaimable_kb = 0u64;

    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("Cached:") {
            cached_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("SReclaimable:") {
            reclaimable_kb = parse_kb(rest);
        }
    }

    let mut rss_kb = 0u64;
    for line in self_status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kb = parse_kb(rest);
            break;
        }
    }

    let used_kb = total_kb.saturating_sub(avail_kb);
    let used_pct = if total_kb > 0 {
        ((used_kb * 100) / total_kb).clamp(0, 100) as u8
    } else {
        0
    };

    MemoryTelemetry {
        total_mb: (total_kb / 1024) as u32,
        available_mb: (avail_kb / 1024) as u32,
        cached_mb: (cached_kb / 1024) as u32,
        reclaimable_mb: (reclaimable_kb / 1024) as u32,
        used_pct,
        wm_rss_mb: (rss_kb / 1024) as u32,
        cleanups_done: 0,
        last_reclaimed_mb: 0,
        prewarmed_count: 0,
    }
}

fn parse_kb(rest: &str) -> u64 {
    rest.split_whitespace()
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Returns unused glibc heap arenas back to the Linux kernel cleanly.
pub fn trim_process_heaps() {
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Cleans expired velox temporary files + process heaps, and optionally drops Linux page cache
/// (only if permitted without blocking or prompting). Never runs on compositor input/render thread.
pub fn perform_cache_cleanup(deep_kernel: bool) -> (u32, bool) {
    let before = read_memory_telemetry();
    trim_process_heaps();

    if let Some(dir) = LaunchCache::cache_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("tmp") {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    let mut kernel_dropped = false;
    if deep_kernel {
        unsafe {
            libc::sync();
        }
        let drop_path = Path::new("/proc/sys/vm/drop_caches");
        if fs::write(drop_path, "1\n").is_ok() {
            kernel_dropped = true;
        }
    }

    let after = read_memory_telemetry();
    let reclaimed_mb = after.available_mb.saturating_sub(before.available_mb);
    (reclaimed_mb, kernel_dropped)
}

/// Gently readaheads up to `max_bytes_per_bin` of frequently launched ELF binaries into page cache
/// via `posix_fadvise(POSIX_FADV_WILLNEED)` so apps launch smoothly while respecting RAM budget.
pub fn prewarm_binaries(bins: &[String], threshold_pct: u8) -> u16 {
    let mem = read_memory_telemetry();
    if mem.used_pct >= threshold_pct.saturating_sub(8) {
        return 0;
    }

    let mut warmed = 0u16;
    let mut total_budget = 40usize * 1024 * 1024;
    for bin in bins.iter().take(5) {
        if total_budget == 0 {
            break;
        }
        let Some(path) = resolve_binary_path(bin) else {
            continue;
        };
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let len = file.metadata().map(|m| m.len() as usize).unwrap_or(0);
        if len == 0 {
            continue;
        }
        let slice = len.min(12 * 1024 * 1024).min(total_budget) as libc::off_t;
        if slice > 0 {
            unsafe {
                libc::posix_fadvise(file.as_raw_fd(), 0, slice, libc::POSIX_FADV_WILLNEED);
            }
            total_budget = total_budget.saturating_sub(slice as usize);
            warmed += 1;
        }
    }
    warmed
}

pub fn resolve_binary_path(bin: &str) -> Option<PathBuf> {
    let p = Path::new(bin);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|d| d.join(bin))
        .find(|cand| cand.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_and_self_rss() {
        let meminfo = "MemTotal:       16384000 kB\nMemFree:         2048000 kB\nMemAvailable:    8192000 kB\nCached:          4096000 kB\nSReclaimable:     512000 kB\n";
        let status = "Name:\tvelox-wm\nVmRSS:\t   98304 kB\n";
        let m = parse_meminfo_and_rss(meminfo, status);
        assert_eq!(m.total_mb, 16000);
        assert_eq!(m.available_mb, 8000);
        assert_eq!(m.cached_mb, 4000);
        assert_eq!(m.reclaimable_mb, 500);
        assert_eq!(m.used_pct, 50);
        assert_eq!(m.wm_rss_mb, 96);
    }

    #[test]
    fn top_frequent_bins_dedups_and_caps() {
        let mut lc = LaunchCache::default();
        lc.launch_counts.insert("alacritty".into(), 9);
        lc.launch_counts.insert("zen-browser".into(), 4);
        let tops = lc.top_frequent_bins(
            &[
                "alacritty".to_string(),
                "thunar".to_string(),
                "velox-settings".to_string(),
            ],
            3,
        );
        assert_eq!(tops, vec!["alacritty", "zen-browser", "thunar"]);
    }
}
