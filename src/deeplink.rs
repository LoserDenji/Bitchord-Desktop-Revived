//! Jam deep links: `ytm-rs://jam/CODE` and `https://grapedjuice.github.io/ytm-rs/jam/?c=CODE`,
//! and the single-instance hand-off built on the same channel.
//!
//! Clicking a link (or the app's own shortcut, while a copy is already running) starts a
//! new process. [`claim`] runs before anything slow: the first instance takes the
//! per-user pipe (Windows) or Unix socket and serves it; any later one finds it taken,
//! hands over its message and exits, and the running copy brings its window up. The
//! channel is claimed within milliseconds of the process starting, so double-launching
//! the shortcut can no longer open a second window: the second launch waits for the
//! first one's channel and hands itself over. Invites queue in `PENDING` and the window
//! request in `SHOW`; the UI drains them with `take()`/`take_show()` (the same pattern
//! as `tray.rs`), woken through the callback `set_wake` installs once the context exists.

use std::sync::{Arc, Mutex};

use crate::jam::normalize_code as normalize;

static PENDING: Mutex<Vec<Invite>> = Mutex::new(Vec::new());
/// A plain later launch asked for the window (see `forward_show`).
static SHOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Wakes the UI for what a later launch handed over; set once the context exists.
static WAKE: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);
/// What a plain later launch sends instead of a code.
const SHOW_MSG: &str = "show";
/// The updater's relaunch passes this: the old copy may still be exiting, so don't hand over to it.
pub const RELAUNCH_FLAG: &str = "--relaunch";
/// Ask a running copy to bring its window up: `--show`. A plain launch already does
/// this, but the flag says so explicitly — and when nothing is running, the app just
/// starts as usual with its window up. Recovery path for a window hidden behind a
/// tray icon the shell never drew (taskbar replacements can do that).
pub const SHOW_FLAG: &str = "--show";
/// Run with "Close to tray" off for this session, so × quits for real: `--no-tray`.
pub const NO_TRAY_FLAG: &str = "--no-tray";

/// Who holds the hand-off channel after [`claim`].
#[derive(Clone, Copy, PartialEq)]
pub enum Claim {
    /// This launch took it, and serves it.
    First,
    /// Another copy holds it (or is starting up and will): hand over, or take it over
    /// if that copy proves to be gone.
    Busy,
}

impl Claim {
    pub fn busy(self) -> bool {
        self == Claim::Busy
    }
}

/// What a launch argument (or a hand-off message) asked for: a relay jam room, or
/// a Listen Together party (with the custom server its invite names).
#[derive(Clone, Debug, PartialEq)]
pub enum Invite {
    Jam(String),
    Party(String, Option<String>),
}

impl Invite {
    /// Over the hand-off channel: a plain code, or a `party:CODE[|server]` line.
    pub fn message(&self) -> String {
        match self {
            Self::Jam(code) => code.clone(),
            Self::Party(code, None) => format!("party:{code}"),
            Self::Party(code, Some(server)) => format!("party:{code}|{server}"),
        }
    }

    fn from_message(msg: &str) -> Option<Self> {
        if let Some(rest) = msg.strip_prefix("party:") {
            let (code, server) = rest.split_once('|').map(|(c, s)| (c, Some(s))).unwrap_or((rest, None));
            return crate::party::clean_code(code).map(|code| Self::Party(code, server.map(str::to_owned)));
        }
        crate::jam::normalize_code(msg).map(Self::Jam)
    }
}

/// What a launch argument names, if it is a room link:
/// `ytm-rs://jam/CODE[/]` (any case) or the jam share link with `?c=CODE` for a jam,
/// `bitchord://party/CODE` or an `/invite/<CODE>` link for a party.
pub fn parse(arg: &str) -> Option<Invite> {
    let arg = arg.trim().trim_matches('"');
    let (scheme, rest) = arg.split_once("://")?;
    let rest = rest.split('#').next().unwrap_or("");
    if scheme.eq_ignore_ascii_case("ytm-rs") {
        // `jam/CODE`, `jam/CODE/`, or `jam/?c=CODE`.
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let path = path.trim_end_matches('/');
        let (host, code) = path.split_once('/').unwrap_or((path, ""));
        if !host.eq_ignore_ascii_case("jam") {
            return None;
        }
        if !code.is_empty() {
            return normalize(code).map(Invite::Jam);
        }
        return query_param(query, "c").and_then(normalize).map(Invite::Jam);
    }
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("bitchord") {
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let lower = path.to_ascii_lowercase();
        let path = lower.trim_end_matches('/').trim_end_matches("/index.html");
        if path == "grapedjuice.github.io/ytm-rs/jam" {
            return query_param(query, "c").and_then(normalize).map(Invite::Jam);
        }
        // A Listen Together invite: `…/invite/<CODE>` on the origin that hosts it,
        // or `bitchord://party/CODE`, with the custom server it names, if any.
        return crate::party::parse_invite(arg).map(|(code, server)| Invite::Party(code, server));
    }
    None
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then_some(v)
    })
}

