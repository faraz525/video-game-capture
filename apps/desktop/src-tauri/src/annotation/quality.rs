use crate::input::{InputEvent, InputEventKind};

use super::frame_actions::{compute_action_stats, index_frame_actions};
use super::types::{HighlightSegment, QualityScore};

/// Score a clip's quality and interest for world model training.
///
/// Analyzes input events to produce quality metrics that help researchers
/// identify the most valuable training examples. High-quality clips have:
/// - Dense, varied input (not idle/AFK)
/// - Rapid mouse movement (active gameplay, not menus)
/// - Multiple simultaneous keys (complex actions)
/// - Input bursts (highlight moments like fights, clutch plays)
pub fn score_clip_quality(
    input_events: &[InputEvent],
    duration_secs: f64,
    fps: u32,
    first_frame_timestamp_us: u64,
) -> QualityScore {
    if input_events.is_empty() || duration_secs <= 0.0 {
        return QualityScore {
            overall_score: 0.0,
            action_density: 0.0,
            input_activity_ratio: 0.0,
            avg_simultaneous_keys: 0.0,
            peak_simultaneous_keys: 0,
            avg_mouse_speed: 0.0,
            peak_mouse_speed: 0.0,
            unique_keys_used: 0,
            highlights: vec![],
            edge_case_flags: vec![],
        };
    }

    let action_density = input_events.len() as f64 / duration_secs;

    // Generate frame actions for analysis
    let frame_count = (duration_secs * fps as f64).round() as u64;
    let frame_actions = index_frame_actions(
        input_events,
        frame_count,
        fps,
        first_frame_timestamp_us,
    );

    let stats = compute_action_stats(&frame_actions);

    // Scale mouse speed to pixels/second (multiply by fps)
    let avg_mouse_speed = stats.avg_mouse_speed * fps as f64;
    let peak_mouse_speed = stats.peak_mouse_speed * fps as f64;

    // Detect highlights (input bursts)
    let highlights = detect_highlights(input_events, duration_secs);

    // Detect edge cases
    let edge_case_flags = detect_edge_cases(
        &stats,
        avg_mouse_speed,
        peak_mouse_speed,
        action_density,
    );

    // Compute overall score (weighted combination)
    let overall_score = compute_overall_score(
        action_density,
        stats.input_activity_ratio,
        stats.avg_simultaneous_keys,
        avg_mouse_speed,
        stats.unique_keys_used,
        highlights.len(),
    );

    QualityScore {
        overall_score,
        action_density,
        input_activity_ratio: stats.input_activity_ratio,
        avg_simultaneous_keys: stats.avg_simultaneous_keys,
        peak_simultaneous_keys: stats.peak_simultaneous_keys,
        avg_mouse_speed,
        peak_mouse_speed,
        unique_keys_used: stats.unique_keys_used,
        highlights,
        edge_case_flags,
    }
}

/// Detect highlight segments based on input burst analysis.
///
/// A highlight is a window where input density is significantly above the
/// clip average — indicating intense gameplay (fights, clutch plays, etc.).
fn detect_highlights(
    events: &[InputEvent],
    duration_secs: f64,
) -> Vec<HighlightSegment> {
    if events.is_empty() || duration_secs < 1.0 {
        return vec![];
    }

    let avg_density = events.len() as f64 / duration_secs;
    let burst_threshold = (avg_density * 2.0).max(10.0); // 2x average or min 10 events/sec

    let window_us = 1_000_000u64; // 1-second sliding window
    let step_us = 500_000u64; // 0.5-second step

    let first_ts = events[0].timestamp_us;
    let last_ts = events[events.len() - 1].timestamp_us;

    if last_ts <= first_ts {
        return vec![];
    }

    let mut highlights: Vec<HighlightSegment> = Vec::new();
    let mut window_start = first_ts;

    while window_start + window_us <= last_ts + step_us {
        let window_end = window_start + window_us;

        let count = events
            .iter()
            .filter(|e| e.timestamp_us >= window_start && e.timestamp_us < window_end)
            .count();

        let density = count as f64; // events per second (window is 1s)

        if density >= burst_threshold {
            let confidence = ((density / burst_threshold) - 1.0).clamp(0.1, 1.0);

            // Merge with previous highlight if overlapping
            if let Some(last) = highlights.last_mut() {
                if window_start <= last.end_us {
                    last.end_us = window_end;
                    last.confidence = last.confidence.max(confidence);
                    window_start += step_us;
                    continue;
                }
            }

            highlights.push(HighlightSegment {
                start_us: window_start,
                end_us: window_end,
                highlight_type: classify_burst(events, window_start, window_end),
                confidence,
            });
        }

        window_start += step_us;
    }

    highlights
}

