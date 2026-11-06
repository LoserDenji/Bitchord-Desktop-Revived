/**
 * BitChord agent chrome spoof — injected into the sign-in window on every
 * page load, before the page's own scripts.
 *
 * The sign-in window claims a plain Chrome user agent (agent.rs), but the
 * live services — Google above all — also check the JavaScript fingerprints
 * a real Chrome has: `window.chrome`, `navigator.userAgentData`,
 * `navigator.plugins` (a Chromium engine with none of them reads as an
 * embedded WebView), and `navigator.webdriver` (must not be true). An
 * engine whose UA says Chrome but whose JS world says WebView gets the
 * "This browser or app may not be secure" page instead of the sign-in.
 *
 * Every spoof is applied only when missing, so on engines that already look
 * like Chrome (WebView2) this is mostly a no-op, and re-running it on every
 * navigation is safe — each hop of the OAuth dance starts a fresh JS world.
 */
(function () {
  'use strict';
  if (window.__BITCHORD_CHROME__) return;
  window.__BITCHORD_CHROME__ = true;

  // Not an automation session.
  try {
    Object.defineProperty(navigator, 'webdriver', {
      get: function () {
        return false;
      },
      configurable: true,
    });
  } catch (error) {
    /* older engines */
  }

  if (!window.chrome) {
    window.chrome = {
      app: { isInstalled: false, InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' }, RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' } },
      runtime: { OnInstalledReason: {}, OnRestartRequiredReason: {}, PlatformArch: {}, PlatformNaclArch: {}, PlatformOs: {}, RequestUpdateCheckStatus: {}, connect: null, sendMessage: null },
      csi: function () {
        return {};
      },
      loadTimes: function () {
        return {};
      },
    };
  }

  var uaVersion = ((navigator.userAgent.match(/Chrome\/(\d+)/) || [])[1]) || '138';

  if (!navigator.userAgentData) {
    var platform = /Win/i.test(navigator.platform)
      ? 'Windows'
      : /Mac/i.test(navigator.platform)
        ? 'macOS'
        : /Linux|X11/i.test(navigator.platform)
          ? 'Linux'
          : 'Unknown';
    var brands = [
      { brand: 'Not A(Brand', version: '99' },
      { brand: 'Chromium', version: uaVersion },
      { brand: 'Google Chrome', version: uaVersion },
    ];
    var data = {
      brands: brands,
      mobile: false,
      platform: platform,
      getHighEntropyValues: function (hints) {
        var result = { brands: brands, mobile: false, platform: platform };
        if (/architecture/.test(hints)) result.architecture = 'x86';
        if (/bitness/.test(hints)) result.bitness = '64';
        if (/model/.test(hints)) result.model = '';
        if (/platformVersion/.test(hints)) result.platformVersion = '15.0.0';
        if (/uaFullVersion/.test(hints)) result.uaFullVersion = uaVersion + '.0.0.0';
        if (/fullVersionList/.test(hints)) result.fullVersionList = brands;
        return Promise.resolve(result);
      },
      toJSON: function () {
        return { brands: brands, mobile: false, platform: platform };
      },
    };
    try {
      Object.defineProperty(navigator, 'userAgentData', { get: function () { return data; }, configurable: true });
    } catch (error) {
      /* already defined non-configurable */
    }
  }

  // A Chromium engine with an empty plugin list reads as headless. Real
  // Chrome exposes the built-in PDF viewer three times under two MIME types.
  try {
    if (!navigator.plugins || navigator.plugins.length === 0) {
      var mimeTypes = [];
      var makePlugin = function (name) {
        var plugin = {
          name: name,
          filename: 'internal-pdf-viewer',
          description: 'Portable Document Format',
          length: 2,
        };
        [0, 1].forEach(function (i) {
          var type = {
            type: i === 0 ? 'application/pdf' : 'text/pdf',
            suffixes: 'pdf',
            description: '',
            enabledPlugin: plugin,
          };
          plugin[i] = type;
          mimeTypes.push(type);
        });
        return plugin;
      };
      var plugins = ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer'].map(makePlugin);
      var array = {
        length: plugins.length,
        item: function (index) {
          return plugins[index] || null;
        },
        namedItem: function (name) {
          for (var i = 0; i < plugins.length; i++) if (plugins[i].name === name) return plugins[i];
          return null;
        },
        refresh: function () {},
      };
      plugins.forEach(function (plugin, index) {
        array[index] = plugin;
        array[plugin.name] = plugin;
      });
      var mimeArray = {
        length: mimeTypes.length,
        item: function (index) {
          return mimeTypes[index] || null;
        },
        namedItem: function (name) {
          for (var i = 0; i < mimeTypes.length; i++) if (mimeTypes[i].type === name) return mimeTypes[i];
          return null;
        },
      };
      mimeTypes.forEach(function (type, index) {
        mimeArray[index] = type;
        mimeArray[type.type] = type;
      });
      Object.defineProperty(navigator, 'plugins', { get: function () { return array; }, configurable: true });
      Object.defineProperty(navigator, 'mimeTypes', { get: function () { return mimeArray; }, configurable: true });
    }
  } catch (error) {
    /* cosmetic only */
  }
})();
