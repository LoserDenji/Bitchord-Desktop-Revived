//! "Sign in with Google" and "Sign in to Spotify" windows.
//!
//! Opens the service's normal sign-in page in the system webview (WebView2 on Windows,
//! WKWebView on macOS, WebKitGTK on Linux), waits until the flow lands on the app's
//! site, then hands the session cookies (HttpOnly ones included) to the app.
//!
//! The window runs in a child process (`ytm-rs --login <dir>`): tao and winit crash
//! when both pump windows in one process (STATUS_FATAL_USER_CALLBACK_EXCEPTION), and a
//! separate process also returns all webview memory to the OS once sign-in ends.

use std::path::PathBuf;

use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebContext, WebViewBuilder};

/// Which sign-in.
#[derive(Clone, Copy)]
pub enum Site {
    /// youtube.com, for YouTube Music.
    Google,
    /// open.spotify.com, for the Spotify transfer.
    Spotify,
    /// discord.com, for Rich Presence through the linked account (fresh-iframe
    /// token capture, like ForkChord's agent shim).
    Discord,
}

impl Site {
    fn flag(self) -> &'static str {
        match self {
            Site::Google => FLAG,
            Site::Spotify => SPOTIFY_FLAG,
            Site::Discord => DISCORD_FLAG,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Site::Google => "Sign in to YouTube Music",
            Site::Spotify => "Sign in to Spotify",
            Site::Discord => "Connect Discord",
        }
    }

    fn start(self) -> &'static str {
        match self {
            Site::Google => "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F",
            Site::Spotify => "https://accounts.spotify.com/en/login?continue=https%3A%2F%2Fopen.spotify.com%2F",
            Site::Discord => "https://discord.com/login",
        }
    }

    /// Where the flow lands once signed in.
    fn home(self) -> &'static str {
        match self {
            Site::Google => "https://music.youtube.com/",
            Site::Spotify => "https://open.spotify.com/",
            Site::Discord => "https://discord.com/",
        }
    }

    /// The cookie that only a signed-in session has.
    fn session_cookie(self) -> &'static str {
        match self {
            Site::Google => "SAPISID",
            Site::Spotify => "sp_dc",
            Site::Discord => "",
        }
    }
}

pub enum Outcome {
    /// Google: the `Cookie` header value for youtube.com. Spotify: the `sp_dc` cookie's value.
    Cookie(String),
    /// Discord: the account token read out of the page (fresh-iframe trick).
    Token(String),
    Cancelled,
    Failed(String),
}

/// Safari's user agent (macOS always reports 10_15_7 here, whatever the version).
#[cfg(target_os = "macos")]
const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// Command-line flags that make the binary run only a sign-in window.
pub const FLAG: &str = "--login";
pub const SPOTIFY_FLAG: &str = "--login-spotify";
pub const DISCORD_FLAG: &str = "--login-discord";

/// Show the sign-in window in a child process; `done` is called exactly once.
pub fn spawn(site: Site, profile_dir: PathBuf, done: impl FnOnce(Outcome) + Send + 'static) {
    std::thread::Builder::new()
        .name("login".into())
        .spawn(move || {
            let res = std::env::current_exe().and_then(|exe| {
                let mut cmd = std::process::Command::new(exe);
                crate::proxy::pass_to(&mut cmd);
                // WebKitGTK renders a blank page (or none) on some Linux drivers, NVIDIA
                // and Wayland most of all, unless its accelerated path is switched off.
                // The sign-in page needs no speed, so the safe path is always the one.
                #[cfg(target_os = "linux")]
                {
                    cmd.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1").env("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
                }
                cmd.arg(site.flag())
                    .arg(&profile_dir)
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
            });
            done(match res {
                Err(e) => Outcome::Failed(format!("could not start sign-in window: {e}")),
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let line = text.lines().last().unwrap_or_default();
                    if let Some(c) = line.strip_prefix("cookie ") {
                        Outcome::Cookie(c.to_owned())
                    } else if let Some(t) = line.strip_prefix("discord ") {
                        Outcome::Token(t.to_owned())
                    } else if let Some(e) = line.strip_prefix("failed ") {
                        Outcome::Failed(e.to_owned())
                    } else if line == "cancelled" {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed(format!("sign-in window exited unexpectedly ({})", out.status))
                    }
                }
            })
        })
        .expect("spawn login thread");
}

/// Entry point of the child process: run the window and report on stdout.
pub fn child_main(site: Site, profile_dir: PathBuf) {
    match run(site, profile_dir) {
        Ok(Outcome::Cookie(c)) => println!("cookie {c}"),
        Ok(Outcome::Token(t)) => println!("discord {t}"),
        Ok(Outcome::Cancelled) => println!("cancelled"),
        Ok(Outcome::Failed(e)) => println!("failed {e}"),
        Err(e) => println!("failed {e:#}"),
    }
}