/// Classify a burst based on the types of input events within it.
fn classify_burst(events: &[InputEvent], start_us: u64, end_us: u64) -> String {
    let mut key_presses = 0u32;
    let mut mouse_clicks = 0u32;
    let mut mouse_moves = 0u32;

    for event in events.iter().filter(|e| e.timestamp_us >= start_us && e.timestamp_us < end_us) {
        match &event.kind {
            InputEventKind::Key(k) if k.pressed => key_presses += 1,
            InputEventKind::MouseButton(b) if b.pressed => mouse_clicks += 1,
            InputEventKind::MouseMove(_) => mouse_moves += 1,
            _ => {}
        }
    }

    if mouse_clicks >= 3 && mouse_moves >= 5 {
        "combat_burst".to_string()
    } else if mouse_moves >= 10 {
        "rapid_camera_movement".to_string()
    } else if key_presses >= 5 {
        "key_input_burst".to_string()
    } else {
        "input_burst".to_string()
    }
}

/// Detect edge case flags that make clips especially valuable for training.
fn detect_edge_cases(
    stats: &super::frame_actions::ActionStats,
    avg_mouse_speed: f64,
    peak_mouse_speed: f64,
    action_density: f64,
) -> Vec<String> {
    let mut flags = Vec::new();

    if peak_mouse_speed > 2000.0 {
        flags.push("rapid_camera_movement".to_string());
    }

    if stats.peak_simultaneous_keys >= 4 {
        flags.push("complex_input_combo".to_string());
    }

    if action_density > 50.0 {
        flags.push("high_action_density".to_string());
    }

    if stats.unique_keys_used >= 10 {
        flags.push("diverse_key_usage".to_string());
    }

    if avg_mouse_speed > 800.0 {
        flags.push("sustained_fast_aiming".to_string());
    }

    flags
}

