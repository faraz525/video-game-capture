use crate::input::{InputEvent, InputEventKind, MouseButton};
use std::collections::{BTreeSet, HashSet};

use super::types::FrameAction;

/// Converts an event-stream of input events into per-frame action snapshots.
///
/// This is the single most requested data format across world model research.
/// Papers like GF-Minecraft, DIAMOND, GameNGen, and Dreamer 4 all require
/// frame-aligned action labels. This state machine walks the event stream,
/// maintaining held keys/buttons and accumulating mouse deltas, then snapshots
/// the state at each video frame boundary.
pub fn index_frame_actions(
    input_events: &[InputEvent],
    frame_count: u64,
    fps: u32,
    first_frame_timestamp_us: u64,
) -> Vec<FrameAction> {
    if frame_count == 0 || fps == 0 {
        return Vec::new();
    }

    let frame_interval_us = 1_000_000u64 / fps as u64;
    let mut actions = Vec::with_capacity(frame_count as usize);

    // State machine
    let mut held_keys: BTreeSet<String> = BTreeSet::new();
    let mut held_buttons: BTreeSet<String> = BTreeSet::new();
    let mut mouse_x: f64 = 0.0;
    let mut mouse_y: f64 = 0.0;
    let mut accum_dx: f64 = 0.0;
    let mut accum_dy: f64 = 0.0;
    let mut accum_scroll_dx: f64 = 0.0;
    let mut accum_scroll_dy: f64 = 0.0;

    let mut event_idx = 0;

    for frame in 0..frame_count {
        let frame_ts = first_frame_timestamp_us + frame * frame_interval_us;

        // Process all input events up to this frame's timestamp
        while event_idx < input_events.len() && input_events[event_idx].timestamp_us <= frame_ts {
            let event = &input_events[event_idx];
            match &event.kind {
                InputEventKind::Key(key) => {
                    if key.pressed {
                        held_keys.insert(key.key.clone());
                    } else {
                        held_keys.remove(&key.key);
                    }
                }
                InputEventKind::MouseButton(btn) => {
                    let name = match btn.button {
                        MouseButton::Left => "left",
                        MouseButton::Right => "right",
                        MouseButton::Middle => "middle",
                    };
                    if btn.pressed {
                        held_buttons.insert(name.to_string());
                    } else {
                        held_buttons.remove(name);
                    }
                    mouse_x = btn.x;
                    mouse_y = btn.y;
                }
                InputEventKind::MouseMove(mv) => {
                    // Accumulate deltas from position changes
                    accum_dx += mv.x - mouse_x;
                    accum_dy += mv.y - mouse_y;
                    mouse_x = mv.x;
                    mouse_y = mv.y;
                }
                InputEventKind::MouseScroll(scroll) => {
                    accum_scroll_dx += scroll.delta_x;
                    accum_scroll_dy += scroll.delta_y;
                }
            }
            event_idx += 1;
        }

        // Snapshot the current state
        actions.push(FrameAction {
            frame,
            timestamp_us: frame_ts,
            keys_held: held_keys.iter().cloned().collect(),
            mouse_buttons_held: held_buttons.iter().cloned().collect(),
            mouse_x,
            mouse_y,
            mouse_dx: accum_dx,
            mouse_dy: accum_dy,
            scroll_dx: accum_scroll_dx,
            scroll_dy: accum_scroll_dy,
        });

        // Reset accumulated deltas for next frame
        accum_dx = 0.0;
        accum_dy = 0.0;
        accum_scroll_dx = 0.0;
        accum_scroll_dy = 0.0;
    }

    actions
}

/// Compute frame count and effective FPS from frame data.
///
/// When the actual captured frames are available, this derives the frame count
/// and FPS from the timestamp range. When only metadata is available, pass
/// the metadata values directly to `index_frame_actions`.
pub fn compute_frame_params(
    duration_secs: f64,
    fps: u32,
) -> (u64, u32) {
    let frame_count = (duration_secs * fps as f64).round() as u64;
    (frame_count, fps)
}

