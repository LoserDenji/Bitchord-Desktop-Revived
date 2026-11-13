//! Launch at login, the way the tray's "Launch at login" item (and Settings' own
//! switch) toggle it. Like ForkChord's, the OS keeps the state and nothing is stored
//! in the app's settings: the registry holds it on Windows, an XDG autostart entry
//! on Linux, and a launch agent on macOS — so any launcher that looks shows the truth.

/// The value name in the Run key (also the .desktop entry's file name and the
/// launch agent's Label).
const NAME: &str = "BitChord";

/// Whether the OS would start BitChord at login.
pub fn enabled() -> bool {
    #[cfg(windows)]
    {
        run_value().is_some()
    }
    #[cfg(target_os = "linux")]
    {
        entry().is_some_and(|p| p.is_file())
    }
    #[cfg(target_os = "macos")]
    {
        plist().is_some_and(|p| p.is_file())
    }
}

/// Turns launching at login on or off; errors are logged, not shown (the tick and
/// the switch read the OS back, so a failed write just doesn't move them).
pub fn set(on: bool) {
    #[cfg(windows)]
    win::set(on);
    #[cfg(target_os = "linux")]
    {
        match entry() {
            Some(path) if !on => {
                if let Err(e) = std::fs::remove_file(&path) {
                    log::warn!("launch at login (autostart entry): {e}");
                }
            }
            Some(path) if on => {
                if let Err(e) = std::fs::write(&path, desktop_entry()) {
                    log::warn!("launch at login (autostart entry): {e}");
                }
            }
            _ => {}
        }
    }
    #[cfg(target_os = "macos")]
    {
        match plist() {
            Some(path) if !on => {
                let _ = std::process::Command::new("launchctl").args(["unload", "-w"]).arg(&path).status();
                if let Err(e) = std::fs::remove_file(&path) {
                    log::warn!("launch at login (launch agent): {e}");
                }
            }
            Some(path) if on => {
                if let Err(e) = std::fs::write(&path, agent()) {
                    log::warn!("launch at login (launch agent): {e}");
                    return;
                }
                if let Err(e) = std::process::Command::new("launchctl").args(["load", "-w"]).arg(&path).status() {
                    log::warn!("launchctl load: {e}");
                }
            }
            _ => {}
        }
    }
}

/// Flips the OS state, answering what it now is.
pub fn toggle() -> bool {
    let on = !enabled();
    set(on);
    on
}

/// This exe, which the OS is told to start. Quoted, for the Windows Run key.
#[cfg(windows)]
fn command() -> Option<String> {
    std::env::current_exe().ok().map(|p| format!("\"{}\"", p.display()))
}

// ------------------------------------------------------------------- Windows

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The Run key's value for this app, when one is written.
#[cfg(windows)]
fn run_value() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &windows::core::HSTRING::from(RUN_KEY),
            &windows::core::HSTRING::from(NAME),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if ok.is_err() {
        return None;
    }
    let len = (size as usize) / 2;
    let s = String::from_utf16_lossy(&buf[..len.min(buf.len())]);
    let s = s.trim_end_matches('\0').trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteValueW, RegSetKeyValueW};
    use windows::core::{HSTRING, PCWSTR};

    use super::{NAME, RUN_KEY, command};

    pub fn set(on: bool) {
        let exe = match command() {
            Some(exe) => exe,
            None => {
                log::warn!("launch at login: this exe's path is unknown");
                return;
            }
        };
        let key = HSTRING::from(RUN_KEY);
        let name = HSTRING::from(NAME);
        let name_ptr = PCWSTR(name.as_ptr());
        let result = if on {
            let data: Vec<u16> = exe.encode_utf16().chain(Some(0)).collect();
            unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &key, name_ptr, REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32) }.ok()
        } else {
            unsafe { RegDeleteValueW(HKEY_CURRENT_USER, &key, name_ptr) }.ok()
        };
        if let Err(e) = result {
            log::warn!("launch at login (registry): {e}");
        }
    }
}

// ------------------------------------------------------------------- Linux

/// The XDG autostart entry's path ($XDG_CONFIG_HOME/autostart, else ~/.config/autostart).
#[cfg(target_os = "linux")]
fn entry() -> Option<std::path::PathBuf> {
    let home = directories::UserDirs::new()?.home_dir().to_path_buf();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    Some(config.join("autostart").join(format!("{NAME}.desktop")))
}

#[cfg(target_os = "linux")]
fn desktop_entry() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=BitChord\n\
         Comment=Native BitChord client\n\
         Exec={exe}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

// ------------------------------------------------------------------- macOS

/// The launch agent's path (~/Library/LaunchAgents).
#[cfg(target_os = "macos")]
fn plist() -> Option<std::path::PathBuf> {
    let home = directories::UserDirs::new()?.home_dir().to_path_buf();
    Some(home.join("Library").join("LaunchAgents").join(format!("com.bitchord.{NAME}.plist")))
}

#[cfg(target_os = "macos")]
fn agent() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key><string>com.bitchord.{NAME}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array><string>{exe}</string></array>\n\
         \x20 <key>RunAtLoad</key><true/>\n\
         </dict>\n\
         </plist>\n"
    )
}
