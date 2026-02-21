use crate::game::detector::{game_to_genre, GameGenre};
use crate::input::{InputEvent, InputEventKind};

use super::frame_actions::{compute_action_stats, index_frame_actions};
use super::types::FrameAction;
use super::types::{DimensionScores, HighlightSegment, QualityScore};

/// Genre-specific weight profile for computing the overall quality score.
///
/// Each weight corresponds to a dimension in `DimensionScores`. Weights sum to 1.0.
/// These profiles encode what model providers value most for each game type,
/// derived from the demand side: what makes training data valuable depends on
/// what the model is trying to learn from that genre.
struct GenreWeights {
    action_density: f64,
    input_continuity: f64,
    input_diversity: f64,
    mouse_control: f64,
    action_complexity: f64,
    highlight_density: f64,
}

impl GenreWeights {
    fn for_genre(genre: GameGenre) -> Self {
        match genre {
            // FPS: Providers want aim precision data, combat sequences, fast reactions.
            // High weight on mouse_control (aim) and highlight_density (combat moments).
            GameGenre::Fps => GenreWeights {
                action_density: 0.20,
                input_continuity: 0.10,
                input_diversity: 0.15,
                mouse_control: 0.20,
                action_complexity: 0.15,
                highlight_density: 0.20,
            },
            // MOBA: Providers want strategic clicking, ability combos, hotkey diversity.
            // High weight on input_diversity (many abilities) and action_complexity (combos).
            GameGenre::Moba => GenreWeights {
                action_density: 0.15,
                input_continuity: 0.15,
                input_diversity: 0.25,
                mouse_control: 0.10,
                action_complexity: 0.20,
                highlight_density: 0.15,
            },
            // Racing/Driving: Providers want sustained control signals, physics, continuous
            // steering. Few keys is NORMAL and expected — don't penalize for it.
            // High weight on input_continuity (sustained throttle/steering) and
            // mouse_control (smooth steering is the primary signal).
            GameGenre::Racing => GenreWeights {
                action_density: 0.05,
                input_continuity: 0.35,
                input_diversity: 0.05,
                mouse_control: 0.30,
                action_complexity: 0.05,
                highlight_density: 0.20,
            },
            // Open World: Providers want diverse gameplay modes (exploration + combat).
            // Balanced weights reflecting the mix of activities.
            GameGenre::OpenWorld => GenreWeights {
                action_density: 0.15,
                input_continuity: 0.20,
                input_diversity: 0.20,
                mouse_control: 0.15,
                action_complexity: 0.15,
                highlight_density: 0.15,
            },
            // Survival/Sandbox: Providers want building, crafting, exploration, some combat.
            // High weight on input_continuity (sustained building) and input_diversity
            // (many different block/item interactions).
            GameGenre::Survival => GenreWeights {
                action_density: 0.10,
                input_continuity: 0.25,
                input_diversity: 0.25,
                mouse_control: 0.15,
                action_complexity: 0.10,
                highlight_density: 0.15,
            },
            // RPG/Strategy: Providers want complex decision-making, ability usage.
            // High weight on input_diversity (many abilities) and action_complexity
            // (complex multi-key sequences).
            GameGenre::Rpg => GenreWeights {
                action_density: 0.10,
                input_continuity: 0.15,
                input_diversity: 0.30,
                mouse_control: 0.10,
                action_complexity: 0.25,
                highlight_density: 0.10,
            },
            // Unknown: Balanced default weights. No single dimension dominates.
            GameGenre::Unknown => GenreWeights {
                action_density: 0.15,
                input_continuity: 0.20,
                input_diversity: 0.15,
                mouse_control: 0.15,
                action_complexity: 0.15,
                highlight_density: 0.20,
            },
        }
    }

    fn weighted_score(&self, dims: &DimensionScores) -> f64 {
        let score = dims.action_density * self.action_density
            + dims.input_continuity * self.input_continuity
            + dims.input_diversity * self.input_diversity
            + dims.mouse_control * self.mouse_control
            + dims.action_complexity * self.action_complexity
            + dims.highlight_density * self.highlight_density;
        score.clamp(0.0, 1.0)
    }
}