/// The first room link among the process arguments.
pub fn from_args() -> Option<Invite> {
    std::env::args_os().skip(1).find_map(|a| parse(&a.to_string_lossy()))
}

/// Queue an invite for the UI (e.g. from the launch arguments).
pub fn push(invite: Invite) {
    PENDING.lock().unwrap().push(invite);
}

/// Invites received since the last call, oldest first.
pub fn take() -> Vec<Invite> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Whether a later launch asked for the window since the last call.
pub fn take_show() -> bool {
    SHOW.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Set the UI's wake-up, once the context exists: what a later launch handed over is
/// only picked up on a pass, and a hidden window repaints rarely.
pub fn set_wake(wake: Arc<dyn Fn() + Send + Sync>) {
    *WAKE.lock().unwrap() = Some(wake);
}

fn wake() {
    if let Some(w) = WAKE.lock().unwrap().as_ref() {
        w();
    }
}

/// A plain launch (or `--show`) while another copy runs: bring that one's window up
/// instead, so only one copy uses (and refreshes) the Google session. Two copies
/// refreshing it apart leave one with old cookies, which Google treats as stolen and
/// signs out. True if a running copy took it.
pub fn forward_show() -> bool {
    if std::env::args_os().any(|a| a == RELAUNCH_FLAG) {
        return false;
    }
    // Windows only lets a background process take the foreground when the one the user
    // just started allows it.
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    forward(SHOW_MSG)
}

/// Per-user channel name. A dev profile (`YTM_DATA_DIR`) gets its own, so test runs
/// never talk to the installed copy.
fn channel_id() -> String {
    let user: String = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let mut name = format!("ytm-rs-jam-{}", if user.is_empty() { "default".to_owned() } else { user });
    if let Some(dir) = std::env::var_os("YTM_DATA_DIR") {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        dir.hash(&mut h);
        name.push_str(&format!("-{:016x}", h.finish()));
    }
    name
}

#[cfg(not(windows))]
mod unix {
    use std::io::{ErrorKind, Read};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    use super::channel_id;

    /// Per-user socket, in the user's runtime dir (`/tmp` with their uid, else).
    pub fn socket_path() -> PathBuf {
        let uid = unsafe { libc::getuid() };
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let name = if dir == PathBuf::from("/tmp") { format!("{}-{}", uid, channel_id()) } else { channel_id() };
        dir.join(format!("{name}.sock"))
    }

    /// Take the channel (bind and serve) or find it taken. A socket file left behind by
    /// a copy that died without cleaning up binds as taken too; the caller notices when
    /// nothing answers and asks [`super::take_over`].
    pub fn bind_and_serve() -> bool {
        let path = socket_path();
        let Ok(listener) = UnixListener::bind(&path) else { return false };
        // A socket in a shared dir is only for its own user to write to.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let spawned = std::thread::Builder::new().name("deeplink".into()).spawn(move || serve(listener));
        if let Err(e) = spawned {
            log::warn!("jam link listener: {e}");
        }
        true
    }

    /// Hand `msg` to whoever serves the socket. `tries`: short, for a holder that's
    /// mid-exit; the channel exists as soon as the holder's process does.
    pub fn try_forward(msg: &str, tries: usize) -> bool {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        for i in 0..tries {
            match UnixStream::connect(socket_path()) {
                Ok(mut stream) => {
                    return stream.write_all(format!("{msg}\n").as_bytes()).and_then(|_| stream.flush()).is_ok();
                }
                // The holder is up but its socket isn't accepting quite yet, or it just
                // died: brief retries tell those apart from "nobody's there" poorly, so
                // only a couple, and only past the first attempt.
                Err(e) if (e.kind() == ErrorKind::ConnectionRefused || e.kind() == ErrorKind::NotFound) && i + 1 < tries => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => return false,
            }
        }
        false
    }

    /// A copy that died left its socket file behind: take the name over, if nothing
    /// answers on it any more.
    pub fn take_over() -> bool {
        if super::forward("show") {
            return false; // Someone is there after all; leave them to it.
        }
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        bind_and_serve()
    }

    fn serve(listener: UnixListener) {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut msg = Vec::new();
            let mut buf = [0u8; 64];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        msg.extend_from_slice(&buf[..n]);
                        if msg.len() > 512 {
                            break;
                        }
                    }
                }
            }
            super::received(String::from_utf8_lossy(&msg).trim().to_owned());
        }
    }
}

