use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Genre classification for games, used to apply appropriate quality scoring weights.
///
/// Different genres have fundamentally different input patterns. An FPS player
/// generates dense mouse movement + combat bursts, while a truck driving game
/// generates sustained low-frequency steering input. Both are valuable training
/// data, but they need different quality criteria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameGenre {
    /// First-person/third-person shooters: CS2, Valorant, Apex, etc.
    /// High value: aim precision, combat bursts, fast reactions.
    Fps,
    /// MOBAs: League of Legends, Dota 2.
    /// High value: ability combos, strategic clicking, hotkey diversity.
    Moba,
    /// Racing and driving games: Rocket League, truck sims.
    /// High value: sustained steering, throttle control, smooth input.
    Racing,
    /// Open world and action RPGs: GTA V, Cyberpunk, Elden Ring.
    /// High value: diverse gameplay modes, exploration + combat mix.
    OpenWorld,
    /// Survival and sandbox: Minecraft, Palworld, Lethal Company.
    /// High value: building, crafting, exploration, varied interactions.
    Survival,
    /// Turn-based or menu-heavy RPGs: Baldur's Gate 3.
    /// High value: complex decision-making, diverse ability usage.
    Rpg,
    /// Unknown or unrecognized game. Uses balanced default weights.
    Unknown,
}

