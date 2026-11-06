//! YouTube Music sign-in and signed requests. Port of
//! `web/extension/lib/ytAuth.ts` + the signed half of `ytFetch.ts`.
//!
//! The scheme is the one Innertube.kt and DesktopYouTubeAuth.kt use: the
//! signature over SAPISID, `X-Goog-AuthUser` to pick the login in the jar, and
//! `X-Goog-PageId` for a brand channel. Logins and channels are listed the way
//! the desktop app lists them: `account/account_menu` for each login, then
//! `account/accounts_list` for that login's channels.
//!
//! The cookies live in this crate's own jar; the shell hands over the
//! agent webview's *cookie store* — both domains the session spans
//! (`.google.com` and `.youtube.com`), HttpOnly cookies included, one full
//! `name=value; Domain=…; Path=…` line per cookie (see `set_cookies`). A
//! page-side `document.cookie` report still parses, as the fallback for
//! engines without a native cookie API — but it can never complete a
//! session on its own, because YouTube identifies it by HttpOnly cookies
//! (`__Secure-1PSID`, `__Secure-3PSID`, the rotating `PSIDTS` pair) that
//! JavaScript cannot read, and everything after that is plain Rust.

use std::sync::Arc;

use reqwest::cookie::{CookieStore, Jar};
use sha1::{Digest, Sha1};

use crate::store::{GOOGLE_ORIGIN, Store, MUSIC_ORIGIN};
use crate::types::{
    ActiveChannel, HttpResponse, HttpRequest, YtAccount, YtAuthState, YtChannel,
};

/// The cookies the signature can be taken over, in the order Google's clients prefer them.
const SIGNING_COOKIES: &[&str] = &["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"];

/// Any recent WEB_REMIX version is accepted for the account endpoints (Innertube.kt keeps a fallback the same way).
const WEB_REMIX_VERSION: &str = "1.20251006.03.00";

/// How many `X-Goog-AuthUser` indices are tried. Google allows ten logins per
/// browser, and the live ones needn't be contiguous.
const MAX_LOGINS: usize = 10;

/// Accounts rarely change; a cookie change clears this at once anyway.
const ACCOUNTS_TTL_MS: i64 = 10 * 60_000;

/// Headers the page's copies of which are dropped: identity is the core's to set.
const IDENTITY_HEADERS: &[&str] = &["authorization", "cookie", "x-goog-authuser", "x-goog-pageid", "x-origin", "origin", "referer"];

/// Identity headers this port sends with every signed request (the DNR
/// `impersonate` rules did this for the extension).
pub const YT_IMPERSONATE: &[(&str, &str)] = &[
    ("Origin", MUSIC_ORIGIN),
    ("Referer", "https://music.youtube.com/"),
];

pub struct YtService {
    pub jar: Arc<Jar>,
    pub client: reqwest::Client,
    store: Arc<Store>,
    /// Unix millis of the last account cache, and the cache itself.
    cache: tokio::sync::Mutex<Option<(i64, Accounts)>>,
    /// What went wrong reading the accounts, in words the UI can show; None when it worked.
    problem: tokio::sync::Mutex<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub auth_user: String,
    pub page_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StoredChannel {
    id: String,
    name: String,
    handle: Option<String>,
    avatar_url: Option<String>,
    brand: bool,
    auth_user: String,
    page_id: Option<String>,
    /// YouTube's own choice for this login, which is the default here too.
    selected_on_web: bool,
}

#[derive(Debug, Clone, Default)]
struct StoredAccount {
    auth_user: String,
    name: String,
    email: Option<String>,
    avatar_url: Option<String>,
    channels: Vec<StoredChannel>,
}

#[derive(Debug, Clone, Default)]
struct Accounts {
    list: Vec<StoredAccount>,
    /// The login YouTube Music's own page uses, the default choice.
    web_index: Option<String>,
}

impl YtService {
    pub fn new(store: Arc<Store>) -> YtService {
        let jar = Arc::new(Jar::default());
        let client = reqwest::Client::builder()
            .cookie_provider(jar.clone())
            .user_agent(concat!(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ",
                "Chrome/138.0.0.0 Safari/537.36"
            ))
            .build()
            .expect("yt client");
        let service = YtService {
            jar,
            client,
            store,
            cache: tokio::sync::Mutex::new(None),
            problem: tokio::sync::Mutex::new(None),
        };
        // A restart restores the agent's last cookie dump, so the session survives.
        if let Some(dump) = service.store.read(|d| d.yt_cookie_dump.clone()) {
            service.set_cookies(&dump);
        }
        service
    }