/// What a hand-off channel delivered, once it's trimmed of its line ending: the window,
/// or a room to join.
fn received(msg: String) {
    if msg == SHOW_MSG {
        log::info!("another launch asked for the window");
        SHOW.store(true, std::sync::atomic::Ordering::Relaxed);
        wake();
        return;
    }
    match Invite::from_message(&msg) {
        Some(invite) => {
            log::info!("room invite received from another launch: {invite:?}");
            push(invite);
            wake();
        }
        None => log::warn!("room link pipe: ignored an invalid message"),
    }
}

/// Claim the hand-off channel, before anything slow: the first launch takes it and
/// serves it from here on; a later one finds it taken and hands itself over (see
/// `forward`/`forward_show`), falling back to [`take_over`] when the holder turns out
/// to be gone. On Windows the channel is the per-user named pipe, taken exclusively
/// with `FILE_FLAG_FIRST_PIPE_INSTANCE`; elsewhere, a Unix socket by the same idea.
#[cfg(windows)]
pub fn claim() -> Claim {
    if win::serve_pipe() {
        Claim::First
    } else {
        // The name is held by another copy; if it dies without a successor, a retry
        // thread takes the pipe over (an updater relaunch does exactly this).
        win::spawn_pipe_retry();
        Claim::Busy
    }
}

#[cfg(not(windows))]
pub fn claim() -> Claim {
    if unix::bind_and_serve() {
        Claim::First
    } else {
        Claim::Busy
    }
}

/// The holder never answered: it's gone (it may have crashed before claiming). Take
/// the channel over for this launch, so the next one hands over to us. False when
/// someone is serving after all.
#[cfg(not(windows))]
pub fn take_over() -> bool {
    unix::take_over()
}

#[cfg(windows)]
pub fn take_over() -> bool {
    // The retry thread `claim` spawned does this on its own.
    false
}

/// Hand `msg` (a jam code, or the window request) to the copy that serves the channel.
/// True when one took it.
#[cfg(windows)]
pub fn forward(msg: &str) -> bool {
    win::forward(msg)
}

#[cfg(not(windows))]
pub fn forward(msg: &str) -> bool {
    // A few tries: the holder's socket accepts as soon as its process starts, but a
    // copy mid-exit may close it between our claim and this call.
    unix::try_forward(msg, 4)
}

