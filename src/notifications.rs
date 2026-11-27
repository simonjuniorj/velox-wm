use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const ORGANIZER_SILENCEABLE_CATEGORIES: &[&str] =
    &["Promotions", "News", "Social", "Suggested"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationUrgency {
    Low,
    Normal,
    Critical,
}

impl Default for NotificationUrgency {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotificationCategory {
    Conversations,
    Promotions,
    News,
    Social,
    Suggested,
    Updates,
    System,
}

impl Default for NotificationCategory {
    fn default() -> Self {
        Self::System
    }
}

impl NotificationCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conversations => "Conversations",
            Self::Promotions => "Promotions",
            Self::News => "News",
            Self::Social => "Social",
            Self::Suggested => "Suggested",
            Self::Updates => "Updates",
            Self::System => "System",
        }
    }

    pub fn short_tag(self) -> &'static str {
        match self {
            Self::Conversations => "Chat",
            Self::Promotions => "Promo",
            Self::News => "News",
            Self::Social => "Social",
            Self::Suggested => "Suggest",
            Self::Updates => "Update",
            Self::System => "System",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "conversations" | "conversation" | "chat" => Some(Self::Conversations),
            "promotions" | "promo" => Some(Self::Promotions),
            "news" => Some(Self::News),
            "social" => Some(Self::Social),
            "suggested" | "suggest" => Some(Self::Suggested),
            "updates" | "update" => Some(Self::Updates),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestedActionKind {
    QuickReply,
    CopyCode,
    OpenUrl,
    MarkRead,
}

impl Default for SuggestedActionKind {
    fn default() -> Self {
        Self::QuickReply
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestedAction {
    pub label: String,
    pub payload: String,
    pub kind: SuggestedActionKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationItem {
    pub id: u32,
    pub app_name: String,
    pub desktop_entry: String,
    pub raw_category_hint: String,
    pub summary: String,
    pub body: String,
    pub urgency: NotificationUrgency,
    pub actions: Vec<NotificationAction>,
    pub timestamp_epoch: u64,
    pub expire_at_epoch: u64,
    pub read: bool,
    pub is_update_notice: bool,
    pub category: NotificationCategory,
    pub conversation_key: Option<String>,
    pub is_conversation: bool,
    pub is_priority: bool,
    pub is_silent: bool,
    pub arrived_in_cooldown: bool,
    pub summary_recap: Option<String>,
    pub suggested_actions: Vec<SuggestedAction>,
    pub snooze_until_epoch: u64,
    pub supports_inline_reply: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationHistoryEntry {
    pub id: u32,
    pub app_name: String,
    pub app_key: String,
    pub category: NotificationCategory,
    pub summary: String,
    pub body_preview: String,
    pub summary_recap: Option<String>,
    pub timestamp_epoch: u64,
    pub conversation_key: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationGroupSummary {
    pub key: String,
    pub app_name: String,
    pub title: String,
    pub count: usize,
    pub unread_count: usize,
    pub is_priority: bool,
    pub recap: String,
    pub latest_epoch: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationSnapshot {
    pub version: u64,
    pub dbus_active: bool,
    pub items: Vec<NotificationItem>,
    pub snoozed: Vec<NotificationItem>,
    pub history: Vec<NotificationHistoryEntry>,
    pub seen_apps: Vec<String>,
    pub unread_count: usize,
    pub status_bar_unread_count: usize,
    pub quiet_unread_count: usize,
    pub cooldown_active: bool,
    pub cooldown_remaining_secs: u64,
    pub cooldown_until_epoch: u64,
    pub cooldown_received_count: usize,
    pub conversation_groups: Vec<ConversationGroupSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PersistedNotificationStore {
    history: Vec<NotificationHistoryEntry>,
    seen_apps: Vec<String>,
}

struct NotificationEngineState {
    config: Config,
    snapshot: NotificationSnapshot,
    next_internal_id: u32,
    bridge_stdin: Option<Arc<Mutex<ChildStdin>>>,
    recent_burst_epochs: Vec<u64>,
    cooldown_started_epoch: u64,
    persist_to_disk: bool,
}

/// D-Bus notification receiver + organizer + snooze/cooldown + Pixel-OS notification center state.
pub struct NotificationService {
    inner: Arc<Mutex<NotificationEngineState>>,
    stop: Arc<AtomicBool>,
    child_proc: Arc<Mutex<Option<Child>>>,
}

const DBUS_NOTIFY_BRIDGE_PY: &str = r#"
import json
import sys
import threading

try:
    import gi
    gi.require_version("Gio", "2.0")
    gi.require_version("GLib", "2.0")
    from gi.repository import Gio, GLib
except Exception as exc:
    sys.stderr.write(f"dbus-bridge-import-err: {exc}\n")
    sys.stderr.flush()
    sys.exit(0)

IFACE_XML = """
<node>
  <interface name='org.freedesktop.Notifications'>
    <method name='GetCapabilities'>
      <arg type='as' name='capabilities' direction='out'/>
    </method>
    <method name='Notify'>
      <arg type='s' name='app_name' direction='in'/>
      <arg type='u' name='replaces_id' direction='in'/>
      <arg type='s' name='app_icon' direction='in'/>
      <arg type='s' name='summary' direction='in'/>
      <arg type='s' name='body' direction='in'/>
      <arg type='as' name='actions' direction='in'/>
      <arg type='a{sv}' name='hints' direction='in'/>
      <arg type='i' name='expire_timeout' direction='in'/>
      <arg type='u' name='id' direction='out'/>
    </method>
    <method name='CloseNotification'>
      <arg type='u' name='id' direction='in'/>
    </method>
    <method name='GetServerInformation'>
      <arg type='s' name='name' direction='out'/>
      <arg type='s' name='vendor' direction='out'/>
      <arg type='s' name='version' direction='out'/>
      <arg type='s' name='spec_version' direction='out'/>
    </method>
    <signal name='NotificationClosed'>
      <arg type='u' name='id'/>
      <arg type='u' name='reason'/>
    </signal>
    <signal name='ActionInvoked'>
      <arg type='u' name='id'/>
      <arg type='s' name='action_key'/>
    </signal>
    <signal name='NotificationReplied'>
      <arg type='u' name='id'/>
      <arg type='s' name='text'/>
    </signal>
  </interface>
</node>
"""

class NotifServer:
    def __init__(self):
        self.next_id = 100
        self.loop = GLib.MainLoop()
        self.node_info = Gio.DBusNodeInfo.new_for_xml(IFACE_XML)
        self.conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.obj_id = self.conn.register_object(
            "/org/freedesktop/Notifications",
            self.node_info.interfaces[0],
            self.on_method,
            None,
            None,
        )
        Gio.bus_own_name_on_connection(
            self.conn,
            "org.freedesktop.Notifications",
            Gio.BusNameOwnerFlags.ALLOW_REPLACEMENT | Gio.BusNameOwnerFlags.REPLACE,
            None,
            None,
        )
        t = threading.Thread(target=self.read_commands, daemon=True)
        t.start()

    def emit_json(self, obj):
        sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
        sys.stdout.flush()

    def on_method(self, conn, sender, path, iface, method, params, invocation):
        if method == "GetCapabilities":
            invocation.return_value(
                GLib.Variant("(as)", (["body", "actions", "persistence", "icon-static", "inline-reply"],))
            )
            return
        if method == "GetServerInformation":
            invocation.return_value(
                GLib.Variant("(ssss)", ("velox-wm", "Velox", "0.6.0", "1.2"))
            )
            return
        if method == "CloseNotification":
            nid = params.unpack()[0]
            self.emit_json({"ev": "close", "id": int(nid)})
            invocation.return_value(None)
            return
        if method == "Notify":
            app_name, replaces_id, _icon, summary, body, actions, hints, timeout = params.unpack()
            nid = int(replaces_id) if replaces_id > 0 else self.next_id
            if replaces_id == 0:
                self.next_id += 1
            urgency = 1
            category = ""
            desktop_entry = ""
            try:
                if "urgency" in hints:
                    urgency = int(hints["urgency"])
                if "category" in hints:
                    category = str(hints["category"])
                if "desktop-entry" in hints:
                    desktop_entry = str(hints["desktop-entry"])
            except Exception:
                pass
            act_list = []
            for i in range(0, len(actions) - 1, 2):
                act_list.append({"key": str(actions[i]), "label": str(actions[i + 1])})
            self.emit_json({
                "ev": "notify",
                "id": nid,
                "app": str(app_name or "App"),
                "desktop_entry": desktop_entry,
                "category": category,
                "summary": str(summary or ""),
                "body": str(body or ""),
                "actions": act_list,
                "urgency": urgency,
                "timeout_ms": int(timeout),
            })
            invocation.return_value(GLib.Variant("(u)", (nid,)))
            return
        invocation.return_value(None)

    def read_commands(self):
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                cmd = json.loads(line)
                c = cmd.get("cmd")
                if c == "action":
                    self.conn.emit_signal(
                        None,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "ActionInvoked",
                        GLib.Variant("(us)", (int(cmd["id"]), str(cmd["key"]))),
                    )
                elif c == "reply":
                    self.conn.emit_signal(
                        None,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "NotificationReplied",
                        GLib.Variant("(us)", (int(cmd["id"]), str(cmd["text"]))),
                    )
                elif c == "close":
                    self.conn.emit_signal(
                        None,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "NotificationClosed",
                        GLib.Variant("(uu)", (int(cmd["id"]), int(cmd.get("reason", 2)))),
                    )
            except Exception:
                pass

    def run(self):
        self.loop.run()

if __name__ == "__main__":
    try:
        NotifServer().run()
    except Exception:
        pass
"#;

impl NotificationService {
    pub fn start(connect_dbus: bool) -> Arc<Self> {
        let mut initial_snap = NotificationSnapshot::default();
        if connect_dbus {
            let store = load_persisted_store();
            initial_snap.history = store.history;
            initial_snap.seen_apps = store.seen_apps;
        }
        let inner = Arc::new(Mutex::new(NotificationEngineState {
            config: Config::default(),
            snapshot: initial_snap,
            next_internal_id: 900_000,
            bridge_stdin: None,
            recent_burst_epochs: Vec::new(),
            cooldown_started_epoch: 0,
            persist_to_disk: connect_dbus,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let child_proc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));

        if connect_dbus {
            let inner_bg = Arc::clone(&inner);
            let stop_bg = Arc::clone(&stop);
            let child_bg = Arc::clone(&child_proc);
            thread::Builder::new()
                .name("velox-notify-dbus".to_string())
                .spawn(move || {
                    while !stop_bg.load(Ordering::Relaxed) {
                        let Ok(mut child) = Command::new("python3")
                            .args(["-u", "-c", DBUS_NOTIFY_BRIDGE_PY])
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .spawn()
                        else {
                            thread::sleep(Duration::from_secs(5));
                            continue;
                        };
                        let stdin_handle = child.stdin.take().map(|s| Arc::new(Mutex::new(s)));
                        let stdout_handle = child.stdout.take();
                        {
                            if let Ok(mut guard) = inner_bg.lock() {
                                guard.bridge_stdin = stdin_handle.clone();
                                guard.snapshot.dbus_active = true;
                                guard.snapshot.version = guard.snapshot.version.wrapping_add(1);
                            }
                        }
                        {
                            if let Ok(mut cguard) = child_bg.lock() {
                                *cguard = Some(child);
                            }
                        }

                        if let Some(stdout) = stdout_handle {
                            let reader = BufReader::new(stdout);
                            for line in reader.lines().map_while(Result::ok) {
                                if stop_bg.load(Ordering::Relaxed) {
                                    break;
                                }
                                let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                                else {
                                    continue;
                                };
                                apply_dbus_event(&inner_bg, val);
                            }
                        }
                        if let Ok(mut guard) = inner_bg.lock() {
                            guard.bridge_stdin = None;
                            guard.snapshot.dbus_active = false;
                            guard.snapshot.version = guard.snapshot.version.wrapping_add(1);
                        }
                        if let Ok(mut cguard) = child_bg.lock() {
                            if let Some(mut c) = cguard.take() {
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                        }
                        if !stop_bg.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_secs(2));
                        }
                    }
                })
                .ok();
        }

        Arc::new(Self {
            inner,
            stop,
            child_proc,
        })
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.child_proc.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    pub fn snapshot(&self) -> NotificationSnapshot {
        self.inner
            .lock()
            .map(|g| g.snapshot.clone())
            .unwrap_or_default()
    }

    pub fn snapshot_if_changed(&self, seen_version: u64) -> Option<NotificationSnapshot> {
        let guard = self.inner.lock().ok()?;
        if guard.snapshot.version != seen_version {
            Some(guard.snapshot.clone())
        } else {
            None
        }
    }

    pub fn apply_config(&self, config: &Config) {
        if let Ok(mut state) = self.inner.lock() {
            state.config = config.clone();
            let now = unix_now_secs();
            reclassify_all_items(&mut state, now);
        }
    }

    pub fn tick(&self, config: &Config) {
        self.tick_at(config, unix_now_secs());
    }

    pub fn tick_at(&self, config: &Config, now: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.config = config.clone();
            let mut changed = false;

            if state.snapshot.cooldown_until_epoch > 0 && now >= state.snapshot.cooldown_until_epoch
            {
                state.snapshot.cooldown_until_epoch = 0;
                state.cooldown_started_epoch = 0;
                state.snapshot.cooldown_received_count = 0;
                state.snapshot.cooldown_active = false;
                state.snapshot.cooldown_remaining_secs = 0;
                changed = true;
            } else if state.snapshot.cooldown_until_epoch > now {
                let rem = state.snapshot.cooldown_until_epoch - now;
                if state.snapshot.cooldown_remaining_secs != rem {
                    state.snapshot.cooldown_remaining_secs = rem;
                    changed = true;
                }
            }

            // Resurface any due snoozed notifications cleanly.
            let mut ready = Vec::new();
            let mut still_snoozed = Vec::new();
            for mut item in state.snapshot.snoozed.drain(..) {
                if item.snooze_until_epoch <= now {
                    item.snooze_until_epoch = 0;
                    item.read = false;
                    item.timestamp_epoch = now;
                    item.expire_at_epoch = now + 7;
                    ready.push(item);
                    changed = true;
                } else {
                    still_snoozed.push(item);
                }
            }
            state.snapshot.snoozed = still_snoozed;
            for item in ready.into_iter().rev() {
                state.snapshot.items.retain(|x| x.id != item.id);
                state.snapshot.items.insert(0, item);
            }

            if changed {
                recompute_and_bump(&mut state, now);
            }
        }
    }

    pub fn restore_items(&self, items: Vec<NotificationItem>) {
        if let Ok(mut state) = self.inner.lock() {
            let max_internal = items
                .iter()
                .map(|n| n.id)
                .filter(|&id| id >= 900_000)
                .max()
                .unwrap_or(899_999);
            state.next_internal_id = state.next_internal_id.max(max_internal + 1);
            state.snapshot.items = items;
            let now = unix_now_secs();
            reclassify_all_items(&mut state, now);
        }
    }

    pub fn push_internal(
        &self,
        app_name: &str,
        summary: &str,
        body: &str,
        urgency: NotificationUrgency,
        actions: Vec<NotificationAction>,
        is_update_notice: bool,
        timeout_ms: i32,
    ) -> u32 {
        self.push_internal_at(
            app_name,
            summary,
            body,
            urgency,
            actions,
            is_update_notice,
            timeout_ms,
            unix_now_secs(),
        )
    }

    pub fn push_internal_at(
        &self,
        app_name: &str,
        summary: &str,
        body: &str,
        urgency: NotificationUrgency,
        actions: Vec<NotificationAction>,
        is_update_notice: bool,
        timeout_ms: i32,
        now: u64,
    ) -> u32 {
        let Ok(mut state) = self.inner.lock() else {
            return 0;
        };
        let id = state.next_internal_id;
        state.next_internal_id = state.next_internal_id.saturating_add(1);
        let raw_item = NotificationItem {
            id,
            app_name: app_name.to_string(),
            desktop_entry: normalize_app_key(app_name),
            raw_category_hint: if is_update_notice {
                "system.update".to_string()
            } else {
                String::new()
            },
            summary: summary.to_string(),
            body: body.to_string(),
            urgency,
            actions,
            timestamp_epoch: now,
            expire_at_epoch: compute_expire_epoch(now, timeout_ms),
            read: false,
            is_update_notice,
            supports_inline_reply: false,
            ..NotificationItem::default()
        };
        ingest_item(&mut state, raw_item, now, true);
        id
    }

    pub fn mark_reloaded(&self, new_version: &str, restored_count: usize) {
        if let Ok(mut state) = self.inner.lock() {
            for item in &mut state.snapshot.items {
                if item.is_update_notice {
                    item.summary = format!("Velox {new_version} Reloaded Successfully");
                    item.body = if restored_count > 0 {
                        format!(
                            "Updated cleanly. Restored {restored_count} window layout(s); resume open work manually."
                        )
                    } else {
                        "Compositor reloaded seamlessly with the new version.".to_string()
                    };
                    item.actions.clear();
                    item.read = false;
                    item.expire_at_epoch = unix_now_secs() + 8;
                    recompute_and_bump(&mut state, unix_now_secs());
                    return;
                }
            }
        }
        let body = if restored_count > 0 {
            format!(
                "Velox {new_version} active. Restored {restored_count} window layout(s); resume apps manually."
            )
        } else {
            format!("Velox {new_version} reloaded cleanly.")
        };
        self.push_internal(
            "Velox Updater",
            "Live Reload Complete",
            &body,
            NotificationUrgency::Normal,
            vec![],
            true,
            7000,
        );
    }

    pub fn mark_all_read(&self) {
        if let Ok(mut state) = self.inner.lock() {
            for item in &mut state.snapshot.items {
                item.read = true;
            }
            recompute_and_bump(&mut state, unix_now_secs());
        }
    }

    pub fn mark_read(&self, id: u32) {
        if let Ok(mut state) = self.inner.lock() {
            if let Some(item) = state.snapshot.items.iter_mut().find(|n| n.id == id) {
                item.read = true;
            }
            recompute_and_bump(&mut state, unix_now_secs());
        }
    }

    pub fn dismiss(&self, id: u32) {
        let stdin_ref = if let Ok(mut state) = self.inner.lock() {
            if let Some(pos) = state.snapshot.items.iter().position(|x| x.id == id) {
                let item = state.snapshot.items.remove(pos);
                update_history_status(&mut state, item.id, "Dismissed");
            } else if let Some(pos) = state.snapshot.snoozed.iter().position(|x| x.id == id) {
                let item = state.snapshot.snoozed.remove(pos);
                update_history_status(&mut state, item.id, "Dismissed");
            }
            recompute_and_bump(&mut state, unix_now_secs());
            state.bridge_stdin.clone()
        } else {
            None
        };
        if let Some(stdin) = stdin_ref {
            send_bridge_cmd(
                &stdin,
                serde_json::json!({ "cmd": "close", "id": id, "reason": 2 }),
            );
        }
    }

    pub fn snooze(&self, id: u32, duration_secs: u64) {
        self.snooze_at(id, duration_secs, unix_now_secs());
    }

    pub fn snooze_at(&self, id: u32, duration_secs: u64, now: u64) {
        if let Ok(mut state) = self.inner.lock() {
            if !state.config.allow_notification_snooze {
                return;
            }
            if let Some(pos) = state.snapshot.items.iter().position(|x| x.id == id) {
                let mut item = state.snapshot.items.remove(pos);
                item.snooze_until_epoch = now + duration_secs.max(15);
                item.read = true;
                update_history_status(&mut state, item.id, "Snoozed");
                state.snapshot.snoozed.insert(0, item);
                recompute_and_bump(&mut state, now);
            }
        }
    }

    pub fn unsnooze_now(&self, id: u32) {
        if let Ok(mut state) = self.inner.lock() {
            let now = unix_now_secs();
            if let Some(pos) = state.snapshot.snoozed.iter().position(|x| x.id == id) {
                let mut item = state.snapshot.snoozed.remove(pos);
                item.snooze_until_epoch = 0;
                item.read = false;
                item.timestamp_epoch = now;
                item.expire_at_epoch = now + 7;
                state.snapshot.items.insert(0, item);
                recompute_and_bump(&mut state, now);
            }
        }
    }

    pub fn clear_all(&self) {
        if let Ok(mut state) = self.inner.lock() {
            let ids: Vec<u32> = state.snapshot.items.iter().map(|i| i.id).collect();
            for id in ids {
                update_history_status(&mut state, id, "Cleared");
            }
            state.snapshot.items.clear();
            recompute_and_bump(&mut state, unix_now_secs());
        }
    }

    pub fn clear_history(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.snapshot.history.clear();
            recompute_and_bump(&mut state, unix_now_secs());
        }
    }

    pub fn invoke_action(&self, id: u32, key: &str) {
        let stdin_ref = if let Ok(mut state) = self.inner.lock() {
            update_history_status(&mut state, id, &format!("Action:{key}"));
            recompute_and_bump(&mut state, unix_now_secs());
            state.bridge_stdin.clone()
        } else {
            None
        };
        if let Some(stdin) = stdin_ref {
            send_bridge_cmd(
                &stdin,
                serde_json::json!({ "cmd": "action", "id": id, "key": key }),
            );
        }
    }

    pub fn send_reply(&self, id: u32, reply_text: &str) {
        let trimmed = reply_text.trim();
        if trimmed.is_empty() {
            return;
        }
        let (stdin_ref, default_key) = if let Ok(mut state) = self.inner.lock() {
            let act_key = state
                .snapshot
                .items
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| {
                    n.actions
                        .iter()
                        .find(|a| a.key == "inline-reply" || a.key == "default")
                        .or_else(|| n.actions.first())
                        .map(|a| a.key.clone())
                });
            update_history_status(&mut state, id, &format!("Replied: {trimmed}"));
            if let Some(pos) = state.snapshot.items.iter().position(|n| n.id == id) {
                state.snapshot.items[pos].read = true;
            }
            recompute_and_bump(&mut state, unix_now_secs());
            (state.bridge_stdin.clone(), act_key)
        } else {
            (None, None)
        };

        // Copy reply to Wayland clipboard as a seamless fallback alongside D-Bus NotificationReplied.
        copy_text_to_clipboard(trimmed);
        if let Some(stdin) = stdin_ref {
            send_bridge_cmd(
                &stdin,
                serde_json::json!({ "cmd": "reply", "id": id, "text": trimmed }),
            );
            if let Some(key) = default_key {
                send_bridge_cmd(
                    &stdin,
                    serde_json::json!({ "cmd": "action", "id": id, "key": key }),
                );
            }
        }
    }
}

fn send_bridge_cmd(stdin: &Arc<Mutex<ChildStdin>>, payload: serde_json::Value) {
    if let Ok(mut guard) = stdin.lock() {
        let _ = writeln!(guard, "{}", payload);
        let _ = guard.flush();
    }
}

pub fn copy_text_to_clipboard(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let payload = text.to_string();
    thread::Builder::new()
        .name("velox-notif-clip".to_string())
        .spawn(move || {
            if let Ok(mut child) = Command::new("wl-copy")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(payload.as_bytes());
                }
                let _ = child.wait();
            }
        })
        .ok();
}

fn apply_dbus_event(inner: &Arc<Mutex<NotificationEngineState>>, val: serde_json::Value) {
    let ev = val.get("ev").and_then(|v| v.as_str()).unwrap_or("");
    let now = unix_now_secs();
    let Ok(mut state) = inner.lock() else {
        return;
    };
    match ev {
        "notify" => {
            let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            let app = val
                .get("app")
                .and_then(|v| v.as_str())
                .unwrap_or("App")
                .to_string();
            let desktop_entry = val
                .get("desktop_entry")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let raw_cat = val
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let summary = val
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let body = val
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let urg = match val.get("urgency").and_then(|v| v.as_u64()).unwrap_or(1) {
                0 => NotificationUrgency::Low,
                2 => NotificationUrgency::Critical,
                _ => NotificationUrgency::Normal,
            };
            let timeout_ms = val
                .get("timeout_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1) as i32;
            let actions: Vec<NotificationAction> = val
                .get("actions")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| {
                            Some(NotificationAction {
                                key: a.get("key")?.as_str()?.to_string(),
                                label: a.get("label")?.as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let supports_inline_reply = actions.iter().any(|a| a.key == "inline-reply")
                || classify_raw_category(&app, &desktop_entry, &raw_cat, &summary, &body, false, &state.config)
                    == NotificationCategory::Conversations;

            let item = NotificationItem {
                id,
                app_name: app,
                desktop_entry,
                raw_category_hint: raw_cat,
                summary,
                body,
                urgency: urg,
                actions,
                timestamp_epoch: now,
                expire_at_epoch: compute_expire_epoch(now, timeout_ms),
                read: false,
                is_update_notice: false,
                supports_inline_reply,
                ..NotificationItem::default()
            };
            ingest_item(&mut state, item, now, false);
        }
        "close" => {
            let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            state.snapshot.items.retain(|x| x.id != id);
            recompute_and_bump(&mut state, now);
        }
        _ => {}
    }
}

fn ingest_item(
    state: &mut NotificationEngineState,
    mut item: NotificationItem,
    now: u64,
    is_internal: bool,
) {
    let app_key = effective_app_key(&item.app_name, &item.desktop_entry);
    if !app_key.is_empty() && !state.snapshot.seen_apps.iter().any(|a| a.eq_ignore_ascii_case(&item.app_name)) {
        state.snapshot.seen_apps.push(item.app_name.clone());
        state.snapshot.seen_apps.sort_by_key(|s| s.to_ascii_lowercase());
    }

    enrich_item_fields(&mut item, &state.config);

    // Per-app disabled toggle: record in history as Blocked if history is enabled, skip active queue.
    if is_app_in_list(&app_key, &item.app_name, &state.config.disabled_notification_apps)
        && !item.is_update_notice
    {
        record_history_entry(state, &item, "Blocked");
        recompute_and_bump(state, now);
        return;
    }

    // Check cooldown burst activation (>= 4 external/burst notifications in 25s window).
    if state.config.notification_cooldown_enabled
        && !item.is_priority
        && !item.is_update_notice
        && !is_internal
    {
        state.recent_burst_epochs.retain(|&ts| now.saturating_sub(ts) <= 25);
        state.recent_burst_epochs.push(now);
        if state.snapshot.cooldown_until_epoch <= now && state.recent_burst_epochs.len() >= 4 {
            state.cooldown_started_epoch = now;
            state.snapshot.cooldown_until_epoch = now + 60;
            state.snapshot.cooldown_received_count = 0;
        }
    }

    let in_cooldown = state.config.notification_cooldown_enabled
        && state.snapshot.cooldown_until_epoch > now
        && !item.is_priority
        && !item.is_update_notice
        && item.urgency != NotificationUrgency::Critical;
    if in_cooldown {
        item.arrived_in_cooldown = true;
        item.is_silent = true;
        state.snapshot.cooldown_received_count = state.snapshot.cooldown_received_count.saturating_add(1);
    }

    let status_lbl = if in_cooldown {
        "Cooldown"
    } else if item.is_silent {
        "Silent"
    } else {
        "Delivered"
    };
    record_history_entry(state, &item, status_lbl);

    state.snapshot.items.retain(|x| x.id != item.id);
    state.snapshot.items.insert(0, item);
    if state.snapshot.items.len() > 64 {
        state.snapshot.items.truncate(64);
    }
    recompute_and_bump(state, now);
}

fn reclassify_all_items(state: &mut NotificationEngineState, now: u64) {
    let cfg = state.config.clone();
    for item in &mut state.snapshot.items {
        enrich_item_fields(item, &cfg);
        if item.arrived_in_cooldown && !item.is_priority {
            item.is_silent = true;
        }
    }
    for item in &mut state.snapshot.snoozed {
        enrich_item_fields(item, &cfg);
    }
    recompute_and_bump(state, now);
}

fn enrich_item_fields(item: &mut NotificationItem, cfg: &Config) {
    item.category = classify_raw_category(
        &item.app_name,
        &item.desktop_entry,
        &item.raw_category_hint,
        &item.summary,
        &item.body,
        item.is_update_notice,
        cfg,
    );
    item.is_conversation = item.category == NotificationCategory::Conversations;
    item.conversation_key = if item.is_conversation {
        Some(extract_conversation_key(&item.app_name, &item.summary))
    } else {
        None
    };

    item.is_priority = item
        .conversation_key
        .as_ref()
        .is_some_and(|ck| {
            cfg.priority_conversations
                .iter()
                .any(|p| p.eq_ignore_ascii_case(ck))
        })
        || cfg
            .priority_conversations
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&format!("{}::{}", item.app_name, item.summary)));

    let app_key = effective_app_key(&item.app_name, &item.desktop_entry);
    let app_silenced = is_app_in_list(&app_key, &item.app_name, &cfg.silent_notification_apps);
    let organizer_silenced = cfg.notification_organizer_enabled
        && cfg
            .organized_silenced_categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case(item.category.label()));
    item.is_silent = if item.is_priority || item.is_update_notice || item.urgency == NotificationUrgency::Critical {
        false
    } else {
        item.urgency == NotificationUrgency::Low || app_silenced || organizer_silenced
    };

    item.summary_recap = if cfg.notification_summaries_enabled {
        summarize_message(&item.summary, &item.body)
    } else {
        None
    };
    item.suggested_actions = generate_suggested_actions(
        &item.summary,
        &item.body,
        item.category,
        !item.actions.is_empty(),
    );
}

pub fn normalize_app_key(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn effective_app_key(app_name: &str, desktop_entry: &str) -> String {
    if !desktop_entry.trim().is_empty() {
        normalize_app_key(desktop_entry)
    } else {
        normalize_app_key(app_name)
    }
}

fn is_app_in_list(app_key: &str, app_name: &str, list: &[String]) -> bool {
    let name_key = normalize_app_key(app_name);
    list.iter().any(|entry| {
        let ek = normalize_app_key(entry);
        !ek.is_empty() && (ek == app_key || ek == name_key)
    })
}

pub fn extract_conversation_key(app_name: &str, summary: &str) -> String {
    let clean_summary = summary
        .split([':', '(', '|'])
        .next()
        .unwrap_or(summary)
        .trim();
    let target = if clean_summary.is_empty() {
        app_name.trim()
    } else {
        clean_summary
    };
    format!("{} · {}", app_name.trim(), target)
}

/// On-device heuristic notification classifier (no cloud / Google calls).
pub fn classify_raw_category(
    app_name: &str,
    desktop_entry: &str,
    raw_hint: &str,
    summary: &str,
    body: &str,
    is_update_notice: bool,
    cfg: &Config,
) -> NotificationCategory {
    if is_update_notice {
        return NotificationCategory::Updates;
    }
    let app_key = effective_app_key(app_name, desktop_entry);
    for (k, v) in &cfg.app_category_overrides {
        if normalize_app_key(k) == app_key {
            if let Some(cat) = NotificationCategory::from_label(v) {
                return cat;
            }
        }
    }

    let hint = raw_hint.to_ascii_lowercase();
    if hint.starts_with("im.") || hint.starts_with("chat") || hint == "presence.online" {
        return NotificationCategory::Conversations;
    }
    if hint.starts_with("device") || hint.starts_with("network") || hint.starts_with("transfer") {
        return NotificationCategory::System;
    }

    let app_l = format!("{} {}", app_name, desktop_entry).to_ascii_lowercase();
    let text_l = format!("{} {}", summary, body).to_ascii_lowercase();

    if contains_any(
        &text_l,
        &[
            "% off",
            "discount",
            "promo",
            "coupon",
            "limited time",
            "flash sale",
            "special offer",
            "cashback",
            "unsubscribe",
            "free trial",
            "upgrade plan",
        ],
    ) {
        return NotificationCategory::Promotions;
    }

    if contains_any(
        &text_l,
        &[
            "breaking news",
            "daily digest",
            "top stories",
            "headline",
            "newsletter",
            "published a new article",
            "weekly report",
        ],
    ) || contains_any(&app_l, &["news", "feed", "rss"])
    {
        return NotificationCategory::News;
    }

    if contains_any(
        &text_l,
        &[
            "liked your",
            "commented on",
            "followed you",
            "retweeted",
            "upvoted",
            "friend request",
            "new subscriber",
            "went live",
        ],
    ) || contains_any(&app_l, &["reddit", "youtube", "twitch", "twitter", "bluesky", "mastodon"])
    {
        return NotificationCategory::Social;
    }

    if contains_any(
        &text_l,
        &[
            "suggested",
            "recommended for you",
            "you might like",
            "quick tip",
            "discover ",
        ],
    ) {
        return NotificationCategory::Suggested;
    }

    if contains_any(
        &app_l,
        &[
            "discord",
            "telegram",
            "signal",
            "slack",
            "element",
            "whatsapp",
            "thunderbird",
            "messages",
            "matrix",
            "teams",
            "vesktop",
            "cinny",
        ],
    ) || (summary.contains('@') || text_l.contains("sent a message") || text_l.contains("replied:"))
    {
        return NotificationCategory::Conversations;
    }

    if contains_any(&text_l, &["update available", "new version", "package upgrade"]) {
        return NotificationCategory::Updates;
    }

    NotificationCategory::System
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Generates a concise on-device extractive recap for longer messages or multi-sentence bodies.
pub fn summarize_message(summary: &str, body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let line_count = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
    if trimmed.chars().count() < 86 && line_count <= 1 {
        return None;
    }

    let collapsed = trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let first_sentence = collapsed
        .split(['.', '!', '?'])
        .map(str::trim)
        .find(|s| s.len() >= 12)
        .unwrap_or(&collapsed);
    let prefix = if summary.trim().is_empty() {
        ""
    } else {
        "Recap: "
    };
    let max_chars = 78;
    let clipped: String = first_sentence.chars().take(max_chars).collect();
    if first_sentence.chars().count() > max_chars || collapsed.len() > first_sentence.len() + 6 {
        Some(format!("{prefix}{clipped}..."))
    } else {
        Some(format!("{prefix}{clipped}"))
    }
}

/// Generates contextual on-device suggested replies & smart actions (copy OTP, open link, reply).
pub fn generate_suggested_actions(
    summary: &str,
    body: &str,
    category: NotificationCategory,
    has_app_actions: bool,
) -> Vec<SuggestedAction> {
    let mut out = Vec::new();
    let combined = format!("{summary} {body}");
    let lower = combined.to_ascii_lowercase();

    // 1. Smart verification/OTP copy chip
    if contains_any(&lower, &["code", "otp", "pin", "verify", "verification", "2fa"]) {
        if let Some(code) = extract_otp_code(&combined) {
            out.push(SuggestedAction {
                label: format!("Copy {code}"),
                payload: code,
                kind: SuggestedActionKind::CopyCode,
            });
        }
    }

    // 2. Smart URL opener chip
    if let Some(url) = extract_first_url(&combined) {
        out.push(SuggestedAction {
            label: "Open Link".to_string(),
            payload: url,
            kind: SuggestedActionKind::OpenUrl,
        });
    }

    // 3. Smart quick reply suggestions for chats & questions
    if category == NotificationCategory::Conversations || combined.contains('?') {
        let replies: &[&str] = if contains_any(&lower, &["thank", "thx", "appreciate"]) {
            &["You're welcome!", "No problem"]
        } else if contains_any(&lower, &["call", "meet", "meeting", "join", "pm", "am", "today"]) {
            &["Sounds good", "On my way", "In 10 min"]
        } else if combined.contains('?') || contains_any(&lower, &["can you", "free", "check"]) {
            &["Yes, on it 👍", "Sounds good", "Busy right now"]
        } else {
            &["Got it 👍", "Thanks!"]
        };
        for r in replies {
            if out.len() >= 3 {
                break;
            }
            out.push(SuggestedAction {
                label: (*r).to_string(),
                payload: (*r).to_string(),
                kind: SuggestedActionKind::QuickReply,
            });
        }
    }

    if out.is_empty() && !has_app_actions {
        out.push(SuggestedAction {
            label: "Mark Read".to_string(),
            payload: String::new(),
            kind: SuggestedActionKind::MarkRead,
        });
    }

    out.truncate(3);
    out
}

fn extract_otp_code(text: &str) -> Option<String> {
    for raw_tok in text.split_whitespace() {
        let tok = raw_tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let digits_only: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
        if (4..=8).contains(&digits_only.len())
            && tok.chars().all(|c| c.is_ascii_digit() || c == '-')
        {
            return Some(digits_only);
        }
    }
    None
}

fn extract_first_url(text: &str) -> Option<String> {
    for tok in text.split_whitespace() {
        let cleaned = tok.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | '.' | ';' | '"' | '\''));
        if cleaned.starts_with("https://") || cleaned.starts_with("http://") {
            return Some(cleaned.to_string());
        }
    }
    None
}

fn record_history_entry(state: &mut NotificationEngineState, item: &NotificationItem, status: &str) {
    if !state.config.notification_history_enabled {
        return;
    }
    let body_preview: String = item.body.trim().chars().take(120).collect();
    let app_key = effective_app_key(&item.app_name, &item.desktop_entry);
    state.snapshot.history.insert(
        0,
        NotificationHistoryEntry {
            id: item.id,
            app_name: item.app_name.clone(),
            app_key,
            category: item.category,
            summary: item.summary.clone(),
            body_preview,
            summary_recap: item.summary_recap.clone(),
            timestamp_epoch: item.timestamp_epoch,
            conversation_key: item.conversation_key.clone(),
            status: status.to_string(),
        },
    );
    if state.snapshot.history.len() > 200 {
        state.snapshot.history.truncate(200);
    }
}

fn update_history_status(state: &mut NotificationEngineState, id: u32, status: &str) {
    if let Some(entry) = state.snapshot.history.iter_mut().find(|h| h.id == id) {
        entry.status = status.to_string();
    }
}

fn recompute_and_bump(state: &mut NotificationEngineState, now: u64) {
    // Stable sort: Priority Conversations first, then active updates/non-silent, newest first within each tier.
    state.snapshot.items.sort_by(|a, b| {
        let tier = |n: &NotificationItem| -> u8 {
            if n.is_priority {
                0
            } else if n.is_update_notice || n.urgency == NotificationUrgency::Critical {
                1
            } else if n.is_conversation {
                2
            } else if !n.is_silent {
                3
            } else {
                4
            }
        };
        tier(a)
            .cmp(&tier(b))
            .then_with(|| b.timestamp_epoch.cmp(&a.timestamp_epoch))
            .then_with(|| b.id.cmp(&a.id))
    });

    state.snapshot.unread_count = state.snapshot.items.iter().filter(|n| !n.read).count();
    state.snapshot.quiet_unread_count = state
        .snapshot
        .items
        .iter()
        .filter(|n| !n.read && n.is_silent && !n.is_priority)
        .count();
    state.snapshot.status_bar_unread_count = if state.config.hide_silent_notifications_in_status_bar {
        state
            .snapshot
            .items
            .iter()
            .filter(|n| !n.read && (!n.is_silent || n.is_priority))
            .count()
    } else {
        state.snapshot.unread_count
    };

    state.snapshot.cooldown_active = state.snapshot.cooldown_until_epoch > now;
    state.snapshot.cooldown_remaining_secs = state.snapshot.cooldown_until_epoch.saturating_sub(now);
    state.snapshot.conversation_groups = build_conversation_groups(&state.snapshot.items, &state.config);
    state.snapshot.version = state.snapshot.version.wrapping_add(1);

    if state.persist_to_disk {
        persist_store(&state.snapshot.history, &state.snapshot.seen_apps);
    }
}

fn build_conversation_groups(items: &[NotificationItem], cfg: &Config) -> Vec<ConversationGroupSummary> {
    let mut grouped: BTreeMap<String, Vec<&NotificationItem>> = BTreeMap::new();
    for item in items {
        if item.category == NotificationCategory::Conversations {
            let key = item
                .conversation_key
                .clone()
                .unwrap_or_else(|| extract_conversation_key(&item.app_name, &item.summary));
            grouped.entry(key).or_default().push(item);
        }
    }

    let mut out = Vec::with_capacity(grouped.len());
    for (key, group_items) in grouped {
        let count = group_items.len();
        let unread_count = group_items.iter().filter(|n| !n.read).count();
        let latest = group_items[0];
        let latest_epoch = group_items.iter().map(|n| n.timestamp_epoch).max().unwrap_or(0);
        let is_priority = group_items.iter().any(|n| n.is_priority);
        let snippets: Vec<String> = group_items
            .iter()
            .take(3)
            .map(|n| {
                let raw = if !n.body.trim().is_empty() {
                    n.body.trim()
                } else {
                    n.summary.trim()
                };
                raw.chars().take(42).collect()
            })
            .collect();
        let recap = if cfg.notification_summaries_enabled && count >= 2 {
            format!("{} messages: {}", count, snippets.join(" | "))
        } else if let Some(ref s) = latest.summary_recap {
            s.clone()
        } else {
            snippets.first().cloned().unwrap_or_default()
        };
        out.push(ConversationGroupSummary {
            key: key.clone(),
            app_name: latest.app_name.clone(),
            title: latest.summary.clone(),
            count,
            unread_count,
            is_priority,
            recap,
            latest_epoch,
        });
    }
    out.sort_by(|a, b| {
        b.is_priority
            .cmp(&a.is_priority)
            .then_with(|| b.latest_epoch.cmp(&a.latest_epoch))
    });
    out
}

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|s| !s.is_empty())
                .map(|h| PathBuf::from(h).join(".local/share"))
        })?;
    Some(base.join("velox-wm").join("notification-history.json"))
}

fn load_persisted_store() -> PersistedNotificationStore {
    let Some(path) = store_path() else {
        return PersistedNotificationStore::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return PersistedNotificationStore::default();
    };
    serde_json::from_str::<PersistedNotificationStore>(&raw).unwrap_or_default()
}

pub fn load_notification_store() -> (Vec<NotificationHistoryEntry>, Vec<String>) {
    let store = load_persisted_store();
    (store.history, store.seen_apps)
}

pub fn clear_persisted_notification_history() {
    let mut store = load_persisted_store();
    store.history.clear();
    persist_store(&store.history, &store.seen_apps);
}

fn persist_store(history: &[NotificationHistoryEntry], seen_apps: &[String]) {
    let Some(path) = store_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let store = PersistedNotificationStore {
        history: history.to_vec(),
        seen_apps: seen_apps.to_vec(),
    };
    if let Ok(json) = serde_json::to_string(&store) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn compute_expire_epoch(now: u64, timeout_ms: i32) -> u64 {
    if timeout_ms == 0 {
        0
    } else if timeout_ms > 0 {
        now + ((timeout_ms as u64 + 999) / 1000).clamp(2, 30)
    } else {
        now + 6
    }
}

pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn format_relative_time(ts: u64) -> String {
    let now = unix_now_secs();
    let diff = now.saturating_sub(ts);
    if diff < 15 {
        "now".to_string()
    } else if diff < 60 {
        format!("{diff}s")
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else {
        format!("{}h", diff / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_push_read_dismiss_flow() {
        let svc = NotificationService::start(false);
        let id1 = svc.push_internal(
            "TestApp",
            "Build complete",
            "All targets passed",
            NotificationUrgency::Normal,
            vec![],
            false,
            5000,
        );
        let id2 = svc.push_internal(
            "Updates",
            "New release",
            "Click reload",
            NotificationUrgency::Critical,
            vec![NotificationAction {
                key: "velox-reload".into(),
                label: "Reload WM".into(),
            }],
            true,
            0,
        );
        let snap = svc.snapshot();
        assert_eq!(snap.items.len(), 2);
        assert_eq!(snap.unread_count, 2);
        assert_eq!(snap.items[0].id, id2);
        svc.mark_all_read();
        assert_eq!(svc.snapshot().unread_count, 0);
        svc.dismiss(id1);
        assert_eq!(svc.snapshot().items.len(), 1);
    }

    #[test]
    fn organizer_summaries_otp_and_priority_work() {
        let svc = NotificationService::start(false);
        let mut cfg = Config::default();
        cfg.notification_organizer_enabled = true;
        cfg.organized_silenced_categories = vec!["Promotions".into()];
        cfg.hide_silent_notifications_in_status_bar = true;
        cfg.priority_conversations = vec!["Discord · Alice".into()];
        svc.apply_config(&cfg);

        // Promotional silent item
        let promo_id = svc.push_internal(
            "ShopMail",
            "Flash sale 50% off",
            "Limited time coupon just for you!",
            NotificationUrgency::Normal,
            vec![],
            false,
            5000,
        );
        // Priority long conversation with question + OTP
        let chat_id = svc.push_internal(
            "Discord",
            "Alice",
            "Hey, are you free for the call today? Verification code 482910 for the sandbox test meeting at 5pm.",
            NotificationUrgency::Normal,
            vec![],
            false,
            5000,
        );
        let snap = svc.snapshot();
        let promo = snap.items.iter().find(|n| n.id == promo_id).unwrap();
        assert_eq!(promo.category, NotificationCategory::Promotions);
        assert!(promo.is_silent);

        let chat = snap.items.iter().find(|n| n.id == chat_id).unwrap();
        assert_eq!(chat.category, NotificationCategory::Conversations);
        assert!(chat.is_priority);
        assert!(!chat.is_silent);
        assert!(chat.summary_recap.is_some());
        assert!(chat
            .suggested_actions
            .iter()
            .any(|a| a.kind == SuggestedActionKind::CopyCode && a.payload == "482910"));
        assert!(chat
            .suggested_actions
            .iter()
            .any(|a| a.kind == SuggestedActionKind::QuickReply));
        // Status bar hides silent promos cleanly when configured
        assert_eq!(snap.unread_count, 2);
        assert_eq!(snap.status_bar_unread_count, 1);
        assert_eq!(snap.quiet_unread_count, 1);
    }

    #[test]
    fn snooze_resurfaces_and_cooldown_triggers_on_burst() {
        let svc = NotificationService::start(false);
        let cfg = Config::default();
        svc.apply_config(&cfg);

        // Simulate burst of 4 external notifications within 5 seconds
        for i in 0..4u32 {
            let val = serde_json::json!({
                "ev": "notify",
                "id": 10 + i,
                "app": "FeedBot",
                "summary": format!("Alert {i}"),
                "body": "Burst message",
                "actions": [],
                "urgency": 1,
                "timeout_ms": 5000
            });
            apply_dbus_event(&svc.inner, val);
        }
        let now = unix_now_secs();
        let snap = svc.snapshot();
        assert!(snap.cooldown_active);
        assert!(snap.cooldown_remaining_secs > 0 && snap.cooldown_remaining_secs <= 60);

        // Snooze one item and advance time
        let target_id = snap.items[0].id;
        svc.snooze_at(target_id, 30, now);
        assert_eq!(svc.snapshot().snoozed.len(), 1);
        svc.tick_at(&cfg, now + 35);
        assert!(svc.snapshot().snoozed.is_empty());
        assert!(svc.snapshot().items.iter().any(|n| n.id == target_id && !n.read));
    }

    #[test]
    fn disabled_app_blocks_delivery_and_keeps_history_toggleable() {
        let svc = NotificationService::start(false);
        let mut cfg = Config::default();
        cfg.disabled_notification_apps = vec!["noisymail".into()];
        svc.apply_config(&cfg);

        svc.push_internal(
            "NoisyMail",
            "Newsletter",
            "Daily junk",
            NotificationUrgency::Normal,
            vec![],
            false,
            5000,
        );
        let snap = svc.snapshot();
        assert!(snap.items.is_empty());
        assert!(snap.seen_apps.iter().any(|a| a == "NoisyMail"));
        assert_eq!(snap.history.first().map(|h| h.status.as_str()), Some("Blocked"));
    }
}