/// Compute action statistics from frame actions (used by quality scoring).
pub fn compute_action_stats(actions: &[FrameAction]) -> ActionStats {
    if actions.is_empty() {
        return ActionStats::default();
    }

    let mut total_keys_held = 0u64;
    let mut peak_keys = 0u32;
    let mut active_frames = 0u64;
    let mut unique_keys: HashSet<String> = HashSet::new();
    let mut total_mouse_speed = 0.0f64;
    let mut peak_mouse_speed = 0.0f64;

    let frame_count = actions.len() as f64;

    for action in actions {
        let num_keys = action.keys_held.len() as u32;
        total_keys_held += num_keys as u64;
        peak_keys = peak_keys.max(num_keys);

        if num_keys > 0 || !action.mouse_buttons_held.is_empty()
            || action.mouse_dx.abs() > 0.1 || action.mouse_dy.abs() > 0.1
        {
            active_frames += 1;
        }

        for key in &action.keys_held {
            unique_keys.insert(key.clone());
        }

        let speed = (action.mouse_dx.powi(2) + action.mouse_dy.powi(2)).sqrt();
        total_mouse_speed += speed;
        peak_mouse_speed = peak_mouse_speed.max(speed);
    }

    ActionStats {
        avg_simultaneous_keys: total_keys_held as f64 / frame_count,
        peak_simultaneous_keys: peak_keys,
        input_activity_ratio: active_frames as f64 / frame_count,
        unique_keys_used: unique_keys.len() as u32,
        avg_mouse_speed: total_mouse_speed / frame_count,
        peak_mouse_speed,
    }
}

/// Summary statistics derived from frame actions.
#[derive(Debug, Default)]
pub struct ActionStats {
    pub avg_simultaneous_keys: f64,
    pub peak_simultaneous_keys: u32,
    pub input_activity_ratio: f64,
    pub unique_keys_used: u32,
    pub avg_mouse_speed: f64,
    pub peak_mouse_speed: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::*;