#[cfg(windows)]
mod win {
    use std::io::Write;

    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE};
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, ReadFile};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };
    use windows::Win32::System::Registry::{HKEY, REG_SZ, RegSetKeyValueW};
    use windows::core::HSTRING;

    use super::received;

    fn pipe_name() -> String {
        format!(r"\\.\pipe\{}", super::channel_id())
    }

    /// Create the pipe, if the name is free, and serve it on a background thread.
    pub fn serve_pipe() -> bool {
        let Some(pipe) = create_pipe() else { return false };
        spawn_serve(pipe);
        true
    }

    /// Another copy holds the name: keep asking for it on a background thread, so this
    /// copy becomes the server when the holder dies (an updater relaunch, or a crash).
    pub fn spawn_pipe_retry() {
        let spawned = std::thread::Builder::new().name("deeplink-takeover".into()).spawn(|| {
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if let Some(pipe) = create_pipe() {
                    log::info!("jam link pipe: taken over after the previous holder left");
                    spawn_serve(pipe);
                    return;
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("jam link takeover: {e}");
        }
    }

    fn create_pipe() -> Option<Pipe> {
        let handle = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(pipe_name()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                512,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            // ERROR_ACCESS_DENIED here means another instance already serves it.
            log::info!("jam link pipe not served by this instance: {}", windows::core::Error::from_thread());
            return None;
        }
        Some(Pipe(handle))
    }

    /// Hand `msg` to a running instance. True if one took it.
    pub fn forward(msg: &str) -> bool {
        let name = pipe_name();
        // The server has one pipe instance; between clients it is briefly busy.
        for _ in 0..20 {
            match std::fs::OpenOptions::new().write(true).open(&name) {
                Ok(mut pipe) => {
                    return pipe.write_all(format!("{msg}\n").as_bytes()).and_then(|_| pipe.flush()).is_ok();
                }
                // ERROR_PIPE_BUSY: another client is mid-handoff.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => std::thread::sleep(std::time::Duration::from_millis(50)),
                // No server (ERROR_FILE_NOT_FOUND) or anything else: nobody to hand to.
                Err(_) => return false,
            }
        }
        false
    }

    struct Pipe(HANDLE);
    // SAFETY: a pipe handle may be used from any thread.
    unsafe impl Send for Pipe {}
    impl Drop for Pipe {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn spawn_serve(pipe: Pipe) {
        let spawned = std::thread::Builder::new().name("deeplink".into()).spawn(move || serve(pipe));
        if let Err(e) = spawned {
            log::warn!("jam link listener: {e}");
        }
    }

    fn serve(pipe: Pipe) {
        loop {
            let connected = unsafe { ConnectNamedPipe(pipe.0, None) };
            if let Err(e) = connected {
                // A client that connected between create/disconnect and this call.
                if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                    log::warn!("jam link pipe: {e}");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
            }
            let mut msg = Vec::new();
            let mut buf = [0u8; 64];
            loop {
                let mut n = 0u32;
                let ok = unsafe { ReadFile(pipe.0, Some(&mut buf), Some(&mut n), None) };
                if ok.is_err() || n == 0 {
                    break; // ERROR_BROKEN_PIPE: the client wrote and closed.
                }
                msg.extend_from_slice(&buf[..n as usize]);
                if msg.len() > 256 {
                    break;
                }
            }
            unsafe {
                let _ = DisconnectNamedPipe(pipe.0);
            }
            received(String::from_utf8_lossy(&msg).trim().to_owned());
        }
    }

    /// The registry writer for `ytm-rs:` links (see `register_protocol`).
    pub(super) fn register_at(root: HKEY, key: &str, exe: &str) -> windows::core::Result<()> {
        let set = |sub: &str, name: &str, value: &str| -> windows::core::Result<()> {
            let path = if sub.is_empty() { key.to_string() } else { format!(r"{key}\{sub}") };
            let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
            let name = HSTRING::from(name);
            unsafe {
                RegSetKeyValueW(
                    root,
                    &HSTRING::from(path),
                    if name.is_empty() { windows::core::PCWSTR::null() } else { windows::core::PCWSTR(name.as_ptr()) },
                    REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    (data.len() * 2) as u32,
                )
            }
            .ok()
        };
        set("", "", "URL:ytm-rs Protocol")?;
        set("", "URL Protocol", "")?;
        set("DefaultIcon", "", &format!("\"{exe}\",0"))?;
        set(r"shell\open\command", "", &format!("\"{exe}\" \"%1\""))
    }
}

/// Register the `ytm-rs:` URL protocol for this user, pointing at this exe.
/// Call only from an installed copy, so dev builds don't take the protocol over.
#[cfg(windows)]
pub fn register_protocol() {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = win::register_at(HKEY_CURRENT_USER, r"Software\Classes\ytm-rs", &exe.to_string_lossy()) {
                log::warn!("registering the ytm-rs: protocol: {e}");
            }
        }
        Err(e) => log::warn!("registering the ytm-rs: protocol: {e}"),
    }
}

#[cfg(not(windows))]
pub fn register_protocol() {}