/// Compute an overall quality score from component metrics.
///
/// Higher scores indicate more valuable training data:
/// - Active gameplay (not idle)
/// - Complex input (multiple keys, mouse + keyboard)
/// - Varied actions (many different keys used)
/// - Highlight moments (input bursts)
fn compute_overall_score(
    action_density: f64,
    activity_ratio: f64,
    avg_keys: f64,
    avg_mouse_speed: f64,
    unique_keys: u32,
    highlight_count: usize,
) -> f64 {
    // Normalize each component to 0-1 range
    let density_score = (action_density / 60.0).min(1.0); // 60 events/sec = max
    let activity_score = activity_ratio;
    let complexity_score = (avg_keys / 3.0).min(1.0); // 3 simultaneous keys = max
    let mouse_score = (avg_mouse_speed / 1000.0).min(1.0); // 1000 px/sec = max
    let diversity_score = (unique_keys as f64 / 15.0).min(1.0); // 15 unique keys = max
    let highlight_score = (highlight_count as f64 / 5.0).min(1.0); // 5 highlights = max

    // Weighted combination
    let score = density_score * 0.15
        + activity_score * 0.25
        + complexity_score * 0.15
        + mouse_score * 0.15
        + diversity_score * 0.15
        + highlight_score * 0.15;

    // Clamp to 0-1
    score.clamp(0.0, 1.0)
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

    fn mouse_click(ts: u64, pressed: bool, x: f64, y: f64) -> InputEvent {
        InputEvent {
            timestamp_us: ts,
            kind: InputEventKind::MouseButton(MouseButtonEvent {
                button: MouseButton::Left,
                pressed,
                x,
                y,
            }),
        }
    }

    #[test]
    fn empty_events_score_zero() {
        let score = score_clip_quality(&[], 10.0, 30, 0);
        assert_eq!(score.overall_score, 0.0);
        assert_eq!(score.action_density, 0.0);
        assert!(score.highlights.is_empty());
    }

    #[test]
    fn zero_duration_score_zero() {
        let events = vec![key_event(0, "KeyW", true)];
        let score = score_clip_quality(&events, 0.0, 30, 0);
        assert_eq!(score.overall_score, 0.0);
    }

    #[test]
    fn active_gameplay_scores_high() {
        // Simulate active FPS gameplay: lots of WASD + mouse movement + shooting
        let mut events = Vec::new();
        let interval = 16_667u64; // ~60fps timing

        for i in 0..180u64 {
            // 3 seconds at 60 events/sec
            let ts = i * interval;
            events.push(key_event(ts, "KeyW", true));
            events.push(mouse_move(ts + 1000, i as f64 * 5.0, 400.0));
            if i % 10 == 0 {
                events.push(mouse_click(ts + 2000, true, i as f64 * 5.0, 400.0));
                events.push(mouse_click(ts + 5000, false, i as f64 * 5.0, 400.0));
            }
        }
        events.push(key_event(3_000_000, "KeyW", false));

        let score = score_clip_quality(&events, 3.0, 30, 0);

        assert!(score.overall_score > 0.3, "active gameplay should score > 0.3, got {}", score.overall_score);
        assert!(score.action_density > 50.0);
        assert!(score.input_activity_ratio > 0.5);
    }

    #[test]
    fn idle_gameplay_scores_low() {
        // Only a single key press in 10 seconds
        let events = vec![
            key_event(0, "KeyW", true),
            key_event(10_000_000, "KeyW", false),
        ];

        let score = score_clip_quality(&events, 10.0, 30, 0);
        assert!(score.overall_score < 0.5, "idle gameplay should score < 0.5, got {}", score.overall_score);
        assert!(score.action_density < 1.0);
    }

    #[test]
    fn highlights_detected_for_bursts() {
        // Create a burst of events in a small window
        let mut events = Vec::new();

        // Sparse events for first 3 seconds
        events.push(key_event(0, "KeyW", true));
        events.push(mouse_move(1_000_000, 100.0, 100.0));
        events.push(key_event(2_000_000, "KeyW", false));

        // Burst at 3-4 seconds: 20 events in 1 second
        for i in 0..20u64 {
            let ts = 3_000_000 + i * 50_000;
            events.push(mouse_move(ts, 200.0 + i as f64 * 10.0, 300.0));
        }

        // Sparse again
        events.push(key_event(5_000_000, "KeyS", true));
        events.push(key_event(6_000_000, "KeyS", false));

        let score = score_clip_quality(&events, 6.0, 30, 0);
        assert!(!score.highlights.is_empty(), "should detect the burst as a highlight");
    }

    #[test]
    fn edge_cases_detected() {
        // Create events with complex input combos
        let mut events = Vec::new();
        let keys = ["KeyW", "KeyA", "ShiftLeft", "ControlLeft", "Space",
                     "KeyR", "KeyE", "KeyQ", "KeyF", "KeyG", "Key1", "Key2"];

        for (i, key) in keys.iter().enumerate() {
            events.push(key_event(i as u64 * 1000, key, true));
        }
        for (i, key) in keys.iter().enumerate() {
            events.push(key_event(1_000_000 + i as u64 * 1000, key, false));
        }

        let score = score_clip_quality(&events, 2.0, 30, 0);
        assert!(score.unique_keys_used >= 10);
        assert!(score.peak_simultaneous_keys >= 4);
        assert!(score.edge_case_flags.contains(&"complex_input_combo".to_string()));
        assert!(score.edge_case_flags.contains(&"diverse_key_usage".to_string()));
    }

    #[test]
    fn overall_score_bounded() {
        let events: Vec<InputEvent> = (0..1000)
            .map(|i| key_event(i * 1000, "KeyW", i % 2 == 0))
            .collect();

        let score = score_clip_quality(&events, 1.0, 30, 0);
        assert!(score.overall_score >= 0.0 && score.overall_score <= 1.0);
    }
}
