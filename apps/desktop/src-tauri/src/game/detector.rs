use sysinfo::System;

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
}
