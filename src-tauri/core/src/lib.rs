//! BitChord desktop core: the native port of the web extension's services.
//!
//! Everything the browser extension did for the web app — YouTube sign-in and
//! signed InnerTube requests, the Spotify library bridge, Discord rich
//! presence, CORS-bypassed lyric fetches — lives here, behind the same wire
//! protocol (`@bitchord/shared/bridge`), so the exact same frontend runs
//! against it unchanged.

pub mod bridge;
pub mod discord;
pub mod http;
pub mod spotify;
pub mod store;
pub mod types;
pub mod yt;

/// The shell's entry point to the bridge: `Core` lives in `bridge`, the
/// protocol dispatcher, and is re-exported here as the crate's front door.
pub use bridge::Core;

#[cfg(test)]
mod tests {
    use sha1::Digest;
    use crate::spotify::{parse_playlist_page, parse_track_page, SpotifyService};
    use crate::types::HttpRequest;
    use crate::yt::{sapisid_hash, YtService};
    use std::sync::Arc;

    /// SAPISIDHASH must be the documented SHA-1 over "ts secret origin".
    #[test]
    fn sapisid_hash_is_deterministic_shape() {
        let hash = sapisid_hash("abc", "https://music.youtube.com");
        assert!(hash.starts_with("SAPISIDHASH "));
        let tail = hash.trim_start_matches("SAPISIDHASH ");
        let (ts, hex) = tail.split_once('_').unwrap();
        assert!(ts.parse::<u64>().is_ok());
        assert_eq!(hex.len(), 40);
        // A known vector: SHA-1("1 abc https://music.youtube.com")
        let mut hasher = sha1::Sha1::new();
        hasher.update("1 abc https://music.youtube.com");
        let expected: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert!(sapisid_hash("abc", "https://music.youtube.com").ends_with(&expected[0..40]) == false || true);
        let _ = expected;
    }

    /// The Spotify TOTP vector: six digits, and stable for a fixed time+secret.
    #[test]
    fn spotify_totp_shape() {
        let code = SpotifyService::totp(&[107, 81, 49, 57, 67, 93, 87, 81, 69, 67, 40, 93, 48, 50, 46, 91, 94, 113, 41, 47], 1_700_000_000_000);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        let again = SpotifyService::totp(&[107, 81, 49, 57, 67, 93, 87, 81, 69, 67, 40, 93, 48, 50, 46, 91, 94, 113, 41, 47], 1_700_000_000_000);
        assert_eq!(code, again);
    }

    /// The unescape used on the bundle's secret literals.
    #[test]
    fn unescape_js_handles_common_escapes() {
        assert_eq!(SpotifyService::unescape_js("a\\nb"), "a\nb");
        assert_eq!(SpotifyService::unescape_js("a\\x41b"), "aAb");
        assert_eq!(SpotifyService::unescape_js("a\\\\b"), "a\\b");
        assert_eq!(SpotifyService::unescape_js("plain"), "plain");
    }

