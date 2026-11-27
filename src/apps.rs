use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppCategory {
    All,
    Games,
    Internet,
    Communication,
    Media,
    Graphics,
    Dev,
    Office,
    System,
    Other,
}

impl Default for AppCategory {
    fn default() -> Self {
        Self::Other
    }
}

impl AppCategory {
    pub const TABS: &'static [AppCategory] = &[
        AppCategory::All,
        AppCategory::Games,
        AppCategory::Internet,
        AppCategory::Communication,
        AppCategory::Media,
        AppCategory::Graphics,
        AppCategory::Dev,
        AppCategory::Office,
        AppCategory::System,
        AppCategory::Other,
    ];

    pub const ASSIGNABLE: &'static [AppCategory] = &[
        AppCategory::Games,
        AppCategory::Internet,
        AppCategory::Communication,
        AppCategory::Media,
        AppCategory::Graphics,
        AppCategory::Dev,
        AppCategory::Office,
        AppCategory::System,
        AppCategory::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Games => "Games",
            Self::Internet => "Internet",
            Self::Communication => "Social",
            Self::Media => "Media",
            Self::Graphics => "Graphics",
            Self::Dev => "Dev",
            Self::Office => "Office",
            Self::System => "System",
            Self::Other => "Other",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "games" | "game" => Some(Self::Games),
            "internet" | "web" | "browser" => Some(Self::Internet),
            "social" | "communication" | "chat" | "messaging" => Some(Self::Communication),
            "media" | "audio" | "video" => Some(Self::Media),
            "graphics" | "design" | "art" => Some(Self::Graphics),
            "dev" | "development" | "coding" => Some(Self::Dev),
            "office" | "productivity" => Some(Self::Office),
            "system" | "utilities" | "settings" => Some(Self::System),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec_args: Vec<String>,
    pub terminal: bool,
    pub category: AppCategory,
    pub custom_category: Option<String>,
    pub badge_color: [u8; 3],
    pub keywords: Vec<String>,
    pub xdg_categories: String,
    pub is_custom: bool,
    pub hidden: bool,
    pub is_game_override: Option<bool>,
}

pub fn is_terminal_emulator_id(raw: &str) -> bool {
    let token = raw.trim().trim_matches('"');
    let base = Path::new(token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(token)
        .strip_suffix(".desktop")
        .unwrap_or(
            Path::new(token)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(token),
        )
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "alacritty"
            | "foot"
            | "footclient"
            | "kitty"
            | "wezterm"
            | "org.wezfurlong.wezterm"
            | "ghostty"
            | "com.mitchellh.ghostty"
            | "konsole"
            | "gnome-terminal"
            | "xfce4-terminal"
            | "xterm"
            | "urxvt"
            | "rxvt"
            | "st"
            | "rio"
            | "contour"
            | "blackbox"
    )
}

impl DesktopApp {
    pub fn is_terminal_app(&self) -> bool {
        self.terminal
            || is_terminal_emulator_id(&self.id)
            || self
                .exec_args
                .first()
                .is_some_and(|e| is_terminal_emulator_id(e))
    }