/// Put the webview in its window. On Linux it goes in the GTK box tao made for it:
/// wry's plain `build` only works with an X11 window that already exists, and these
/// start hidden (and Wayland has no such window at all).
pub trait BuildIn<'a> {
    fn build_in(self, window: &'a tao::window::Window) -> wry::Result<wry::WebView>;
}

impl<'a> BuildIn<'a> for WebViewBuilder<'a> {
    fn build_in(self, window: &'a tao::window::Window) -> wry::Result<wry::WebView> {
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            self.build_gtk(window.default_vbox().expect("tao makes a vbox by default"))
        }
        #[cfg(not(target_os = "linux"))]
        self.build(window)
    }
}

/// The cookies the browser would send to `url`.
#[cfg(not(target_os = "macos"))]
fn cookies_for(webview: &wry::WebView, url: &str) -> wry::Result<Vec<wry::cookie::Cookie<'static>>> {
    webview.cookies_for_url(url)
}

/// On macOS wry's `cookies_for_url` keeps only cookies set for exactly the page's host, so
/// Google's, set for `.youtube.com`, never came back and signing in never finished.
#[cfg(target_os = "macos")]
fn cookies_for(webview: &wry::WebView, url: &str) -> wry::Result<Vec<wry::cookie::Cookie<'static>>> {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
    Ok(webview.cookies()?.into_iter().filter(|c| c.domain().is_some_and(|d| domain_matches(&host, d))).collect())
}

/// Whether a cookie for `domain` (".youtube.com", "youtube.com", "music.youtube.com")
/// goes to `host`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn domain_matches(host: &str, domain: &str) -> bool {
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

/// Delete the webview profile so the next sign-in starts fresh (on sign-out, and after
/// signing in). The browser processes can hold its files for a moment after the
/// window closes, so it tries again for a few seconds; blocks meanwhile.
pub fn forget(profile_dir: &std::path::Path) {
    for tries in 1..=10 {
        match std::fs::remove_dir_all(profile_dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) if tries == 10 => log::warn!("couldn't delete the sign-in window's profile {}: {e}", profile_dir.display()),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(500)),
        }
    }
}

/// WKWebView has no profile folder (wry ignores the context's on macOS), so each
/// profile gets a data store of its own (macOS 14+; older ones share the default),
/// named by an id kept in the folder. `forget` deletes the folder, so the next sign-in
/// starts from a fresh store.
#[cfg(target_os = "macos")]
fn store_id(profile_dir: &std::path::Path) -> [u8; 16] {
    use sha1::Digest;
    let file = profile_dir.join("store-id");
    if let Ok(saved) = std::fs::read(&file)
        && let Ok(id) = <[u8; 16]>::try_from(saved.as_slice())
    {
        return id;
    }
    let seed = format!("{} {:?} {}", profile_dir.display(), std::time::SystemTime::now(), std::process::id());
    let mut id = [0u8; 16];
    id.copy_from_slice(&sha1::Sha1::digest(seed)[..16]);
    let _ = std::fs::create_dir_all(profile_dir);
    let _ = std::fs::write(&file, id);
    id
}