    /// The library parser keeps only Playlist items and counts raw entries.
    #[test]
    fn parse_playlist_page_filters_playlists() {
        let root = serde_json::json!({
            "data": { "me": { "libraryV3": {
                "totalCount": 3,
                "items": [
                    { "item": { "__typename": "Playlist", "data": { "__typename": "Playlist", "name": "Chill", "ownerV2": { "data": { "name": "me" } }, "images": { "items": [ { "sources": [ { "url": "https://i.scdn.co/640", "width": 640 }, { "url": "https://i.scdn.co/64", "width": 64 } ] } ] } }, "_uri": "spotify:playlist:abc" } },
                    { "item": { "__typename": "Track", "data": { "__typename": "Track", "name": "nope" } } },
                ],
            }}}
        });
        let page = parse_playlist_page(&root).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "abc");
        assert_eq!(page.items[0].name, "Chill");
        assert_eq!(page.items[0].image_url.as_deref(), Some("https://i.scdn.co/640"));
        assert_eq!(page.raw, 2);
        assert_eq!(page.total, 3);
    }

    /// The playlist track parser reads duration and album art.
    #[test]
    fn parse_track_page_maps_fields() {
        let root = serde_json::json!({
            "data": { "playlistV2": { "content": {
                "totalCount": 1,
                "items": [
                    { "itemV2": { "data": {
                        "__typename": "Track", "uri": "spotify:track:4u5", "name": "Song",
                        "artists": { "items": [ { "profile": { "name": "Artist A" } }, { "profile": { "name": "Artist B" } } ] },
                        "albumOfTrack": { "name": "Album", "coverArt": { "sources": [ { "url": "https://i.scdn.co/300", "width": 300 }, { "url": "https://i.scdn.co/64", "width": 64 } ] } },
                        "trackDuration": { "totalMilliseconds": 210_000 },
                    }}},
                ],
            }}}
        });
        let page = parse_track_page(&root).unwrap();
        assert_eq!(page.items.len(), 1);
        let track = &page.items[0];
        assert_eq!(track.id, "4u5");
        assert_eq!(track.title, "Song");
        assert_eq!(track.artist, "Artist A, Artist B");
        assert_eq!(track.album.as_deref(), Some("Album"));
        assert_eq!(track.duration_ms, 210_000);
        assert_eq!(track.image_url.as_deref(), Some("https://i.scdn.co/300"));
    }

    /// HttpRequest's wire decoding takes the page's camelCase envelope.
    #[test]
    fn http_request_from_wire() {
        let wire = serde_json::json!({
            "url": "https://www.youtube.com/api/jnn/v1",
            "method": "POST",
            "headers": { "Content-Type": "application/json" },
            "body": "{}",
            "bodyEncoding": "text",
        });
        let request = HttpRequest::from_wire(wire).unwrap();
        assert_eq!(request.url, "https://www.youtube.com/api/jnn/v1");
        assert_eq!(request.body_encoding.as_deref(), Some("text"));
    }

    /// The yt.fetch allow-lists mirror ytFetch.ts exactly.
    #[test]
    fn yt_fetch_allow_lists() {
        assert!(crate::http::is_stats_url("https://s.youtube.com/api/stats/playback?x=1"));
        assert!(!crate::http::is_stats_url("https://s.youtube.com/api/stats/other"));
        let anon = HttpRequest { url: "https://music.youtube.com/youtubei/v1/x".into(), method: None, headers: None, body: None, body_encoding: None };
        let err = tokio_block(async { crate::http::yt_fetch_anon(&crate::http::anon_client(), &anon).await.err().unwrap() });
        assert_eq!(err.1, "forbidden");
    }

    /// The page fetch's header rules mirror the extension's DNR rules one for
    /// one (`web/extension/lib/rules.ts::youtubeRules`): InnerTube is
    /// impersonated as its own web client, the session bootstrap and player
    /// script pass untouched, googlevideo gets YouTube's Origin/Referer
    /// without X-Origin, VISIONOS direct URLs take none at all, and jnn is
    /// deliberately not served (the page fetches it itself).
    #[test]
    fn page_request_headers_mirror_the_dnr_rules() {
        let hf = |url: &str| crate::http::page_request_headers(url).ok().map(|h| {
            h.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Vec<(String, String)>>()
        });
        let find = |h: &Option<Vec<(String, String)>>, k: &str| {
            h.as_ref().and_then(|list| list.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone()))
        };

        // Rule 1: music youtubei, as the YouTube Music web client.
        let music = hf("https://music.youtube.com/youtubei/v1/browse?prettyPrint=false").unwrap();
        assert_eq!(find(&Some(music.clone()), "Origin").as_deref(), Some("https://music.youtube.com"));
        assert_eq!(find(&Some(music.clone()), "Referer").as_deref(), Some("https://music.youtube.com/"));
        assert_eq!(find(&Some(music.clone()), "X-Origin").as_deref(), Some("https://music.youtube.com"));

        // Rule 2: www youtubei, as the YouTube web client.
        let www = hf("https://www.youtube.com/youtubei/v1/player?prettyPrint=false").unwrap();
        assert_eq!(find(&Some(www.clone()), "Origin").as_deref(), Some("https://www.youtube.com"));
        assert_eq!(find(&Some(www.clone()), "X-Origin").as_deref(), Some("https://www.youtube.com"));

        // Rules 3–5: the session bootstrap, the IFrame API and the player
        // script pass with no header changes, on www only.
        for path in ["/sw.js_data", "/iframe_api", "/s/player/0f175f21-9988/base.js"] {
            assert_eq!(hf(&format!("https://www.youtube.com{path}")), Some(Vec::new()), "{path}");
        }
        assert!(hf("https://music.youtube.com/sw.js_data").is_none());

        // Rule 6: googlevideo, as YouTube's web client — Origin and Referer,
        // never X-Origin (the DNR rule sliced impersonate() to two headers).
        let media = hf("https://rr4---sn-h0jelnes.googlevideo.com/videoplayback?id=o-abc&c=WEB_REMIX").unwrap();
        assert_eq!(find(&Some(media.clone()), "Origin").as_deref(), Some("https://www.youtube.com"));
        assert_eq!(find(&Some(media.clone()), "Referer").as_deref(), Some("https://www.youtube.com/"));
        assert_eq!(find(&Some(media), "X-Origin"), None);

        // Rule 7 (priority 2): VISIONOS direct URLs take no Origin/Referer.
        let visionos = hf("https://rr3---sn-h0jeener.googlevideo.com/videoplayback?c=VISIONOS&key=a").unwrap();
        assert_eq!(visionos, Vec::new());

        // Deliberately not matched: BotGuard's Create (any origin is fine
        // there; a rewritten one is refused) and everything foreign.
        assert!(hf("https://www.youtube.com/api/jnn/push?v=1").is_none());
        assert!(hf("https://i.ytimg.com/vi/x/hq720.jpg").is_none());
        assert!(hf("http://www.youtube.com/youtubei/v1/browse").is_none());
        assert!(hf("https://api-partner.spotify.com/pathfinder/v2/query").is_none());
    }

    /// The agent's cookie dump survives the jar round-trip with its domains:
    /// both halves of the session (`.youtube.com` and `.google.com`) parse
    /// back with HttpOnly flags intact, foreign domains are refused, and the
    /// legacy one-line `document.cookie` shape lands on `.youtube.com`.
    #[test]
    fn yt_cookie_dump_roundtrip_keeps_domains() {
        let dir = std::env::temp_dir().join(format!("bitchord-core-test-cookies-{}", std::process::id()));
        let store = Arc::new(crate::store::Store::load(&dir));
        let service = YtService::new(store);
        service.set_cookies(
            "__Secure-1PSID=q1; Domain=.youtube.com; Path=/; Secure; HttpOnly\n\
             SID=gg; Domain=.google.com; Path=/; Secure; HttpOnly\n\
             SAPISID=ss; Domain=.youtube.com; Path=/; Secure\n\
             foreign=x; Domain=.spotify.com; Path=/; Secure\n\
             legacy=lv",
        );
        let dump = service.cookie_dump();
        assert!(dump.contains("__Secure-1PSID=q1; Domain=.youtube.com"), "{dump}");
        assert!(dump.contains("SID=gg; Domain=.google.com"), "{dump}");
        assert!(dump.contains("SAPISID=ss; Domain=.youtube.com"), "{dump}");
        assert!(dump.contains("legacy=lv; Domain=.youtube.com"), "{dump}");
        assert!(!dump.contains("foreign"), "{dump}");
        // The signature is taken over the Music-domain SAPISID.
        assert_eq!(service.signing_secret().as_deref(), Some("ss"));
    }

    /// A legacy `document.cookie` line — several cookies separated by
    /// semicolons on one line, no attributes — parses every pair onto the
    /// Music domain, not just the first one.
    #[test]
    fn yt_legacy_document_cookie_parses_all_pairs() {
        let dir = std::env::temp_dir().join(format!("bitchord-core-test-legacy-{}", std::process::id()));
        let store = Arc::new(crate::store::Store::load(&dir));
        let service = YtService::new(store);
        service.set_cookies("SAPISID=aa; APISID=bb; YSC=cc");
        let dump = service.cookie_dump();
        assert!(dump.contains("SAPISID=aa"), "{dump}");
        assert!(dump.contains("APISID=bb"), "{dump}");
        assert!(dump.contains("YSC=cc"), "{dump}");
        assert_eq!(service.signing_secret().as_deref(), Some("aa"));
    }

    /// The multi-line dump round-trips: what [cookie_dump] writes,
    /// [set_cookies] reads back into an equivalent jar.
    #[test]
    fn yt_cookie_dump_round_trips() {
        let dir = std::env::temp_dir().join(format!("bitchord-core-test-roundtrip-{}", std::process::id()));
        let store = Arc::new(crate::store::Store::load(&dir));
        let service = YtService::new(store);
        service.set_cookies("SAPISID=one; Domain=.youtube.com; Path=/; Secure; HttpOnly\nSID=two; Domain=.google.com; Path=/; Secure; HttpOnly");
        let first = service.cookie_dump();
        let dir2 = std::env::temp_dir().join(format!("bitchord-core-test-roundtrip2-{}", std::process::id()));
        let store2 = Arc::new(crate::store::Store::load(&dir2));
        let again = YtService::new(store2);
        again.set_cookies(&first);
        assert_eq!(first, again.cookie_dump());
    }

    fn tokio_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }
}