    pub fn effective_category_label(&self) -> &str {
        if let Some(ref cc) = self.custom_category {
            let trimmed = cc.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        self.category.label()
    }

    pub fn launch(
        &self,
        terminal_cmd: &str,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
    ) {
        if self.exec_args.is_empty() {
            return;
        }

        let already_terminal_bin = self
            .exec_args
            .first()
            .is_some_and(|e| is_terminal_emulator_id(e));
        let mut cmd = if self.terminal && !already_terminal_bin {
            let term_parts = parse_exec_args(terminal_cmd);
            let term_bin = term_parts
                .first()
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("alacritty");
            let mut c = Command::new(term_bin);
            if term_parts.len() > 1 {
                c.args(&term_parts[1..]);
            }
            c.arg("-e");
            c.args(&self.exec_args);
            c
        } else {
            let mut c = Command::new(&self.exec_args[0]);
            if self.exec_args.len() > 1 {
                c.args(&self.exec_args[1..]);
            }
            c
        };

        if let Some(wd) = wayland_display {
            cmd.env("WAYLAND_DISPLAY", wd);
        }
        if let Some(xd) = x11_display {
            cmd.env("DISPLAY", xd);
        }

        cmd.env("XDG_CURRENT_DESKTOP", "velox")
            .env("GDK_BACKEND", "wayland,x11")
            .env("QT_QPA_PLATFORM", "wayland;xcb")
            .env("SDL_VIDEODRIVER", "wayland,x11")
            .env("MOZ_ENABLE_WAYLAND", "1")
            .env("ELECTRON_OZONE_PLATFORM_HINT", "auto")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        if let Err(err) = cmd.spawn() {
            tracing::warn!(?err, app = %self.name, "Failed to launch desktop app");
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppOverrideEntry {
    pub id: String,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub exec_line: Option<String>,
    pub terminal: Option<bool>,
    pub category: Option<AppCategory>,
    pub custom_category: Option<String>,
    pub badge_color: Option<[u8; 3]>,
    pub keywords: Option<Vec<String>>,
    pub hidden: bool,
    pub is_game_override: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomAppEntry {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec_line: String,
    pub terminal: bool,
    pub category: AppCategory,
    pub custom_category: Option<String>,
    pub badge_color: [u8; 3],
    pub keywords: Vec<String>,
    pub is_game_override: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCategoryRule {
    pub category_name: String,
    pub keywords: Vec<String>,
    pub builtin_category: Option<AppCategory>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppLibraryStore {
    pub overrides: Vec<AppOverrideEntry>,
    pub custom_apps: Vec<CustomAppEntry>,
    pub category_rules: Vec<CustomCategoryRule>,
    pub extra_search_dirs: Vec<String>,
}

impl AppLibraryStore {
    pub fn store_path() -> Option<PathBuf> {
        Config::config_dir().map(|d| d.join("app_library.toml"))
    }

    pub fn modified_nanos() -> u128 {
        Self::store_path()
            .and_then(|p| fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    pub fn load_or_default() -> Self {
        let Some(path) = Self::store_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str::<Self>(&raw).unwrap_or_default()
    }

    pub fn save(&self) -> bool {
        let Some(dir) = Config::config_dir() else {
            return false;
        };
        if fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let path = dir.join("app_library.toml");
        let tmp = dir.join("app_library.toml.tmp");
        let Ok(serialized) = toml::to_string_pretty(self) else {
            return false;
        };
        if fs::write(&tmp, serialized).is_err() {
            return false;
        }
        fs::rename(&tmp, &path).is_ok()
    }

    pub fn upsert_override(&mut self, ov: AppOverrideEntry) {
        if let Some(pos) = self.overrides.iter().position(|e| e.id == ov.id) {
            self.overrides[pos] = ov;
        } else {
            self.overrides.push(ov);
        }
    }

    pub fn remove_override(&mut self, id: &str) {
        self.overrides.retain(|e| e.id != id);
    }

    pub fn upsert_custom_app(&mut self, app: CustomAppEntry) {
        if let Some(pos) = self.custom_apps.iter().position(|e| e.id == app.id) {
            self.custom_apps[pos] = app;
        } else {
            self.custom_apps.push(app);
        }
    }

    pub fn remove_custom_app(&mut self, id: &str) {
        self.custom_apps.retain(|e| e.id != id);
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AppIndexCache {
    dir_sig: String,
    apps: Vec<DesktopApp>,
}

fn cache_file_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("velox-wm").join("apps_cache_v2.json")
}

fn dir_signature(dirs: &[PathBuf]) -> String {
    let mut parts = Vec::new();
    for dir in dirs {
        let nanos = fs::metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        parts.push(format!("{}={nanos}", dir.display()));
    }
    parts.join("|")
}

fn app_search_dirs(extra: &[String]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let mut push_unique = |p: PathBuf| {
        if seen.insert(p.clone()) {
            dirs.push(p);
        }
    };

    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        push_unique(h.join(".local/share/applications"));
        push_unique(h.join(".local/share/flatpak/exports/share/applications"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        push_unique(PathBuf::from(data_home).join("applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for d in std::env::split_paths(&data_dirs) {
            push_unique(d.join("applications"));
        }
    }
    for extra_dir in extra {
        let trimmed = extra_dir.trim();
        if !trimmed.is_empty() {
            push_unique(PathBuf::from(trimmed));
        }
    }
    push_unique(PathBuf::from("/usr/local/share/applications"));
    push_unique(PathBuf::from("/usr/share/applications"));
    push_unique(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs
}

pub fn scan_desktop_apps() -> Vec<DesktopApp> {
    let store = AppLibraryStore::load_or_default();
    let dirs = app_search_dirs(&store.extra_search_dirs);
    let sig = dir_signature(&dirs);
    let cache_path = cache_file_path();

    let raw_apps = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<AppIndexCache>(&raw).ok())
        .filter(|cache| cache.dir_sig == sig && !cache.apps.is_empty())
        .map(|cache| cache.apps)
        .unwrap_or_else(|| {
            let scanned = scan_desktop_apps_uncached(&dirs);
            if let Some(parent) = cache_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(&AppIndexCache {
                dir_sig: sig,
                apps: scanned.clone(),
            }) {
                let _ = fs::write(&cache_path, json);
            }
            scanned
        });

    merge_with_store(raw_apps, &store)
}

/// Scans all `.desktop` entries bypassing the raw index cache, then merges `app_library.toml` overrides.
pub fn scan_desktop_apps_including_hidden() -> Vec<DesktopApp> {
    let store = AppLibraryStore::load_or_default();
    let dirs = app_search_dirs(&store.extra_search_dirs);
    let raw = scan_desktop_apps_uncached(&dirs);
    merge_with_store(raw, &store)
}

pub fn merge_with_store(mut base_apps: Vec<DesktopApp>, store: &AppLibraryStore) -> Vec<DesktopApp> {
    for app in &mut base_apps {
        // Re-apply smart category rules & user-defined keyword rules cleanly.
        let (detected_cat, detected_custom) = categorize_smart(
            &app.id,
            &app.name,
            &app.comment,
            &app.exec_args,
            &app.xdg_categories,
            &app.keywords,
            &store.category_rules,
        );
        app.category = detected_cat;
        app.custom_category = detected_custom;
        app.badge_color = badge_color_for_name(&app.name, app.category);

        if let Some(ov) = store.overrides.iter().find(|o| o.id == app.id) {
            if let Some(ref name) = ov.name {
                if !name.trim().is_empty() {
                    app.name = name.trim().to_string();
                }
            }
            if let Some(ref comment) = ov.comment {
                app.comment = comment.clone();
            }
            if let Some(ref exec_line) = ov.exec_line {
                let args = parse_exec_args(exec_line);
                if !args.is_empty() {
                    app.exec_args = args;
                }
            }
            if let Some(term) = ov.terminal {
                app.terminal = term;
            }
            if let Some(cat) = ov.category {
                app.category = cat;
            }
            if let Some(ref cc) = ov.custom_category {
                let trimmed = cc.trim();
                app.custom_category = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            if let Some(col) = ov.badge_color {
                app.badge_color = col;
            }
            if let Some(ref kws) = ov.keywords {
                app.keywords = kws.clone();
            }
            app.hidden = ov.hidden;
            app.is_game_override = ov.is_game_override;
        }
    }

    for c in &store.custom_apps {
        let exec_args = parse_exec_args(&c.exec_line);
        if exec_args.is_empty() || c.name.trim().is_empty() {
            continue;
        }
        let custom_category = c
            .custom_category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let id = if c.id.trim().is_empty() {
            format!("custom:{}.desktop", c.name.to_ascii_lowercase().replace(' ', "-"))
        } else {
            c.id.clone()
        };
        base_apps.retain(|a| a.id != id);
        base_apps.push(DesktopApp {
            id,
            name: c.name.trim().to_string(),
            comment: c.comment.trim().to_string(),
            exec_args,
            terminal: c.terminal,
            category: c.category,
            custom_category,
            badge_color: if c.badge_color == [0, 0, 0] {
                badge_color_for_name(&c.name, c.category)
            } else {
                c.badge_color
            },
            keywords: c.keywords.clone(),
            xdg_categories: String::new(),
            is_custom: true,
            hidden: false,
            is_game_override: c.is_game_override,
        });
    }

    base_apps.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    base_apps
}

fn scan_desktop_apps_uncached(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut apps_by_id: HashMap<String, DesktopApp> = HashMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            if apps_by_id.contains_key(&id) {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if let Some(app) = parse_desktop_entry(&id, &content) {
                    apps_by_id.insert(id, app);
                }
            }
        }
    }

    let mut apps: Vec<DesktopApp> = apps_by_id.into_values().collect();
    apps.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    apps
}

pub fn parse_desktop_entry(id: &str, content: &str) -> Option<DesktopApp> {
    let mut in_desktop_entry = false;
    let mut fields: HashMap<&str, &str> = HashMap::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            if !key.contains('[') {
                fields.entry(key).or_insert(v.trim());
            }
        }
    }

    if fields.get("Type").copied() != Some("Application") {
        return None;
    }
    if fields.get("NoDisplay").copied() == Some("true")
        || fields.get("Hidden").copied() == Some("true")
    {
        return None;
    }
    if let Some(only) = fields.get("OnlyShowIn") {
        let allowed = only
            .split(';')
            .any(|d| matches!(d.trim(), "velox" | "XFCE" | "GNOME" | "KDE"));
        if !allowed {
            return None;
        }
    }
    if let Some(try_exec) = fields.get("TryExec") {
        if !executable_exists(try_exec) {
            return None;
        }
    }

    let name = fields.get("Name")?.to_string();
    let exec_raw = fields.get("Exec")?;
    let exec_args = parse_exec_args(exec_raw);
    if exec_args.is_empty() || !executable_exists(&exec_args[0]) {
        return None;
    }

    let comment = fields
        .get("Comment")
        .or_else(|| fields.get("GenericName"))
        .copied()
        .unwrap_or("")
        .to_string();
    let terminal = fields.get("Terminal").copied() == Some("true");
    let xdg_categories = fields.get("Categories").copied().unwrap_or("").to_string();
    let keywords: Vec<String> = fields
        .get("Keywords")
        .copied()
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect();
    let (category, custom_category) =
        categorize_smart(id, &name, &comment, &exec_args, &xdg_categories, &keywords, &[]);
    let badge_color = badge_color_for_name(&name, category);

    Some(DesktopApp {
        id: id.to_string(),
        name,
        comment,
        exec_args,
        terminal,
        category,
        custom_category,
        badge_color,
        keywords,
        xdg_categories,
        is_custom: false,
        hidden: false,
        is_game_override: None,
    })
}

/// Parses an XDG `Exec=` line, unquoting arguments and removing `%f/%u/%i/%c/%k` placeholders.
pub fn parse_exec_args(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    let mut cleaned = Vec::new();
    for tok in tokens {
        if tok == "%i" {
            continue;
        }
        if matches!(
            tok.as_str(),
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%c" | "%k" | "%v" | "%m"
        ) {
            continue;
        }
        let stripped = tok
            .replace("%f", "")
            .replace("%F", "")
            .replace("%u", "")
            .replace("%U", "");
        if !stripped.is_empty() {
            cleaned.push(stripped);
        }
    }
    cleaned
}

pub fn executable_exists(bin: &str) -> bool {
    let p = Path::new(bin);
    if p.is_absolute() {
        return p.exists();
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(bin).is_file())
}

/// Smart category classifier using XDG categories, executable/ID/keyword heuristics, and user rules.
pub fn categorize_smart(
    id: &str,
    name: &str,
    comment: &str,
    exec_args: &[String],
    xdg_categories: &str,
    keywords: &[String],
    custom_rules: &[CustomCategoryRule],
) -> (AppCategory, Option<String>) {
    let exec_str = exec_args.join(" ").to_ascii_lowercase();
    let cats_l = xdg_categories.to_ascii_lowercase();
    let full_l = format!(
        "{id} {name} {comment} {exec_str} {cats_l} {}",
        keywords.join(" ")
    )
    .to_ascii_lowercase();

    // 1. User-defined custom category rules take top precedence.
    for rule in custom_rules {
        if rule.category_name.trim().is_empty() {
            continue;
        }
        let matched = rule.keywords.iter().any(|kw| {
            let k = kw.trim().to_ascii_lowercase();
            !k.is_empty() && full_l.contains(&k)
        });
        if matched {
            let builtin = rule
                .builtin_category
                .or_else(|| AppCategory::from_label(&rule.category_name))
                .unwrap_or(AppCategory::Other);
            let custom_lbl = if AppCategory::from_label(&rule.category_name).is_some() {
                None
            } else {
                Some(rule.category_name.trim().to_string())
            };
            return (builtin, custom_lbl);
        }
    }

    // 2. Games detection (handles Steam URLs, Proton, Wine, Lutris, Heroic, RetroArch, emulators, etc.)
    let game_hits = [
        "game",
        "emulator",
        "arcade",
        "strategygame",
        "simulation",
        "roleplaying",
        "shooter",
        "racinggame",
        "actiongame",
        "adventuregame",
        "steam://rungameid",
        "steam_app_",
        "lutris",
        "heroic",
        "legendary",
        "bottles",
        "prismlauncher",
        "multimc",
        "retroarch",
        "rpcs3",
        "pcsx2",
        "dolphin-emu",
        "ppsspp",
        "duckstation",
        "yuzu",
        "ryujinx",
        "cemu",
        "gamescope",
        "gamemoderun",
        "mangohud",
        "proton",
        "minecraft",
        "transportfever",
        "cities",
        "factorio",
        "satisfactory",
        "itch.io",
        ".exe",
    ];
    if game_hits.iter().any(|g| full_l.contains(g)) || exec_str.contains("/steam/") {
        return (AppCategory::Games, None);
    }

    // 3. Communication / Social
    if contains_any_word(
        &full_l,
        &[
            "instantmessaging",
            "chat",
            "videoconference",
            "ircclient",
            "discord",
            "vesktop",
            "telegram",
            "signal",
            "slack",
            "element",
            "cinny",
            "whatsapp",
            "teams",
            "thunderbird",
            "betterbird",
            "zoom",
        ],
    ) {
        return (AppCategory::Communication, None);
    }

    // 4. Graphics & Creative
    if contains_any_word(
        &cats_l,
        &[
            "graphics",
            "2dgraphics",
            "3dgraphics",
            "rastergraphics",
            "vectorgraphics",
            "photography",
        ],
    ) || contains_any_word(
        &full_l,
        &[
            "gimp",
            "krita",
            "inkscape",
            "blender",
            "darktable",
            "rawtherapee",
            "aseprite",
            "kdenlive",
            "shotcut",
            "figma",
        ],
    ) {
        return (AppCategory::Graphics, None);
    }

    // 5. Media (Audio / Video)
    if contains_any_word(&cats_l, &["audiovideo", "audio", "video", "music", "player"])
        || contains_any_word(
            &full_l,
            &[
                "obs",
                "vlc",
                "mpv",
                "spotify",
                "rhythmbox",
                "audacity",
                "ardour",
                "celluloid",
                "haruna",
                "stremio",
                "kodi",
                "pavucontrol",
                "helvum",
            ],
        )
    {
        return (AppCategory::Media, None);
    }

    // 6. Development
    if contains_any_word(&cats_l, &["development", "ide", "debugger", "building"])
        || contains_any_word(
            &full_l,
            &[
                "vscode",
                "vscodium",
                "code ",
                "/code",
                "zed",
                "neovim",
                "nvim",
                "helix",
                "sublime",
                "intellij",
                "pycharm",
                "clion",
                "rustrover",
                "android-studio",
                "gitkraken",
                "godot",
            ],
        )
    {
        return (AppCategory::Dev, None);
    }

    // 7. Internet
    if contains_any_word(&cats_l, &["network", "webbrowser", "email"])
        || contains_any_word(
            &full_l,
            &[
                "firefox",
                "zen-browser",
                "chromium",
                "google-chrome",
                "brave",
                "vivaldi",
                "librewolf",
                "qbittorrent",
                "transmission",
            ],
        )
    {
        return (AppCategory::Internet, None);
    }

    // 8. Office / Productivity
    if contains_any_word(
        &cats_l,
        &["office", "wordprocessor", "spreadsheet", "presentation", "education"],
    ) || contains_any_word(
        &full_l,
        &[
            "libreoffice",
            "onlyoffice",
            "obsidian",
            "logseq",
            "zotero",
            "calibre",
            "okular",
            "evince",
        ],
    ) {
        return (AppCategory::Office, None);
    }

    // 9. System / Utilities / Terminals / File Managers
    if contains_any_word(
        &cats_l,
        &["system", "settings", "utility", "filemanager", "terminalemulator", "hardwaresettings"],
    ) || contains_any_word(
        &full_l,
        &[
            "alacritty",
            "kitty",
            "wezterm",
            "foot",
            "ghostty",
            "konsole",
            "thunar",
            "dolphin",
            "nautilus",
            "btop",
            "htop",
            "gparted",
            "velox-settings",
        ],
    ) {
        return (AppCategory::System, None);
    }

    (AppCategory::Other, None)
}

fn contains_any_word(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

pub fn badge_color_for_name(name: &str, cat: AppCategory) -> [u8; 3] {
    match cat {
        AppCategory::Games => [242, 142, 54],
        AppCategory::Internet => [56, 152, 236],
        AppCategory::Communication => [116, 118, 246],
        AppCategory::Media => [232, 92, 138],
        AppCategory::Graphics => [216, 112, 232],
        AppCategory::Dev => [138, 102, 242],
        AppCategory::Office => [44, 186, 146],
        AppCategory::System => [92, 148, 224],
        _ => {
            let hash = name.bytes().fold(0u8, |acc, b| acc.wrapping_add(b));
            match hash % 4 {
                0 => [74, 142, 245],
                1 => [124, 98, 238],
                2 => [48, 192, 156],
                _ => [226, 134, 68],
            }
        }
    }
}

pub fn generate_custom_id(name: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let slug: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("custom-{slug}-{secs}.desktop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_exec_and_filters_hidden() {
        let hidden = "[Desktop Entry]\nType=Application\nName=Hidden\nExec=sh\nNoDisplay=true\n";
        assert!(parse_desktop_entry("hidden.desktop", hidden).is_none());

        let valid = "[Desktop Entry]\nType=Application\nName=Shell Test\nComment=Run shell\nExec=\"/bin/sh\" -c \"echo hi\" %U\nCategories=System;Utility;\nTerminal=false\n";
        let app = parse_desktop_entry("sh.desktop", valid).expect("valid app");
        assert_eq!(app.name, "Shell Test");
        assert_eq!(app.category, AppCategory::System);
        assert_eq!(app.exec_args, vec!["/bin/sh", "-c", "echo hi"]);
    }

    #[test]
    fn ignores_desktop_actions_section() {
        let entry = "[Desktop Entry]\nType=Application\nName=Main\nExec=/bin/sh\n[Desktop Action New]\nName=Other\nExec=/bin/false\n";
        let app = parse_desktop_entry("main.desktop", entry).expect("main entry");
        assert_eq!(app.name, "Main");
        assert_eq!(app.exec_args, vec!["/bin/sh"]);
    }

    #[test]
    fn detects_steam_and_proton_games_and_custom_rules() {
        let (cat_steam, _) = categorize_smart(
            "steam_app_1066780.desktop",
            "Transport Fever 2",
            "Launch via Steam",
            &["/bin/sh".into(), "steam://rungameid/1066780".into()],
            "Utility;",
            &[],
            &[],
        );
        assert_eq!(cat_steam, AppCategory::Games);

        let rules = vec![CustomCategoryRule {
            category_name: "Simulators".into(),
            keywords: vec!["train".into(), "transport".into()],
            builtin_category: Some(AppCategory::Games),
        }];
        let (cat_rule, custom) = categorize_smart(
            "tf2.desktop",
            "Train Sandbox",
            "",
            &["/bin/true".into()],
            "",
            &[],
            &rules,
        );
        assert_eq!(cat_rule, AppCategory::Games);
        assert_eq!(custom.as_deref(), Some("Simulators"));
    }

    #[test]
    fn store_overrides_and_custom_apps_merge_cleanly() {
        let base = vec![DesktopApp {
            id: "foo.desktop".into(),
            name: "Foo".into(),
            comment: "Old".into(),
            exec_args: vec!["/bin/sh".into()],
            category: AppCategory::Other,
            ..DesktopApp::default()
        }];
        let mut store = AppLibraryStore::default();
        store.upsert_override(AppOverrideEntry {
            id: "foo.desktop".into(),
            name: Some("Foo Edited".into()),
            comment: Some("Edited comment".into()),
            exec_line: Some("/bin/sh -c true".into()),
            terminal: Some(true),
            category: Some(AppCategory::Dev),
            custom_category: Some("Workspace Tools".into()),
            badge_color: Some([10, 200, 120]),
            keywords: Some(vec!["sandbox".into()]),
            hidden: false,
            is_game_override: Some(false),
        });
        store.upsert_custom_app(CustomAppEntry {
            id: "custom-demo.desktop".into(),
            name: "Custom App".into(),
            comment: "User added".into(),
            exec_line: "/bin/sh".into(),
            terminal: false,
            category: AppCategory::Games,
            custom_category: Some("My Games".into()),
            badge_color: [240, 140, 40],
            keywords: vec!["rpg".into()],
            is_game_override: Some(true),
        });

        let merged = merge_with_store(base, &store);
        assert_eq!(merged.len(), 2);
        let edited = merged.iter().find(|a| a.id == "foo.desktop").unwrap();
        assert_eq!(edited.name, "Foo Edited");
        assert_eq!(edited.comment, "Edited comment");
        assert_eq!(edited.exec_args, vec!["/bin/sh", "-c", "true"]);
        assert!(edited.terminal);
        assert_eq!(edited.category, AppCategory::Dev);
        assert_eq!(edited.custom_category.as_deref(), Some("Workspace Tools"));
        assert_eq!(edited.badge_color, [10, 200, 120]);
        let custom = merged.iter().find(|a| a.id == "custom-demo.desktop").unwrap();
        assert!(custom.is_custom);
        assert_eq!(custom.is_game_override, Some(true));
    }
}
