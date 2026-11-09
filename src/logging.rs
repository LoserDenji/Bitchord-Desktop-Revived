//! File logging on every platform, for reports that come in after the fact.
//!
//! The app writes `bitchord.log` beside its settings (see `data_dir`), keeping the
//! previous run as `bitchord.prev.log` — the run that crashed is the interesting one,
//! and it must survive the relaunch. Warnings and errors land there from every part
//! of the app, panics included, so a silent "it just disappeared" report comes with
//! evidence. The sign-in webview children log to their own file, since they are
//! separate processes that outlive the user's patience for watching them.
//!
//! `RUST_LOG` (or `YTM_DEFAULT_LOG` for a test build) picks the filter as usual;
//! `YTM_LOG=<path>` sends the main log somewhere else instead.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Where and how the logger writes.
enum Kind {
    /// The main app: rotate the last run's log aside and start a fresh one.
    Main,
    /// A webview child (`login` / `spotify` / `turnstile`): append to its own file.
    Child(&'static str),
    /// A dev run with `YTM_LOG=<path>`: exactly there, no rotation.
    Dev(PathBuf),
}

/// Log to `dir/bitchord.log`, with the last run kept beside it as `.prev.log`.
pub fn init(dir: &Path) {
    // Dev hook: YTM_LOG=<path> logs there (test runs beside a running copy).
    if let Some(path) = std::env::var_os("YTM_LOG") {
        setup(dir, Kind::Dev(path.into()));
    } else {
        setup(dir, Kind::Main);
    }
}

/// Log to `dir/bitchord-<tag>.log` (appended across runs), for a child process.
pub fn init_child(dir: &Path, tag: &'static str) {
    setup(dir, Kind::Child(tag));
}

fn setup(dir: &Path, kind: Kind) {
    // Where this run's log goes, and what to say about it in the banner.
    let (file, path_note) = match &kind {
        Kind::Main => {
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join("bitchord.log");
            // The last run's log too: reopening the app after a problem would otherwise
            // wipe it. Held open by nobody (single instance) — but a copy frozen from an
            // earlier crash may still have it, and then this run simply starts fresh.
            let _ = std::fs::rename(&path, dir.join("bitchord.prev.log"));
            match std::fs::File::create(&path) {
                Ok(f) => (Some(f), path.display().to_string()),
                Err(e) => (None, format!("nowhere (couldn't create {}: {e})", path.display())),
            }
        }
        Kind::Child(tag) => {
            let _ = std::fs::create_dir_all(dir);
            let path = dir.join(format!("bitchord-{tag}.log"));
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => (Some(f), path.display().to_string()),
                Err(e) => (None, format!("nowhere (couldn't open {}: {e})", path.display())),
            }
        }
        Kind::Dev(path) => match std::fs::File::create(path) {
            Ok(f) => (Some(f), path.display().to_string()),
            Err(e) => (None, format!("nowhere (couldn't create {}: {e})", path.display())),
        },
    };

    let mut builder = env_logger::Builder::from_env(
        // A test build can be made more talkative: YTM_DEFAULT_LOG=warn,bitchord=debug cargo build.
        env_logger::Env::default().default_filter_or(option_env!("YTM_DEFAULT_LOG").unwrap_or("warn,bitchord=info")),
    );
    match file {
        // Debug builds echo to the terminal as well (`cargo run` still talks); release
        // builds have no console on Windows, where writing to stderr just fails quietly.
        Some(f) => builder.target(env_logger::Target::Pipe(Box::new(Tee { file: f, stderr: cfg!(debug_assertions), complained: false }))),
        None => builder.target(env_logger::Target::Stderr),
    };
    // try_init, not init: a second setup in one process (a test, say) must not
    // take the app down over a logger that is already there.
    let _ = builder.try_init();

    // A panic while the window is up, or while hidden in the tray, used to vanish with
    // the window (release builds have no console): put it in the log before dying.
    let next = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        log::error!("panic on thread {name}: {info}");
        next(info);
    }));

    let args: Vec<String> = std::env::args().collect();
    log::info!(
        "=== BitChord {} ({}/{}) ===",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    log::info!("log: {path_note}; data: {}", dir.display());
    log::info!("args: {args:?}");
}

/// A log writer that goes to the file, and to stderr as well where that can be seen.
struct Tee {
    file: std::fs::File,
    stderr: bool,
    /// Set after the first failed write, so a full disk is complained about once
    /// instead of once per line.
    complained: bool,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.stderr {
            let _ = std::io::stderr().write_all(buf);
            let _ = std::io::stderr().flush();
        }
        match self.file.write_all(buf).and_then(|_| self.file.flush()) {
            Ok(()) => self.complained = false,
            Err(e) => {
                // Never take the logger down with the file: say it once on stderr.
                if !self.complained {
                    self.complained = true;
                    let _ = std::io::stderr().write_all(format!("bitchord: log file write failed: {e}\n").as_bytes());
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = self.file.flush();
        Ok(())
    }
}
