/**
 * BitChord agent shim — injected into the sign-in window on the real service
 * origins (music.youtube.com / accounts.google.com, open.spotify.com,
 * discord.com).
 *
 * These pages hold the user's actual sessions. The page's own JavaScript
 * world is the only place those cookies and tokens are reachable (YouTube's
 * SAPISID and Spotify's sp_dc are readable from document.cookie, and
 * Discord's token is hidden behind a fresh same-origin iframe), so this
 * script reports them to the Rust core over IPC. Credentials end up in the
 * core's store, never in the app page.
 *
 * The service is derived from the origin on every report, not baked in at
 * injection: the sign-in flow hops origins (accounts.google.com →
 * accounts.youtube.com → music.youtube.com), and the shell may reuse the
 * window for a different service later. Reports from origins that are none
 * of the services are dropped.
 */
(function () {
  'use strict';
  var internals = window.__TAURI_INTERNALS__;
  if (!internals || window.__BITCHORD_AGENT__) return;
  window.__BITCHORD_AGENT__ = true;

  var kindOf = function () {
    var host = (window.location && window.location.hostname) || '';
    if (host === 'discord.com' || host.endsWith('.discord.com')) return 'discord';
    if (host.indexOf('spotify') !== -1) return 'spotify';
    if (host.endsWith('youtube.com') || host.endsWith('google.com')) return 'yt';
    return null;
  };

  var report = function (kind, payload) {
    try {
      internals
        .invoke('agent_report', { kind: kind, payload: String(payload == null ? '' : payload) })
        .catch(function () {});
    } catch (error) {
      /* The window outlives a shutdown core; drop the report. */
    }
  };

  var readDiscordToken = function () {
    // DiscordLoginScreen.TOKEN_SNIPPET: Discord takes localStorage away from
    // its window once the app boots, but a fresh same-origin iframe still
    // sees it.
    try {
      var frame = document.createElement('iframe');
      frame.style.display = 'none';
      (document.body || document.documentElement).appendChild(frame);
      var raw = frame.contentWindow ? frame.contentWindow.localStorage.getItem('token') : null;
      frame.remove();
      return raw ? raw.slice(1, -1) : '';
    } catch (error) {
      return '';
    }
  };

  var poll = function () {
    var kind = kindOf();
    if (kind === 'discord') {
      var token = readDiscordToken();
      if (token) {
        report(kind, token);
        return;
      }
    } else if (kind) {
      // yt / spotify: the whole cookie line; the core picks the cookies it
      // knows (SAPISID for YouTube, sp_dc for Spotify).
      report(kind, document.cookie);
    }
    setTimeout(poll, 2000);
  };

  if (document.readyState === 'complete' || document.readyState === 'interactive') {
    poll();
  } else {
    document.addEventListener('DOMContentLoaded', poll);
    window.addEventListener('load', poll);
  }
})();