    /// Feeds a cookie dump from the agent window into the jar. The current
    /// shape is one cookie per line with its attributes — what the shell
    /// captures natively from the webview's cookie store:
    ///
    /// ```text
    /// __Secure-1PSID=…; Domain=.youtube.com; Path=/; Secure; HttpOnly
    /// SID=…; Domain=.google.com; Path=/; Secure; HttpOnly
    /// ```
    ///
    /// The old one-line `document.cookie` shape still parses: there the
    /// segments after the first are not attributes but more cookies, and
    /// every one of them lands on `.youtube.com`. Cookies on any other
    /// domain are dropped; an absent domain means `.youtube.com`, since that
    /// is the domain every signed request goes to.
    pub fn set_cookies(&self, dump: &str) {
        let music = url::Url::parse(MUSIC_ORIGIN).unwrap();
        let google = url::Url::parse(GOOGLE_ORIGIN).unwrap();
        for line in dump.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut cookie: Option<(String, String)> = None;
            let mut domain = String::new();
            let mut path = String::new();
            let mut flags = String::new();
            for segment in line.split(';') {
                let segment = segment.trim();
                if segment.is_empty() {
                    continue;
                }
                // Value-less attributes first: Secure, HttpOnly.
                if !segment.contains('=') {
                    if cookie.is_some() {
                        if segment.eq_ignore_ascii_case("secure") {
                            flags.push_str("; Secure");
                        } else if segment.eq_ignore_ascii_case("httponly") {
                            flags.push_str("; HttpOnly");
                        }
                    }
                    continue;
                }
                let (raw_key, value) = segment.split_once('=').unwrap();
                let key = raw_key.trim().to_ascii_lowercase();
                if is_cookie_attribute(&key) {
                    // Belongs to the cookie already open, never starts one.
                    if cookie.is_some() {
                        match key.as_str() {
                            "domain" => domain = value.trim().to_string(),
                            "path" => path = value.trim().to_string(),
                            _ => {}
                        }
                    }
                    continue;
                }
                // Anything else starts a new cookie: this is the boundary
                // between the entries of a legacy `document.cookie` line.
                if let Some((name, value)) = cookie.take() {
                    self.add_cookie((&music, &google), &name, &value, &domain, &path, &flags);
                }
                cookie = Some((raw_key.trim().to_string(), value.trim().to_string()));
                domain.clear();
                path.clear();
                flags.clear();
            }
            if let Some((name, value)) = cookie.take() {
                self.add_cookie((&music, &google), &name, &value, &domain, &path, &flags);
            }
        }
    }

    /// Adds one parsed cookie to the jar, defaulting to the Music domain and
    /// refusing cookies for domains YouTube's session never spans. The jar
    /// rejects cookies whose domain doesn't match the URL they are added
    /// through (RFC 6265 domain-match), so a Google-domain cookie must be
    /// added through the Google URL — that's the pair of URLs' whole job.
    fn add_cookie(
        &self,
        urls: (&url::Url, &url::Url),
        name: &str,
        value: &str,
        domain: &str,
        path: &str,
        flags: &str,
    ) {
        if name.is_empty() || value.is_empty() {
            return;
        }
        let host = domain.trim().trim_start_matches('.');
        let host = if host.is_empty() {
            "youtube.com"
        } else if host.ends_with("youtube.com") || host.ends_with("google.com") {
            host
        } else {
            return;
        };
        let path = if path.trim().is_empty() { "/" } else { path.trim() };
        let raw = format!("{name}={value}; Domain=.{host}; Path={path}{flags}");
        let url = if host.ends_with("google.com") { urls.1 } else { urls.0 };
        self.jar.add_cookie_str(&raw, url);
    }