#[cfg(windows)]
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_protocol() {
        assert_eq!(parse("ytm-rs://jam/ABC234"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("ytm-rs://jam/abc234/"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("YTM-RS://JAM/aBc234"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("\"ytm-rs://jam/ABC234\""), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("ytm-rs://jam/?c=xyz789"), Some(Invite::Jam("XYZ789".into())));
        assert_eq!(parse("bitchord://party/XYZ789"), Some(Invite::Party("XYZ789".into(), None)));
        assert_eq!(
            parse("https://bitchord.kushagrasingh.in/invite/xyz789?server=party.example.com"),
            Some(Invite::Party("XYZ789".into(), Some("https://party.example.com".into())))
        );
    }

    #[test]
    fn messages() {
        assert_eq!(Invite::from_message("ABC234"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(Invite::from_message("party:XYZ789"), Some(Invite::Party("XYZ789".into(), None)));
        assert_eq!(Invite::Party("XYZ789".into(), Some("https://p.example.com".into())).message(), "party:XYZ789|https://p.example.com");
        assert_eq!(Invite::from_message("party:XYZ789|https://p.example.com"), Some(Invite::Party("XYZ789".into(), Some("https://p.example.com".into()))));
        assert_eq!(Invite::from_message("party:XYZ78"), None);
        assert_eq!(Invite::from_message("nonsense"), None);
    }

    #[test]
    fn share_link() {
        assert_eq!(parse("https://grapedjuice.github.io/ytm-rs/jam/?c=ABC234"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("https://grapedjuice.github.io/ytm-rs/jam?c=abc234"), Some(Invite::Jam("ABC234".into())));
        assert_eq!(parse("https://grapedjuice.github.io/ytm-rs/jam/index.html?x=1&c=abc234#top"), Some(Invite::Jam("ABC234".into())));
    }

    #[test]
    fn rejects() {
        for bad in [
            "",
            "%1",
            "ABC234",
            "ytm-rs://jam/",
            "ytm-rs://jam/ABC23",      // too short
            "ytm-rs://jam/ABC2345",    // too long
            "ytm-rs://jam/ABC0I1",     // 0, I, 1 are not in the alphabet
            "ytm-rs://jam/ABC234/xyz", // extra path
            "ytm-rs://room/ABC234",
            "https://example.com/ytm-rs/jam/?c=ABC234",
            "https://grapedjuice.github.io/ytm-rs/?c=ABC234",
            "https://grapedjuice.github.io/ytm-rs/jam/",
            "--login",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn queue() {
        push(Invite::Jam("ABC234".into()));
        push(Invite::Party("XYZ789".into(), None));
        assert_eq!(take(), [Invite::Jam("ABC234".into()), Invite::Party("XYZ789".into(), None)]);
        assert!(take().is_empty());
    }

    #[test]
    fn channel_name_is_per_user() {
        let name = channel_id();
        assert!(name.starts_with("ytm-rs-jam-"), "{name}");
        // The Windows pipe form keeps its prefix; the Unix socket form is a plain name.
        assert!(!name.contains('\\'));
    }

    #[cfg(windows)]
    #[test]
    fn register_under_scratch_key() {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW};
        use windows::core::HSTRING;
        let key = format!(r"Software\ytm-rs-deeplink-test-{}", std::process::id());
        // `register_at` lives in `mod win` (`pub(super)`): reachable from here only
        // through the module path, not via `use super::*`.
        win::register_at(HKEY_CURRENT_USER, &key, r"C:\x\ytm-rs.exe").unwrap();
        let read = |sub: &str, name: &str| -> String {
            let mut buf = [0u16; 256];
            let mut len = (buf.len() * 2) as u32;
            let path = if sub.is_empty() { key.clone() } else { format!(r"{key}\{sub}") };
            let r = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(path),
                    &HSTRING::from(name),
                    RRF_RT_REG_SZ,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    Some(&mut len),
                )
            };
            assert!(r.is_ok(), "{sub}\\{name}: {r:?}");
            String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)])
        };
        let results = (read("", ""), read("", "URL Protocol"), read("DefaultIcon", ""), read(r"shell\open\command", ""));
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
            let _ = windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
        }
        assert_eq!(results.0, "URL:ytm-rs Protocol");
        assert_eq!(results.1, "");
        assert_eq!(results.2, r#""C:\x\ytm-rs.exe",0"#);
        assert_eq!(results.3, r#""C:\x\ytm-rs.exe" "%1""#);
    }
}
