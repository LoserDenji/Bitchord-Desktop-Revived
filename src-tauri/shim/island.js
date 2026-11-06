/**
 * BitChord island shim — injected as an initialization script into the
 * Dynamic Island window, before the island's own script runs.
 *
 * The island page is first-party (island.html in the app build), but it
 * deliberately ships no @tauri-apps/api dependency — the same choice the
 * main window's shim makes. All it needs is the invoke surface, so that is
 * all this installs. The island's script guards on `__IslandHost` being
 * absent, which is also what makes the page renderable in a plain browser
 * for design work.
 */
(function () {
  'use strict';
  var internals = window.__TAURI_INTERNALS__;
  if (!internals || window.__BITCHORD_ISLAND_HOST__) return;
  window.__BITCHORD_ISLAND_HOST__ = true;
  window.__IslandHost = {
    invoke: function (cmd, args) {
      return internals.invoke(cmd, args || {});
    },
  };
})();