fn run(site: Site, profile_dir: PathBuf) -> anyhow::Result<Outcome> {
    if matches!(site, Site::Discord) {
        return run_discord(site, profile_dir);
    }
    let mut event_loop = EventLoopBuilder::<()>::with_user_event().build();

    let window = WindowBuilder::new()
        .with_title(site.title())
        .with_inner_size(tao::dpi::LogicalSize::new(480.0, 720.0))
        .build(&event_loop)?;

    let proxy = event_loop.create_proxy();
    #[cfg(target_os = "macos")]
    let store = store_id(&profile_dir);
    let mut context = WebContext::new(Some(profile_dir));
    let builder = WebViewBuilder::new_with_web_context(&mut context);
    #[cfg(target_os = "macos")]
    let builder = {
        use wry::WebViewBuilderExtDarwin;
        // WKWebView's own user agent has no "Version/… Safari/…", so YouTube Music takes it
        // for an unknown browser and shows "not optimized for your browser" instead of the
        // site. Say it's Safari, which is the engine it is.
        builder.with_data_store_identifier(store).with_user_agent(SAFARI_UA)
    };
    // Google binds a sign-in from a browser that offers device-bound sessions to a key
    // only that browser holds (`__Secure-1PSIDRTS`, renewed every 10 minutes), so the
    // copy the app takes stops working after some hours. Without them Google signs in
    // the way it does Firefox and Safari, and the copy lasts.
    let webview = crate::proxy::webview(builder, matches!(site, Site::Google))
        .with_url(site.start())
        .with_on_page_load_handler(move |ev, url| {
            if matches!(ev, PageLoadEvent::Finished) && url.starts_with(site.home()) {
                let _ = proxy.send_event(());
            }
        })
        .build_in(&window)?;

    let mut outcome = Outcome::Cancelled;
    // The session cookie isn't always in the store the moment the page lands there:
    // WKWebView hands its cookies over asynchronously, so Spotify's `sp_dc` (set well
    // after the page's own cookies) could miss the first check and the window would
    // sit there looking broken. After landing, the store is asked again for a few
    // seconds before the flow is left alone.
    let mut rechecking: Option<u32> = None;
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(()) => match cookies_for(&webview, site.home()) {
                Ok(cookies) => {
                    // Landing there while signed out also fires this; only a signed-in
                    // session has the session cookie (SAPISID, sp_dc).
                    if let Some(c) = cookies.iter().find(|c| c.name() == site.session_cookie()) {
                        outcome = Outcome::Cookie(match site {
                            Site::Spotify => c.value().to_owned(),
                            Site::Google => cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; "),
                        });
                        *flow = ControlFlow::Exit;
                    } else {
                        rechecking = Some(0);
                        *flow = ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_millis(300));
                    }
                }
                Err(e) => {
                    outcome = Outcome::Failed(format!("could not read cookies: {e}"));
                    *flow = ControlFlow::Exit;
                }
            },
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                // The store's turn again: the session cookie, or one more wait.
                if let Some(n) = rechecking {
                    match cookies_for(&webview, site.home()) {
                        Ok(cookies) => {
                            if let Some(c) = cookies.iter().find(|c| c.name() == site.session_cookie()) {
                                outcome = Outcome::Cookie(match site {
                                    Site::Spotify => c.value().to_owned(),
                                    Site::Google => cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; "),
                                });
                                *flow = ControlFlow::Exit;
                            } else if n < 30 {
                                rechecking = Some(n + 1);
                                *flow = ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_millis(300));
                            } else {
                                // Still signed out, whatever the page says: the user can
                                // keep trying in the window, or close it.
                                rechecking = None;
                            }
                        }
                        Err(e) => {
                            outcome = Outcome::Failed(format!("could not read cookies: {e}"));
                            *flow = ControlFlow::Exit;
                        }
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            _ => {}
        }
    });
    drop(webview);
    Ok(outcome)
}

/// The Discord window: there's no cookie to catch — the account token hides behind a
/// fresh same-origin iframe once the app has booted (Discord takes `localStorage`
/// away from its own window), so the page is quietly asked for it every two seconds
/// until it's there, as ForkChord's agent shim does.
fn run_discord(site: Site, profile_dir: PathBuf) -> anyhow::Result<Outcome> {
    let mut event_loop = EventLoopBuilder::<()>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(site.title())
        .with_inner_size(tao::dpi::LogicalSize::new(480.0, 720.0))
        .build(&event_loop)?;
    #[cfg(target_os = "macos")]
    let store = store_id(&profile_dir);
    let mut context = WebContext::new(Some(profile_dir));
    let builder = WebViewBuilder::new_with_web_context(&mut context);
    #[cfg(target_os = "macos")]
    let builder = builder.with_data_store_identifier(store);
    let webview = crate::proxy::webview(builder, false).with_url(site.start()).build_in(&window)?;
    // DiscordLoginScreen.TOKEN_SNIPPET: the value is the JSON string with its quotes.
    let script = concat!(
        "(function(){try{var f=document.createElement('iframe');f.style.display='none';",
        "(document.body||document.documentElement).appendChild(f);",
        "var t=f.contentWindow.localStorage.getItem('token');f.remove();return t||'';}catch(e){return '';}})()"
    );
    let mut outcome = Outcome::Cancelled;
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) | Event::NewEvents(StartCause::Poll) => {
                let read = webview.evaluate_script(script).unwrap_or_default();
                let token = read.trim().trim_matches('"').to_owned();
                // A real token is long; short answers are the page not signed in yet,
                // or an engine that didn't return the string.
                if token.len() > 20 && token.bytes().all(|b| b.is_ascii_graphic()) {
                    outcome = Outcome::Token(token);
                    *flow = ControlFlow::Exit;
                } else {
                    *flow = ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_secs(2));
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            _ => {}
        }
    });
    drop(webview);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::domain_matches;

    #[test]
    fn parent_domain_cookies_go_to_subdomains() {
        assert!(domain_matches("music.youtube.com", ".youtube.com"));
        assert!(domain_matches("music.youtube.com", "youtube.com"));
        assert!(domain_matches("music.youtube.com", "music.youtube.com"));
        assert!(!domain_matches("music.youtube.com", "accounts.google.com"));
        assert!(!domain_matches("music.youtube.com", "tube.com"));
        assert!(!domain_matches("evilyoutube.com", ".youtube.com"));
    }
}