    /// The jar's cookies as the multi-line dump [Self::set_cookies] reads
    /// back on the next launch — one cookie per line with its domain, so
    /// both halves of the session (`.youtube.com` and `.google.com`) survive
    /// a restart. The jar absorbs YouTube's rotating Set-Cookie headers on
    /// its own; re-persisting this after signed reads keeps the stored dump
    /// current without ever opening a page.
    pub fn cookie_dump(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for (origin, domain) in [(MUSIC_ORIGIN, ".youtube.com"), (GOOGLE_ORIGIN, ".google.com")] {
            let url = url::Url::parse(origin).unwrap();
            let Some(header) = self.jar.cookies(&url) else { continue };
            let Ok(text) = header.to_str() else { continue };
            for pair in text.split("; ") {
                if pair.contains('=') {
                    lines.push(format!("{pair}; Domain={domain}; Path=/"));
                }
            }
        }
        lines.join("\n")
    }

    /// The cookie the signature can be taken over, in Google's preferred order.
    pub fn signing_secret(&self) -> Option<String> {
        let url = url::Url::parse(MUSIC_ORIGIN).unwrap();
        for name in SIGNING_COOKIES {
            for cookie in self.jar.cookies(&url).into_iter() {
                for pair in cookie.to_str().unwrap_or_default().split("; ") {
                    if let Some((k, v)) = pair.split_once('=') {
                        if k == *name && !v.is_empty() {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    pub async fn browser_signed_in(&self) -> bool {
        self.signing_secret().is_some()
    }
}

/// `SAPISIDHASH <seconds>_<sha1(seconds + " " + secret + " " + origin)>`, as DesktopYouTubeAuth.sapisidHash.
pub fn sapisid_hash(secret: &str, origin: &str) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hasher = Sha1::new();
    hasher.update(format!("{timestamp} {secret} {origin}"));
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {timestamp}_{hex}")
}

// ---- cookie jar helpers --------------------------------------------------------

/// The Set-Cookie attribute names; a segment keyed by one of these is an
/// attribute of the cookie before it, never the start of a new one.
fn is_cookie_attribute(key: &str) -> bool {
    matches!(
        key,
        "domain" | "path" | "expires" | "max-age" | "samesite" | "secure" | "httponly" | "partitioned" | "priority"
    )
}

// ---- Account reading (DesktopAccountParser) ------------------------------------

type Json = serde_json::Value;

/// Every object stored under [key], anywhere inside (ytAuth.renderers).
fn renderers(root: &Json, key: &str, out: &mut Vec<Json>) {
    match root {
        Json::Array(items) => {
            for item in items {
                renderers(item, key, out);
            }
        }
        Json::Object(map) => {
            for (k, v) in map {
                if k == key && v.is_object() {
                    out.push(v.clone());
                } else {
                    renderers(v, key, out);
                }
            }
        }
        _ => {}
    }
}

/// The first string under [key], anywhere inside (ytAuth.findString).
fn find_string(root: &Json, key: &str) -> Option<String> {
    match root {
        Json::Array(items) => items.iter().find_map(|item| find_string(item, key)),
        Json::Object(map) => {
            for (k, v) in map {
                if k == key && v.is_string() {
                    return v.as_str().map(|s| s.to_string());
                }
                if let Some(found) = find_string(v, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// A `runs` or `simpleText` field as plain text (ytAuth.text).
fn text(node: Option<&Json>) -> String {
    let Some(node) = node else { return String::new() };
    if let Some(runs) = node.get("runs").and_then(|r| r.as_array()) {
        return runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
    }
    node.get("simpleText").and_then(|t| t.as_str()).unwrap_or_default().to_string()
}

fn photo(item: &Json) -> Option<String> {
    item.get("accountPhoto")
        .and_then(|p| p.get("thumbnails"))
        .and_then(|t| t.as_array())
        .and_then(|t| t.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
}

fn channel_id(auth_user: &str, page_id: Option<&str>) -> String {
    format!("{auth_user}:{}", page_id.unwrap_or("own"))
}

fn parse_channels(root: &Json, auth_user: &str) -> Vec<StoredChannel> {
    let mut items = Vec::new();
    renderers(root, "accountItem", &mut items);
    let mut channels = Vec::new();
    for item in items {
        let name = text(item.get("accountName"));
        if name.is_empty() {
            continue;
        }
        let page_id = find_string(&item, "pageId");
        let handle = {
            let h = text(item.get("channelHandle"));
            if h.is_empty() {
                text(item.get("accountByline"))
            } else {
                h
            }
        };
        let handle = if handle.is_empty() { None } else { Some(handle) };
        let id = channel_id(auth_user, page_id.as_deref());
        if channels.iter().any(|c: &StoredChannel| c.id == id) {
            continue;
        }
        channels.push(StoredChannel {
            id,
            name,
            handle,
            avatar_url: photo(&item),
            brand: page_id.is_some(),
            auth_user: auth_user.to_string(),
            page_id,
            selected_on_web: item.get("isSelected").and_then(|s| s.as_bool()).unwrap_or(false),
        });
    }
    channels
}

/// Why a response has no account in it, read from what Google says about the session (ytAuth.describeEmpty).
fn describe_empty(endpoint: &str, body: &Json) -> String {
    let services = body
        .get("responseContext")
        .and_then(|c| c.get("serviceTrackingParams"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let logged_in = services.iter().find_map(|service| {
        service
            .get("params")
            .and_then(|p| p.as_array())
            .and_then(|p| {
                p.iter().find(|param| param.get("key").and_then(|k| k.as_str()) == Some("logged_in"))
            })
            .and_then(|param| param.get("value").and_then(|v| v.as_str()))
    });
    if logged_in == Some("0") {
        return format!("{endpoint}: YouTube answered as if signed out (the session wasn't accepted)");
    }
    let keys = if body.is_object() {
        body.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()
    } else {
        format!("{body:?}")
    };
    format!("{endpoint}: no account in the answer ({keys})")
}

impl YtService {
    /// One login, or None with the reason when that index isn't signed in
    /// (ytAuth.readLogin). Only `account_menu` decides.
    async fn read_login(&self, auth_user: &str) -> (Option<StoredAccount>, Option<String>) {
        let identity = Identity { auth_user: auth_user.to_string(), page_id: None };
        let menu = match self.music_api("account/account_menu", &identity).await {
            Ok(menu) => menu,
            Err(error) => return (None, Some(format!("account_menu: {error}"))),
        };
        let mut headers = Vec::new();
        renderers(&menu, "activeAccountHeaderRenderer", &mut headers);
        let Some(header) = headers.into_iter().next() else {
            return (None, Some(describe_empty("account_menu", &menu)));
        };
        let name = {
            let n = text(header.get("accountName"));
            if n.is_empty() { "YouTube Music".to_string() } else { n }
        };
        let email = {
            let e = text(header.get("email"));
            if e.is_empty() { None } else { Some(e) }
        };
        let handle = {
            let h = text(header.get("channelHandle"));
            if h.is_empty() { None } else { Some(h) }
        };
        let avatar_url = photo(&header);
        let mut channels = parse_channels(&self.music_api("account/accounts_list", &identity).await.unwrap_or(Json::Null), auth_user);
        if channels.is_empty() {
            channels = vec![StoredChannel {
                id: channel_id(auth_user, None),
                name: name.clone(),
                handle: handle.or_else(|| email.clone()),
                avatar_url: avatar_url.clone(),
                brand: false,
                auth_user: auth_user.to_string(),
                page_id: None,
                selected_on_web: true,
            }];
        }
        (
            Some(StoredAccount {
                auth_user: auth_user.to_string(),
                name,
                email,
                avatar_url,
                channels,
            }),
            None,
        )
    }

    /// The login YouTube Music itself is using in this browser: its page's
    /// `SESSION_INDEX` (ytcfg). That's the right default rather than index 0.
    async fn web_session_index(&self) -> Option<String> {
        let html = self
            .signed_fetch(
                MUSIC_ORIGIN,
                HttpRequest { url: MUSIC_ORIGIN.to_string(), method: Some("GET".into()), headers: None, body: None, body_encoding: None },
                Identity { auth_user: "0".into(), page_id: None },
                None,
            )
            .await
            .ok()?;
        if html.status != 200 {
            return None;
        }
        let regex = regex::Regex::new(r#""SESSION_INDEX":"?(\d+)"#).unwrap();
        regex.captures(&html.body).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
    }

    async fn read_accounts(&self) -> (Accounts, Option<String>) {
        let indices: Vec<String> = (0..MAX_LOGINS).map(|i| i.to_string()).collect();
        let mut reads = Vec::with_capacity(MAX_LOGINS);
        for idx in &indices {
            reads.push(self.read_login(idx));
        }
        let (results, web_index) = tokio::join!(futures_util::future::join_all(reads), self.web_session_index());
        let mut list = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (account, _) in &results {
            let Some(account) = account else { continue };
            let key = account
                .email
                .clone()
                .unwrap_or_else(|| {
                    format!(
                        "{}|{}",
                        account.name,
                        account.channels.iter().map(|c| c.id.split(':').nth(1).unwrap_or("").to_string()).collect::<Vec<_>>().join(",")
                    )
                });
            if !seen.insert(key) {
                continue;
            }
            list.push(account.clone());
        }
        let problem = if list.is_empty() {
            let idx: usize = web_index.as_deref().and_then(|w| w.parse().ok()).unwrap_or(0);
            Some(
                results
                    .get(idx)
                    .and_then(|(_, why)| why.clone())
                    .or_else(|| results.first().and_then(|(_, why)| why.clone()))
                    .unwrap_or_else(|| "no signed-in Google account found".into()),
            )
        } else {
            None
        };
        (Accounts { list, web_index }, problem)
    }

    async fn accounts(&self, refresh: bool) -> Accounts {
        let now = now_ms();
        {
            let cache = self.cache.lock().await;
            if let Some((at, accounts)) = cache.as_ref() {
                if !refresh && now - at < ACCOUNTS_TTL_MS {
                    return accounts.clone();
                }
            }
        }
        let (accounts, problem) = self.read_accounts().await;
        {
            let mut cache = self.cache.lock().await;
            *cache = Some((now, accounts.clone()));
        }
        *self.problem.lock().await = problem;
        accounts
    }

    /// Forgets the account list, after a sign-in or sign-out in the browser (ytAuth.invalidateAccounts).
    pub async fn invalidate_accounts(&self) {
        *self.cache.lock().await = None;
    }

    fn choose_channel(accounts: &Accounts, selected_id: Option<&str>) -> Option<StoredChannel> {
        let all: Vec<StoredChannel> = accounts.list.iter().flat_map(|a| a.channels.clone()).collect();
        let web = accounts.list.iter().find(|a| Some(&a.auth_user) == accounts.web_index.as_ref()).or_else(|| accounts.list.first());
        all.iter()
            .find(|c| Some(c.id.as_str()) == selected_id)
            .cloned()
            .or_else(|| {
                web.and_then(|w| {
                    w.channels.iter().find(|c| c.selected_on_web).cloned().or_else(|| w.channels.first().cloned())
                })
            })
    }

    fn public_channel(c: &StoredChannel) -> YtChannel {
        YtChannel { id: c.id.clone(), name: c.name.clone(), handle: c.handle.clone(), avatar_url: c.avatar_url.clone(), brand: c.brand }
    }

    pub async fn auth_state(&self, refresh: bool) -> YtAuthState {
        let (yt_connected, selected) = self.store.read(|d| (d.yt_connected, d.yt_selected_channel_id.clone()));
        let browser_signed_in = self.signing_secret().is_some();
        if !browser_signed_in || !yt_connected {
            return YtAuthState {
                browser_signed_in,
                connected: yt_connected,
                signed_in: false,
                accounts: vec![],
                active: None,
                problem: None,
            };
        }
        let accounts = self.accounts(refresh).await;
        let active = Self::choose_channel(&accounts, selected.as_deref());
        YtAuthState {
            browser_signed_in,
            connected: true,
            signed_in: active.is_some(),
            problem: if active.is_some() { None } else { self.problem.lock().await.clone() },
            accounts: accounts
                .list
                .iter()
                .map(|a| YtAccount {
                    auth_user: a.auth_user.clone(),
                    name: a.name.clone(),
                    email: a.email.clone(),
                    avatar_url: a.avatar_url.clone(),
                    channels: a.channels.iter().map(|c| Self::public_channel(c)).collect(),
                })
                .collect(),
            active: active.map(|a| ActiveChannel {
                id: a.id,
                name: a.name,
                handle: a.handle,
                avatar_url: a.avatar_url,
                brand: a.brand,
                auth_user: a.auth_user,
            }),
        }
    }

    /// The identity signed requests use right now, or None when they can't be made (ytAuth.activeIdentity).
    pub async fn active_identity(&self) -> Option<Identity> {
        let (yt_connected, selected) = self.store.read(|d| (d.yt_connected, d.yt_selected_channel_id.clone()));
        if !yt_connected || self.signing_secret().is_none() {
            return None;
        }
        let accounts = self.accounts(false).await;
        Self::choose_channel(&accounts, selected.as_deref()).map(|a| Identity { auth_user: a.auth_user, page_id: a.page_id })
    }

    pub async fn select_channel(&self, id: &str) -> Result<YtAuthState, String> {
        let accounts = self.accounts(false).await;
        if !accounts.list.iter().any(|a| a.channels.iter().any(|c| c.id == id)) {
            return Err(format!("No channel {id} in this browser"));
        }
        self.store.with(|d| d.yt_selected_channel_id = Some(id.to_string()));
        self.store.save();
        Ok(self.auth_state(false).await)
    }

    pub async fn set_connected(&self, connected: bool) -> YtAuthState {
        self.store.with(|d| d.yt_connected = connected);
        self.store.save();
        self.auth_state(connected).await
    }

    // ---- Requests ---------------------------------------------------------------

    /// A request made as [identity], signed for [sign_as] when that isn't the
    /// URL's own origin (ytAuth.signedFetch). The jar attaches the cookies.
    pub async fn signed_fetch(
        &self,
        url: &str,
        request: HttpRequest,
        identity: Identity,
        sign_as: Option<&str>,
    ) -> Result<HttpResponse, (String, String)> {
        let Some(secret) = self.signing_secret() else {
            return Err(("The app is not signed in to YouTube.".into(), "unauthenticated".into()));
        };
        let origin = sign_as
            .map(|s| s.to_string())
            .unwrap_or_else(|| url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_default());
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(map) = &request.headers {
            for (key, value) in map {
                if !IDENTITY_HEADERS.contains(&key.to_lowercase().as_str()) {
                    headers.push((key.clone(), value.clone()));
                }
            }
        }
        headers.push(("Authorization".into(), sapisid_hash(&secret, &origin)));
        headers.push(("X-Goog-AuthUser".into(), identity.auth_user.clone()));
        headers.push(("X-Origin".into(), origin.clone()));
        // The DNR impersonate rules gave every signed request the YouTube
        // Origin/Referer pair; YouTube's own web client sends the same.
        headers.push(("Origin".into(), origin.clone()));
        headers.push(("Referer".into(), format!("{origin}/")));
        match &identity.page_id {
            Some(page_id) => headers.push(("X-Goog-PageId".into(), page_id.clone())),
            None => {}
        }
        send_signed(&self.client, url, request.method.as_deref().unwrap_or("GET"), headers, request.body.as_deref(), request.body_encoding.as_deref(), false).await
    }

    /// A POST to a YouTube Music InnerTube endpoint as the client YouTube Music itself is (ytAuth.musicApi).
    pub async fn music_api(&self, endpoint: &str, identity: &Identity) -> Result<Json, String> {
        let url = format!("{MUSIC_ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
        let body = serde_json::json!({
            "context": { "client": { "clientName": "WEB_REMIX", "clientVersion": WEB_REMIX_VERSION, "hl": "en", "gl": "US" }, "user": {} },
        });
        let response = self
            .signed_fetch(
                &url,
                HttpRequest {
                    url: url.clone(),
                    method: Some("POST".into()),
                    headers: Some([
                        ("Content-Type".to_string(), "application/json".to_string()),
                        ("X-Youtube-Client-Name".to_string(), "67".to_string()),
                        ("X-Youtube-Client-Version".to_string(), WEB_REMIX_VERSION.to_string()),
                    ].into_iter().collect()),
                    body: Some(body.to_string()),
                    body_encoding: None,
                },
                identity.clone(),
                None,
            )
            .await
            .map_err(|(message, _)| message)?;
        if response.status != 200 {
            return Err(format!("HTTP {}", response.status));
        }
        serde_json::from_str(&response.body).map_err(|e| e.to_string())
    }

    /// `yt.fetch` with `auth` (ytFetch.signedYtFetch): InnerTube or a stats ping.
    pub async fn yt_fetch_signed(&self, request: &HttpRequest) -> Result<HttpResponse, (String, String)> {
        let stats = crate::http::is_stats_url(&request.url) && request.method.as_deref().unwrap_or("GET") == "GET";
        if !stats && !crate::http::YT_SIGNED_ALLOWED.iter().any(|base| request.url.starts_with(base)) {
            return Err(forbidden_url(&request.url));
        }
        let Some(identity) = self.active_identity().await else {
            return Err(("Not signed in to YouTube Music.".into(), "unauthenticated".into()));
        };
        // A stats ping is signed for YouTube Music, whichever host it goes to, as the web client's are.
        let sign_as = if stats { Some(MUSIC_ORIGIN) } else { None };
        self.signed_fetch(&request.url, request.clone(), identity, sign_as).await
    }
}

fn forbidden_url(url: &str) -> (String, String) {
    let parsed = url::Url::parse(url).ok();
    let where_ = parsed
        .as_ref()
        .map(|u| format!("{}{}", u.origin().ascii_serialization(), u.path()))
        .unwrap_or_else(|| url.to_string());
    (format!("yt.fetch doesn't serve {where_}"), "forbidden".into())
}

/// Sends a signed request with the explicit header set (the browser did this
/// via fetch + DNR rules; here the headers go on directly).
async fn send_signed(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    headers: Vec<(String, String)>,
    body: Option<&str>,
    body_encoding: Option<&str>,
    impersonate_yt: bool,
) -> Result<HttpResponse, (String, String)> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| (e.to_string(), "internal".into()))?;
    let mut builder = client.request(method, url);
    for (key, value) in &headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    if impersonate_yt {
        for (key, value) in YT_IMPERSONATE {
            builder = builder.header(*key, *value);
        }
    }
    if let Some(body) = body {
        if body_encoding == Some("base64") {
            let bytes = crate::http::from_base64(body).map_err(|e| (e, "internal".into()))?;
            builder = builder.body(bytes);
        } else {
            builder = builder.body(body.to_string());
        }
    }
    let response = builder.send().await.map_err(|e| (e.to_string(), "network".into()))?;
    Ok(crate::http::serialise(response, false).await)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