/// Score a clip's quality for world model training, weighted by game genre.
///
/// Computes genre-agnostic dimension scores, then applies genre-specific
/// weights to produce the `overall_score`. Providers can ignore the weighted
/// score and re-weight `dimension_scores` for their specific use case.
pub fn score_clip_quality(
    input_events: &[InputEvent],
    duration_secs: f64,
    fps: u32,
    first_frame_timestamp_us: u64,
    game_name: Option<&str>,
) -> QualityScore {
    let genre = game_to_genre(game_name);

    if input_events.is_empty() || duration_secs <= 0.0 {
        return QualityScore {
            overall_score: 0.0,
            genre: genre.as_str().to_string(),
            dimension_scores: DimensionScores::default(),
            action_density: 0.0,
            input_activity_ratio: 0.0,
            avg_simultaneous_keys: 0.0,
            peak_simultaneous_keys: 0,
            avg_mouse_speed: 0.0,
            peak_mouse_speed: 0.0,
            unique_keys_used: 0,
            input_continuity: 0.0,
            mouse_control_smoothness: 0.0,
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

    // Scale mouse speed to pixels/second
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

    // Compute new metrics
    let input_continuity = compute_input_continuity(input_events, duration_secs);
    let mouse_control_smoothness = compute_mouse_control(&frame_actions);

    // Compute normalized dimension scores (genre-agnostic)
    let dimension_scores = DimensionScores {
        action_density: (action_density / 60.0).min(1.0),
        input_continuity,
        input_diversity: (stats.unique_keys_used as f64 / 15.0).min(1.0),
        mouse_control: mouse_control_smoothness,
        action_complexity: (stats.avg_simultaneous_keys / 3.0).min(1.0),
        highlight_density: if duration_secs > 0.0 {
            ((highlights.len() as f64 / duration_secs) * 60.0 / 10.0).min(1.0)
        } else {
            0.0
        },
    };

    // Apply genre-specific weights
    let weights = GenreWeights::for_genre(genre);
    let overall_score = weights.weighted_score(&dimension_scores);

    QualityScore {
        overall_score,
        genre: genre.as_str().to_string(),
        dimension_scores,
        action_density,
        input_activity_ratio: stats.input_activity_ratio,
        avg_simultaneous_keys: stats.avg_simultaneous_keys,
        peak_simultaneous_keys: stats.peak_simultaneous_keys,
        avg_mouse_speed,
        peak_mouse_speed,
        unique_keys_used: stats.unique_keys_used,
        input_continuity,
        mouse_control_smoothness,
        highlights,
        edge_case_flags,
    }
}

/// Compute input continuity: how evenly input events are distributed across time.
///
/// Divides the clip into 1-second buckets and measures the coefficient of variation
/// (std_dev / mean) of event counts per bucket. Low CV = uniform distribution =
/// high continuity. A truck sim with steady W + gentle steering scores high.
/// A clip with one combat burst and lots of idle scores low.
fn compute_input_continuity(events: &[InputEvent], duration_secs: f64) -> f64 {
    if events.is_empty() || duration_secs < 1.0 {
        return 0.0;
    }

    let bucket_count = duration_secs.ceil() as usize;
    if bucket_count == 0 {
        return 0.0;
    }

    let first_ts = events[0].timestamp_us;
    let bucket_duration_us = (duration_secs * 1_000_000.0) as u64 / bucket_count as u64;
    if bucket_duration_us == 0 {
        return 0.0;
    }

    let mut buckets = vec![0u32; bucket_count];
    for event in events {
        let bucket = ((event.timestamp_us.saturating_sub(first_ts)) / bucket_duration_us) as usize;
        let bucket = bucket.min(bucket_count - 1);
        buckets[bucket] += 1;
    }

    let mean = events.len() as f64 / bucket_count as f64;
    if mean < 0.001 {
        return 0.0;
    }

    let variance = buckets
        .iter()
        .map(|&count| (count as f64 - mean).powi(2))
        .sum::<f64>()
        / bucket_count as f64;
    let std_dev = variance.sqrt();
    let cv = std_dev / mean;

    // CV of 0 = perfectly uniform = 1.0. CV of 2+ = very bursty = 0.0.
    (1.0 - cv / 2.0).clamp(0.0, 1.0)
}

/// Compute mouse control smoothness: purposeful, sustained mouse movement.
///
/// Combines mouse activity ratio (are you moving the mouse?) with smoothness
/// (is the movement steady or erratic?). A player carefully aiming or smoothly
/// steering scores high. Idle mouse or wild flicking scores lower.
fn compute_mouse_control(frame_actions: &[FrameAction]) -> f64 {
    if frame_actions.is_empty() {
        return 0.0;
    }

    let speeds: Vec<f64> = frame_actions
        .iter()
        .map(|a| (a.mouse_dx.powi(2) + a.mouse_dy.powi(2)).sqrt())
        .collect();

    let active_frames = speeds.iter().filter(|&&s| s > 0.5).count();
    let activity_ratio = active_frames as f64 / frame_actions.len() as f64;

    if active_frames < 2 {
        return 0.0;
    }

    // Compute smoothness from active frames only
    let active_speeds: Vec<f64> = speeds.iter().copied().filter(|&s| s > 0.5).collect();
    let mean_speed = active_speeds.iter().sum::<f64>() / active_speeds.len() as f64;

    if mean_speed < 0.001 {
        return 0.0;
    }

    let variance = active_speeds
        .iter()
        .map(|&s| (s - mean_speed).powi(2))
        .sum::<f64>()
        / active_speeds.len() as f64;
    let cv = variance.sqrt() / mean_speed;

    // Low CV = smooth sustained movement = high smoothness.
    // CV of 3+ = very erratic = 0 smoothness.
    let smoothness = (1.0f64 - cv / 3.0).clamp(0.0, 1.0);

    // Need both activity and smoothness for a high score.
    (activity_ratio * 0.5 + smoothness * 0.5).clamp(0.0, 1.0)
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
        let score = score_clip_quality(&[], 10.0, 30, 0, None);
        assert_eq!(score.overall_score, 0.0);
        assert_eq!(score.action_density, 0.0);
        assert!(score.highlights.is_empty());
        assert_eq!(score.genre, "unknown");
    }

    #[test]
    fn zero_duration_score_zero() {
        let events = vec![key_event(0, "KeyW", true)];
        let score = score_clip_quality(&events, 0.0, 30, 0, None);
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

        let score = score_clip_quality(&events, 3.0, 30, 0, Some("Counter-Strike 2"));

        assert!(score.overall_score > 0.3, "active gameplay should score > 0.3, got {}", score.overall_score);
        assert!(score.action_density > 50.0);
        assert!(score.input_activity_ratio > 0.5);
        assert_eq!(score.genre, "fps");
    }

    #[test]
    fn idle_gameplay_scores_low() {
        // Only a single key press in 10 seconds
        let events = vec![
            key_event(0, "KeyW", true),
            key_event(10_000_000, "KeyW", false),
        ];

        let score = score_clip_quality(&events, 10.0, 30, 0, None);
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

        let score = score_clip_quality(&events, 6.0, 30, 0, None);
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

        let score = score_clip_quality(&events, 2.0, 30, 0, None);
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

        let score = score_clip_quality(&events, 1.0, 30, 0, None);
        assert!(score.overall_score >= 0.0 && score.overall_score <= 1.0);
    }

    // --- Genre-specific scoring tests ---

    #[test]
    fn genre_affects_scoring() {
        // Same input data, different game names → different overall scores
        let mut events = Vec::new();
        for i in 0..60u64 {
            let ts = i * 16_667;
            events.push(key_event(ts, "KeyW", true));
            events.push(mouse_move(ts + 5000, 100.0 + i as f64 * 3.0, 200.0));
        }

        let fps_score = score_clip_quality(&events, 1.0, 30, 0, Some("Counter-Strike 2"));
        let racing_score = score_clip_quality(&events, 1.0, 30, 0, Some("Rocket League"));
        let unknown_score = score_clip_quality(&events, 1.0, 30, 0, None);

        assert_eq!(fps_score.genre, "fps");
        assert_eq!(racing_score.genre, "racing");
        assert_eq!(unknown_score.genre, "unknown");

        // The raw dimension_scores should be identical regardless of genre
        assert_eq!(fps_score.dimension_scores, racing_score.dimension_scores);
        assert_eq!(fps_score.dimension_scores, unknown_score.dimension_scores);

        // But overall_score will differ due to different genre weights
        // (exact values depend on the input, but they should not all be equal)
        assert!(
            !(fps_score.overall_score == racing_score.overall_score
                && racing_score.overall_score == unknown_score.overall_score),
            "genre weighting should produce different overall scores for \
             at least some genre pairs"
        );
    }

    #[test]
    fn sustained_input_scores_high_continuity() {
        // Simulate steady input: evenly distributed events over 5 seconds
        let mut events = Vec::new();
        for i in 0..100u64 {
            // 20 events/sec, evenly spaced
            let ts = i * 50_000; // every 50ms
            events.push(key_event(ts, "KeyW", true));
        }

        let score = score_clip_quality(&events, 5.0, 30, 0, None);
        assert!(
            score.input_continuity > 0.6,
            "steady input should have high continuity, got {}",
            score.input_continuity
        );
        assert!(
            score.dimension_scores.input_continuity > 0.6,
            "dimension score should match"
        );
    }

    #[test]
    fn bursty_input_scores_low_continuity() {
        // All events packed into the first second of a 10-second clip
        let mut events = Vec::new();
        for i in 0..50u64 {
            let ts = i * 20_000; // 50 events in first second
            events.push(key_event(ts, "KeyW", true));
        }
        // Add a single event at the end to establish 10s duration
        events.push(key_event(10_000_000, "KeyW", false));

        let score = score_clip_quality(&events, 10.0, 30, 0, None);
        assert!(
            score.input_continuity < 0.5,
            "bursty input should have low continuity, got {}",
            score.input_continuity
        );
    }

    #[test]
    fn smooth_mouse_scores_high_control() {
        // Simulate smooth, sustained mouse movement (like steering)
        let mut events = Vec::new();
        for i in 0..90u64 {
            let ts = i * 33_333; // ~30fps
            // Smooth linear mouse movement
            events.push(mouse_move(ts, 100.0 + i as f64 * 5.0, 200.0 + i as f64 * 2.0));
        }

        let score = score_clip_quality(&events, 3.0, 30, 0, None);
        assert!(
            score.mouse_control_smoothness > 0.3,
            "smooth mouse movement should have high control, got {}",
            score.mouse_control_smoothness
        );
    }

    #[test]
    fn no_mouse_scores_zero_control() {
        // Only keyboard events
        let events = vec![
            key_event(0, "KeyW", true),
            key_event(1_000_000, "KeyW", false),
        ];

        let score = score_clip_quality(&events, 1.0, 30, 0, None);
        assert_eq!(
            score.mouse_control_smoothness, 0.0,
            "no mouse input should have zero mouse control"
        );
    }

    #[test]
    fn racing_genre_rewards_sustained_steering() {
        // Simulate truck driving: hold W + gentle sustained mouse steering
        let mut events = Vec::new();
        events.push(key_event(0, "KeyW", true)); // hold W
        for i in 0..300u64 {
            let ts = i * 33_333; // ~30fps over 10 seconds
            // Small, consistent mouse movements (gentle steering)
            let angle = i as f64 * 0.05;
            events.push(mouse_move(
                ts + 5000,
                500.0 + angle.sin() * 20.0,
                400.0 + angle.cos() * 5.0,
            ));
        }
        events.push(key_event(10_000_000, "KeyW", false));

        let racing_score = score_clip_quality(&events, 10.0, 30, 0, Some("Rocket League"));
        let fps_score = score_clip_quality(&events, 10.0, 30, 0, Some("Counter-Strike 2"));

        // Racing weights should reward sustained + smooth mouse more than FPS weights
        assert!(
            racing_score.overall_score > fps_score.overall_score,
            "racing weights should rate sustained steering higher than FPS weights: \
             racing={:.3}, fps={:.3}",
            racing_score.overall_score,
            fps_score.overall_score
        );
    }

    #[test]
    fn fps_genre_rewards_combat_bursts() {
        // Simulate FPS combat: intense bursts followed by quiet repositioning.
        // This is bursty (low continuity) with diverse keys and highlights —
        // exactly what FPS weights should reward over racing weights.
        let mut events = Vec::new();
        let keys = ["KeyW", "KeyA", "KeyS", "KeyD", "ShiftLeft", "Space",
                     "KeyR", "KeyE", "KeyQ", "KeyF"];

        // Combat burst 1 (0-1s): intense firefight
        for i in 0..60u64 {
            let ts = i * 16_000;
            events.push(key_event(ts, keys[i as usize % keys.len()], i % 2 == 0));
            events.push(mouse_move(ts + 2000, 500.0 + (i as f64 * 7.0) % 200.0, 400.0));
            events.push(mouse_click(ts + 5000, true, 500.0, 400.0));
            events.push(mouse_click(ts + 8000, false, 500.0, 400.0));
        }

        // Quiet period (1-4s): repositioning, just holding W
        events.push(key_event(1_000_000, "KeyW", true));
        events.push(mouse_move(2_000_000, 600.0, 400.0));
        events.push(key_event(4_000_000, "KeyW", false));

        // Combat burst 2 (4-5s): another firefight
        for i in 0..60u64 {
            let ts = 4_000_000 + i * 16_000;
            events.push(key_event(ts, keys[i as usize % keys.len()], i % 2 == 0));
            events.push(mouse_move(ts + 2000, 600.0 + (i as f64 * 5.0) % 150.0, 350.0));
            events.push(mouse_click(ts + 5000, true, 600.0, 350.0));
            events.push(mouse_click(ts + 8000, false, 600.0, 350.0));
        }

        let fps_score = score_clip_quality(&events, 5.0, 30, 0, Some("Counter-Strike 2"));
        let racing_score = score_clip_quality(&events, 5.0, 30, 0, Some("Rocket League"));

        // FPS weights should reward bursty combat with diverse keys + highlights
        // more than racing weights (which reward continuity + smooth steering).
        assert!(
            fps_score.overall_score > racing_score.overall_score,
            "FPS weights should rate bursty combat higher than racing weights: \
             fps={:.3}, racing={:.3}",
            fps_score.overall_score,
            racing_score.overall_score
        );
    }

    #[test]
    fn dimension_scores_all_bounded() {
        let mut events = Vec::new();
        for i in 0..200u64 {
            let ts = i * 5_000;
            events.push(key_event(ts, "KeyW", i % 2 == 0));
            events.push(mouse_move(ts + 1000, i as f64 * 3.0, 200.0));
        }

        let score = score_clip_quality(&events, 1.0, 30, 0, None);
        let dims = &score.dimension_scores;

        assert!(dims.action_density >= 0.0 && dims.action_density <= 1.0);
        assert!(dims.input_continuity >= 0.0 && dims.input_continuity <= 1.0);
        assert!(dims.input_diversity >= 0.0 && dims.input_diversity <= 1.0);
        assert!(dims.mouse_control >= 0.0 && dims.mouse_control <= 1.0);
        assert!(dims.action_complexity >= 0.0 && dims.action_complexity <= 1.0);
        assert!(dims.highlight_density >= 0.0 && dims.highlight_density <= 1.0);
    }

    #[test]
    fn input_continuity_uniform_distribution() {
        let continuity = compute_input_continuity(
            &(0..100).map(|i| key_event(i * 50_000, "KeyW", true)).collect::<Vec<_>>(),
            5.0,
        );
        // Perfectly uniform: ~20 events per 1-second bucket
        assert!(continuity > 0.8, "uniform distribution should score > 0.8, got {}", continuity);
    }

    #[test]
    fn input_continuity_single_burst() {
        let mut events: Vec<InputEvent> = (0..50)
            .map(|i| key_event(i * 10_000, "KeyW", true))
            .collect();
        events.push(key_event(5_000_000, "KeyW", false)); // end marker

        let continuity = compute_input_continuity(&events, 5.0);
        assert!(continuity < 0.5, "single burst should score < 0.5, got {}", continuity);
    }

    #[test]
    fn input_continuity_empty() {
        assert_eq!(compute_input_continuity(&[], 5.0), 0.0);
    }

    #[test]
    fn mouse_control_no_frames() {
        assert_eq!(compute_mouse_control(&[]), 0.0);
    }

    #[test]
    fn genre_weights_sum_to_one() {
        let genres = [
            GameGenre::Fps, GameGenre::Moba, GameGenre::Racing,
            GameGenre::OpenWorld, GameGenre::Survival, GameGenre::Rpg,
            GameGenre::Unknown,
        ];

        for genre in genres {
            let w = GenreWeights::for_genre(genre);
            let sum = w.action_density + w.input_continuity + w.input_diversity
                + w.mouse_control + w.action_complexity + w.highlight_density;
            assert!(
                (sum - 1.0).abs() < 0.001,
                "weights for {:?} should sum to 1.0, got {}",
                genre,
                sum
            );
        }
    }
}
