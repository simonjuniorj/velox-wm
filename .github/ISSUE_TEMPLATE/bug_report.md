---
name: Bug / Glitch / Stutter Report
about: Report a crash, visual glitch, frame stutter, input bug, or unexpected behavior in Velox WM
title: "[Bug] "
labels: bug
---

> Thanks for testing Velox WM! Velox is in active early development — detailed reports help us stabilize KMS/DRM rendering, XWayland edge cases, input focus, and battery saver pacing.

### Summary
A clear description of the bug, glitch, or stutter you experienced.

### Steps to Reproduce
1. Launch / configure...
2. Perform action...
3. Observe...

### Expected vs Actual Behavior
- **Expected:**
- **Actual:**

### Environment Details
- **Distro & Kernel:** (e.g., Arch Linux `6.x`)
- **GPU & Driver:** (e.g., AMD RADV / NVIDIA / Intel Mesa)
- **Monitors & Refresh Rates:** (e.g., DP-1 `1920x1080@144Hz` VRR, HDMI-1 `2560x1440@60Hz`)
- **Session Type:** Bare-metal DRM/LightDM (`--drm`) or Nested (`--nested`)
- **Affected App:** (Name + Wayland native vs XWayland)
- **Velox Version / Commit:** (`velox-wm` version from `Cargo.toml` or `git rev-parse --short HEAD`)

### Relevant Logs
Please attach the tail of `~/.local/state/velox-wm/velox.log` if available:
```text
Paste relevant log lines here (strip any personal usernames/hostnames if needed)
```