    fn key_event(ts: u64, key: &str, pressed: bool) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::Key(KeyEvent {
                key: key.to_string(),
                pressed,
            }),
        }
    }

    fn mouse_move(ts: u64, x: f64, y: f64) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::MouseMove(MouseMoveEvent { x, y }),
        }
    }

    fn mouse_button(ts: u64, button: MouseButton, pressed: bool, x: f64, y: f64) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::MouseButton(MouseButtonEvent {
                button,
                pressed,
                x,
                y,
            }),
        }
    }

    fn scroll(ts: u64, dx: f64, dy: f64) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::MouseScroll(MouseScrollEvent {
                delta_x: dx,
                delta_y: dy,
            }),
        }
    }

    #[test]
    fn empty_input_produces_zero_frame_actions() {
        let actions = index_frame_actions(&[], 10, 30, 0);
        assert_eq!(actions.len(), 10);
        for action in &actions {
            assert!(action.keys_held.is_empty());
            assert!(action.mouse_buttons_held.is_empty());
            assert_eq!(action.mouse_dx, 0.0);
            assert_eq!(action.mouse_dy, 0.0);
        }
    }

    #[test]
    fn zero_frames_produces_empty_result() {
        let actions = index_frame_actions(&[], 0, 30, 0);
        assert!(actions.is_empty());
    }

    #[test]
    fn zero_fps_produces_empty_result() {
        let actions = index_frame_actions(&[], 10, 0, 0);
        assert!(actions.is_empty());
    }

    #[test]
    fn key_held_across_multiple_frames() {
        // Press W at frame 0, release at frame 5 (at 30fps, frame interval = 33333us)
        let events = vec![
            key_event(0, "KeyW", true),
            key_event(166_665, "KeyW", false), // ~5 frames in
        ];

        let actions = index_frame_actions(&events, 10, 30, 0);
        assert_eq!(actions.len(), 10);

        // Frames 0-4: W should be held
        for i in 0..5 {
            assert!(
                actions[i].keys_held.contains(&"KeyW".to_string()),
                "frame {i} should have KeyW held"
            );
        }

        // Frames 5-9: W should be released
        for i in 5..10 {
            assert!(
                !actions[i].keys_held.contains(&"KeyW".to_string()),
                "frame {i} should not have KeyW held"
            );
        }
    }

    #[test]
    fn multiple_simultaneous_keys() {
        let events = vec![
            key_event(0, "KeyW", true),
            key_event(1000, "ShiftLeft", true),
            key_event(2000, "Space", true),
        ];

        let actions = index_frame_actions(&events, 3, 30, 0);

        // By frame 0 (ts=0): KeyW pressed
        assert_eq!(actions[0].keys_held, vec!["KeyW"]);

        // By frame 1 (ts=33333): all three keys pressed
        assert_eq!(actions[1].keys_held.len(), 3);
        assert!(actions[1].keys_held.contains(&"KeyW".to_string()));
        assert!(actions[1].keys_held.contains(&"ShiftLeft".to_string()));
        assert!(actions[1].keys_held.contains(&"Space".to_string()));
    }

    #[test]
    fn key_press_and_release_within_one_frame() {
        // Press and release within the same frame interval
        let events = vec![
            key_event(1000, "KeyW", true),
            key_event(2000, "KeyW", false),
        ];

        // Frame at ts=33333 — both events already processed, key is released
        let actions = index_frame_actions(&events, 2, 30, 0);

        // Frame 0 (ts=0): no events yet
        assert!(actions[0].keys_held.is_empty());

        // Frame 1 (ts=33333): key pressed then released within interval
        assert!(actions[1].keys_held.is_empty());
    }

    #[test]
    fn mouse_delta_accumulation() {
        let events = vec![
            mouse_move(5000, 100.0, 200.0),
            mouse_move(10000, 120.0, 210.0), // dx=20, dy=10
            mouse_move(15000, 150.0, 220.0), // dx=30, dy=10
        ];

        let actions = index_frame_actions(&events, 2, 30, 0);

        // Frame 0 (ts=0): no events yet
        assert_eq!(actions[0].mouse_x, 0.0);
        assert_eq!(actions[0].mouse_dx, 0.0);

        // Frame 1 (ts=33333): all mouse moves processed
        assert_eq!(actions[1].mouse_x, 150.0);
        assert_eq!(actions[1].mouse_y, 220.0);
        // Total delta: 0→100 + 100→120 + 120→150 = 150, 0→200 + 200→210 + 210→220 = 220
        assert!((actions[1].mouse_dx - 150.0).abs() < 0.01);
        assert!((actions[1].mouse_dy - 220.0).abs() < 0.01);
    }

    #[test]
    fn mouse_buttons_tracked() {
        let events = vec![
            mouse_button(0, MouseButton::Left, true, 100.0, 200.0),
            mouse_button(50000, MouseButton::Left, false, 100.0, 200.0),
        ];

        let actions = index_frame_actions(&events, 3, 30, 0);

        // Frame 0: left button pressed
        assert!(actions[0].mouse_buttons_held.contains(&"left".to_string()));

        // Frame 1 (ts=33333): still pressed (release is at 50000)
        assert!(actions[1].mouse_buttons_held.contains(&"left".to_string()));

        // Frame 2 (ts=66666): released
        assert!(actions[2].mouse_buttons_held.is_empty());
    }

    #[test]
    fn scroll_accumulation() {
        let events = vec![
            scroll(5000, 0.0, 3.0),
            scroll(10000, 0.0, -1.0),
        ];

        let actions = index_frame_actions(&events, 2, 30, 0);

        // Frame 0: no events yet
        assert_eq!(actions[0].scroll_dy, 0.0);

        // Frame 1: both scroll events accumulated
        assert!((actions[1].scroll_dy - 2.0).abs() < 0.01);
    }

    #[test]
    fn frame_timestamps_correct() {
        let actions = index_frame_actions(&[], 5, 60, 100_000);

        // 60fps = 1_000_000/60 = 16666us per frame (integer division), starting at 100000
        let interval = 1_000_000u64 / 60;
        assert_eq!(actions[0].timestamp_us, 100_000);
        assert_eq!(actions[1].timestamp_us, 100_000 + interval);
        assert_eq!(actions[2].timestamp_us, 100_000 + 2 * interval);
    }

    #[test]
    fn frame_numbers_sequential() {
        let actions = index_frame_actions(&[], 5, 30, 0);
        for (i, action) in actions.iter().enumerate() {
            assert_eq!(action.frame, i as u64);
        }
    }

    #[test]
    fn compute_frame_params_correct() {
        let (count, fps) = compute_frame_params(10.0, 30);
        assert_eq!(count, 300);
        assert_eq!(fps, 30);

        let (count, fps) = compute_frame_params(1.5, 60);
        assert_eq!(count, 90);
        assert_eq!(fps, 60);
    }

    #[test]
    fn action_stats_empty() {
        let stats = compute_action_stats(&[]);
        assert_eq!(stats.avg_simultaneous_keys, 0.0);
        assert_eq!(stats.peak_simultaneous_keys, 0);
        assert_eq!(stats.unique_keys_used, 0);
    }

    #[test]
    fn action_stats_with_data() {
        let actions = vec![
            FrameAction {
                frame: 0,
                timestamp_us: 0,
                keys_held: vec!["KeyW".to_string(), "ShiftLeft".to_string()],
                mouse_buttons_held: vec![],
                mouse_x: 0.0, mouse_y: 0.0,
                mouse_dx: 10.0, mouse_dy: 0.0,
                scroll_dx: 0.0, scroll_dy: 0.0,
            },
            FrameAction {
                frame: 1,
                timestamp_us: 33333,
                keys_held: vec!["KeyW".to_string()],
                mouse_buttons_held: vec![],
                mouse_x: 10.0, mouse_y: 0.0,
                mouse_dx: 0.0, mouse_dy: 0.0,
                scroll_dx: 0.0, scroll_dy: 0.0,
            },
        ];

        let stats = compute_action_stats(&actions);
        assert!((stats.avg_simultaneous_keys - 1.5).abs() < 0.01);
        assert_eq!(stats.peak_simultaneous_keys, 2);
        assert_eq!(stats.unique_keys_used, 2);
        assert_eq!(stats.input_activity_ratio, 1.0); // both frames active
        assert!((stats.peak_mouse_speed - 10.0).abs() < 0.01);
    }

    #[test]
    fn realistic_gameplay_scenario() {
        // Simulate a short FPS gameplay: WASD movement + mouse aiming + shooting
        // At 30fps, frame interval = 1_000_000/30 = 33333us
        // Frame timestamps: 0, 33333, 66666, 99999, 133332, 166665, 199998, 233331, ...
        let events = vec![
            // Player starts moving forward
            key_event(0, "KeyW", true),
            mouse_move(5000, 500.0, 400.0),
            // Player strafes left while moving forward
            key_event(50000, "KeyA", true),
            mouse_move(60000, 520.0, 395.0),
            // Player shoots
            mouse_button(80000, MouseButton::Left, true, 520.0, 395.0),
            mouse_button(90000, MouseButton::Left, false, 520.0, 395.0),
            // Player stops moving
            key_event(150000, "KeyW", false),
            key_event(160000, "KeyA", false),
            // Player crouches (at 199999, within frame 6 at ts=199998)
            key_event(199_998, "ControlLeft", true),
            mouse_move(210000, 530.0, 400.0),
        ];

        // 30fps, 10 frames (~333ms)
        let actions = index_frame_actions(&events, 10, 30, 0);
        assert_eq!(actions.len(), 10);

        // Frame 0 (t=0): W pressed
        assert!(actions[0].keys_held.contains(&"KeyW".to_string()));

        // Frame 2 (t=66666): W+A held, mouse button was pressed and released
        assert!(actions[2].keys_held.contains(&"KeyW".to_string()));
        assert!(actions[2].keys_held.contains(&"KeyA".to_string()));
        assert!(actions[2].mouse_buttons_held.is_empty()); // released at 90000

        // Frame 5 (t=166665): keys released (at 150000 and 160000)
        assert!(actions[5].keys_held.is_empty());

        // Frame 6 (t=199998): ControlLeft pressed at exactly 199998
        assert!(actions[6].keys_held.contains(&"ControlLeft".to_string()));
    }
}