impl GameGenre {
    pub fn as_str(&self) -> &'static str {
        match self {
            GameGenre::Fps => "fps",
            GameGenre::Moba => "moba",
            GameGenre::Racing => "racing",
            GameGenre::OpenWorld => "open_world",
            GameGenre::Survival => "survival",
            GameGenre::Rpg => "rpg",
            GameGenre::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for GameGenre {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Map a detected game display name to its genre.
///
/// Uses the display names from `KNOWN_GAMES` (e.g., "Counter-Strike 2", not "cs2").
/// Returns `GameGenre::Unknown` for unrecognized games.
pub fn game_to_genre(game_name: Option<&str>) -> GameGenre {
    let Some(name) = game_name else {
        return GameGenre::Unknown;
    };

    match name {
        // FPS / Shooters
        "Counter-Strike 2"
        | "Counter-Strike: Global Offensive"
        | "Valorant"
        | "Fortnite"
        | "Apex Legends"
        | "Overwatch 2"
        | "Destiny 2"
        | "Call of Duty: Warzone"
        | "Call of Duty"
        | "Rainbow Six Siege"
        | "PUBG: Battlegrounds"
        | "Helldivers 2" => GameGenre::Fps,

        // MOBAs
        "League of Legends" | "Dota 2" => GameGenre::Moba,

        // Racing / Driving
        "Rocket League" => GameGenre::Racing,

        // Open World / Action RPG
        "Grand Theft Auto V" | "Elden Ring" | "Cyberpunk 2077" => GameGenre::OpenWorld,

        // Survival / Sandbox
        "Minecraft" | "Palworld" | "Lethal Company" | "Palia" => GameGenre::Survival,

        // RPG / Strategy
        "Baldur's Gate 3" => GameGenre::Rpg,

        _ => GameGenre::Unknown,
    }
}

/// Known game process names mapped to display names.
const KNOWN_GAMES: &[(&str, &str)] = &[
    ("cs2", "Counter-Strike 2"),
    ("csgo", "Counter-Strike: Global Offensive"),
    ("valorant", "Valorant"),
    ("valorant-win64-shipping", "Valorant"),
    ("fortnite", "Fortnite"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("apex_legends", "Apex Legends"),
    ("r5apex", "Apex Legends"),
    ("overwatch", "Overwatch 2"),
    ("leagueoflegends", "League of Legends"),
    ("league of legends", "League of Legends"),
    ("dota2", "Dota 2"),
    ("rocketleague", "Rocket League"),
    ("minecraft", "Minecraft"),
    ("gta5", "Grand Theft Auto V"),
    ("gtav", "Grand Theft Auto V"),
    ("eldenring", "Elden Ring"),
    ("cyberpunk2077", "Cyberpunk 2077"),
    ("baldursgate3", "Baldur's Gate 3"),
    ("bg3", "Baldur's Gate 3"),
    ("palworld", "Palworld"),
    ("helldivers2", "Helldivers 2"),
    ("lethal company", "Lethal Company"),
    ("palia", "Palia"),
    ("destiny2", "Destiny 2"),
    ("warzone", "Call of Duty: Warzone"),
    ("cod", "Call of Duty"),
    ("rainbowsix", "Rainbow Six Siege"),
    ("pubg", "PUBG: Battlegrounds"),
    ("tslgame", "PUBG: Battlegrounds"),
];

/// Detect the currently running game by scanning processes.
///
/// On Windows, this first checks the foreground window process,
/// then falls back to scanning all processes against the known games list.
pub fn detect_current_game() -> Option<String> {
    // Try foreground window first (Windows-only)
    #[cfg(target_os = "windows")]
    if let Some(game) = detect_from_foreground_window() {
        return Some(game);
    }

    // Fall back to process scan (cross-platform)
    detect_from_process_list()
}

/// Scan running processes for known game executables.
fn detect_from_process_list() -> Option<String> {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    for process in sys.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        // Strip common extensions
        let name = name
            .strip_suffix(".exe")
            .unwrap_or(&name);

        for &(process_name, display_name) in KNOWN_GAMES {
            if name == process_name {
                return Some(display_name.to_string());
            }
        }
    }

    None
}

/// Detect the game from the foreground window's process (Windows-only).
#[cfg(target_os = "windows")]
fn detect_from_foreground_window() -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::core::PWSTR;

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let query_result = QueryFullProcessImageNameW(process, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut size);

        // Always close the handle, regardless of query success
        let _ = CloseHandle(process);

        query_result.ok()?;

        let path = String::from_utf16_lossy(&buf[..size as usize]);
        let exe_name = path
            .rsplit('\\')
            .next()
            .unwrap_or(&path)
            .to_lowercase();
        let exe_name = exe_name
            .strip_suffix(".exe")
            .unwrap_or(&exe_name);

        for &(process_name, display_name) in KNOWN_GAMES {
            if exe_name == process_name {
                return Some(display_name.to_string());
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_games_list_not_empty() {
        assert!(!KNOWN_GAMES.is_empty());
    }

    #[test]
    fn known_games_have_display_names() {
        for &(process_name, display_name) in KNOWN_GAMES {
            assert!(!process_name.is_empty(), "process name should not be empty");
            assert!(!display_name.is_empty(), "display name should not be empty");
        }
    }

    #[test]
    fn process_list_detection_does_not_panic() {
        // This test just verifies detection doesn't crash.
        // It won't find a game in CI, but should return None gracefully.
        let result = detect_from_process_list();
        // Result is either Some(game_name) or None — both are valid
        let _ = result;
    }

    #[test]
    fn detect_current_game_does_not_panic() {
        let result = detect_current_game();
        let _ = result;
    }

    #[test]
    fn fps_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("Counter-Strike 2")), GameGenre::Fps);
        assert_eq!(game_to_genre(Some("Valorant")), GameGenre::Fps);
        assert_eq!(game_to_genre(Some("Apex Legends")), GameGenre::Fps);
        assert_eq!(game_to_genre(Some("PUBG: Battlegrounds")), GameGenre::Fps);
        assert_eq!(game_to_genre(Some("Helldivers 2")), GameGenre::Fps);
    }

    #[test]
    fn moba_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("League of Legends")), GameGenre::Moba);
        assert_eq!(game_to_genre(Some("Dota 2")), GameGenre::Moba);
    }

    #[test]
    fn racing_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("Rocket League")), GameGenre::Racing);
    }

    #[test]
    fn open_world_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("Grand Theft Auto V")), GameGenre::OpenWorld);
        assert_eq!(game_to_genre(Some("Elden Ring")), GameGenre::OpenWorld);
        assert_eq!(game_to_genre(Some("Cyberpunk 2077")), GameGenre::OpenWorld);
    }

    #[test]
    fn survival_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("Minecraft")), GameGenre::Survival);
        assert_eq!(game_to_genre(Some("Palworld")), GameGenre::Survival);
        assert_eq!(game_to_genre(Some("Lethal Company")), GameGenre::Survival);
    }

    #[test]
    fn rpg_games_classified_correctly() {
        assert_eq!(game_to_genre(Some("Baldur's Gate 3")), GameGenre::Rpg);
    }

    #[test]
    fn unknown_game_returns_unknown() {
        assert_eq!(game_to_genre(None), GameGenre::Unknown);
        assert_eq!(game_to_genre(Some("Some Random Game")), GameGenre::Unknown);
    }

    #[test]
    fn genre_as_str_roundtrips() {
        let genres = [
            GameGenre::Fps, GameGenre::Moba, GameGenre::Racing,
            GameGenre::OpenWorld, GameGenre::Survival, GameGenre::Rpg,
            GameGenre::Unknown,
        ];
        for genre in genres {
            let s = genre.as_str();
            assert!(!s.is_empty());
            let json = serde_json::to_string(&genre).unwrap();
            let deserialized: GameGenre = serde_json::from_str(&json).unwrap();
            assert_eq!(genre, deserialized);
        }
    }

    #[test]
    fn all_known_games_have_genre() {
        // Every game in KNOWN_GAMES should map to a non-Unknown genre
        let mut seen_display_names = std::collections::HashSet::new();
        for &(_, display_name) in KNOWN_GAMES {
            seen_display_names.insert(display_name);
        }
        for name in seen_display_names {
            let genre = game_to_genre(Some(name));
            assert_ne!(
                genre,
                GameGenre::Unknown,
                "known game '{}' should have a genre, got Unknown",
                name
            );
        }
    }
}
