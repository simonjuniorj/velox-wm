use std::collections::HashMap;

use smithay::utils::{Logical, Point, Rectangle};

#[derive(Debug, Clone, Copy)]
struct AnimState {
    cur_x: f32,
    cur_y: f32,
    cur_w: f32,
    cur_h: f32,
    target: Rectangle<i32, Logical>,
    active: bool,
}

/// Frame-rate-independent smooth exponential ease-out animator for window positions and sizes.
#[derive(Debug, Default)]
pub struct WindowAnimator {
    entries: HashMap<u64, AnimState>,
}

impl WindowAnimator {
    /// Immediately snaps a window to `target` without trailing animation (used during mouse drags).
    pub fn snap(&mut self, id: u64, target: Rectangle<i32, Logical>) {
        self.entries.insert(
            id,
            AnimState {
                cur_x: target.loc.x as f32,
                cur_y: target.loc.y as f32,
                cur_w: target.size.w as f32,
                cur_h: target.size.h as f32,
                target,
                active: false,
            },
        );
    }

    /// Updates animation toward `target` for window `id` and returns the current visual location.
    pub fn step(
        &mut self,
        id: u64,
        target: Rectangle<i32, Logical>,
        enabled: bool,
        speed: f32,
        dt_secs: f32,
    ) -> Point<i32, Logical> {
        if !enabled {
            self.snap(id, target);
            return target.loc;
        }

        let entry = self.entries.entry(id).or_insert(AnimState {
            cur_x: target.loc.x as f32,
            cur_y: target.loc.y as f32 + 12.0,
            cur_w: target.size.w as f32,
            cur_h: target.size.h as f32,
            target,
            active: true,
        });

        if entry.target.loc != target.loc {
            entry.target = target;
            entry.active = true;
        } else if entry.target.size != target.size {
            // Size-only updates do not animate window origin, so avoid waking 144Hz animation frames.
            entry.target.size = target.size;
            entry.cur_w = target.size.w as f32;
            entry.cur_h = target.size.h as f32;
        }

        if entry.active {
            let factor = (1.0
                - (1.0 - speed.clamp(0.05, 0.9)).powf((dt_secs * 60.0).clamp(0.2, 3.0)))
            .clamp(0.05, 1.0);
            entry.cur_x += (target.loc.x as f32 - entry.cur_x) * factor;
            entry.cur_y += (target.loc.y as f32 - entry.cur_y) * factor;
            entry.cur_w += (target.size.w as f32 - entry.cur_w) * factor;
            entry.cur_h += (target.size.h as f32 - entry.cur_h) * factor;

            if (target.loc.x as f32 - entry.cur_x).abs() < 0.75
                && (target.loc.y as f32 - entry.cur_y).abs() < 0.75
            {
                entry.cur_x = target.loc.x as f32;
                entry.cur_y = target.loc.y as f32;
                entry.cur_w = target.size.w as f32;
                entry.cur_h = target.size.h as f32;
                entry.active = false;
            }
        }

        Point::from((entry.cur_x.round() as i32, entry.cur_y.round() as i32))
    }

    pub fn any_active(&self) -> bool {
        self.entries.values().any(|e| e.active)
    }

    pub fn retain_ids(&mut self, live_ids: &[u64]) {
        self.entries.retain(|k, _| live_ids.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_converges_to_target() {
        let mut anim = WindowAnimator::default();
        let target = Rectangle::new((200, 120).into(), (800, 600).into());
        for _ in 0..60 {
            anim.step(1, target, true, 0.25, 1.0 / 60.0);
        }
        let final_loc = anim.step(1, target, true, 0.25, 1.0 / 60.0);
        assert_eq!(final_loc, Point::from((200, 120)));
        assert!(!anim.any_active());
    }

    #[test]
    fn snap_clears_active_state_immediately() {
        let mut anim = WindowAnimator::default();
        let target = Rectangle::new((420, 160).into(), (640, 480).into());
        anim.snap(7, target);
        assert!(!anim.any_active());
        assert_eq!(
            anim.step(7, target, true, 0.25, 1.0 / 144.0),
            Point::from((420, 160))
        );
    }

    #[test]
    fn size_only_resize_does_not_activate_location_animation() {
        let mut anim = WindowAnimator::default();
        let initial = Rectangle::new((100, 80).into(), (600, 400).into());
        anim.snap(9, initial);
        let resized = Rectangle::new((100, 80).into(), (920, 680).into());
        let loc = anim.step(9, resized, true, 0.25, 1.0 / 144.0);
        assert_eq!(loc, Point::from((100, 80)));
        assert!(!anim.any_active());
    }
}
