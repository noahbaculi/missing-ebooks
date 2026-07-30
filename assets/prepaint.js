(function () {
  var saved = localStorage.getItem('theme');
  /** @type {"light" | "dark"} */
  var t = (saved === 'light' || saved === 'dark')
    ? saved
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = t;
  if (localStorage.getItem('boldTopFolder') === 'off') {
    document.documentElement.dataset.boldTop = 'off';
  }
  if (localStorage.getItem('italicNestedFolders') === 'off') {
    document.documentElement.dataset.italicNested = 'off';
  }
  if (localStorage.getItem('introDismissed') === 'true') {
    document.documentElement.dataset.intro = 'dismissed';
  }

  // ACCENT-DERIVE:BEGIN. The single implementation, exposed to app.js as
  // window.deriveWarningInk below; behavior checked by tests/accent/derive.test.mjs.
  /**
   * Relative luminance of a #rrggbb hex, per WCAG.
   * @param {string} hex
   * @returns {number}
   */
  function luminance(hex) {
    var ch = [1, 3, 5].map(function (i) {
      var c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
  }
  /**
   * WCAG contrast ratio between two #rrggbb hexes.
   * @param {string} a
   * @param {string} b
   * @returns {number}
   */
  function contrastRatio(a, b) {
    var l1 = luminance(a), l2 = luminance(b);
    var hi = Math.max(l1, l2), lo = Math.min(l1, l2);
    return (hi + 0.05) / (lo + 0.05);
  }
  /**
   * Blend `pct`% of `hex` into `surf` in sRGB, matching CSS color-mix.
   * @param {string} hex
   * @param {number} pct
   * @param {string} surf
   * @returns {string}
   */
  function mixColors(hex, pct, surf) {
    var f = pct / 100, out = '#';
    for (var i = 1; i < 6; i += 2) {
      var h = parseInt(hex.slice(i, i + 2), 16);
      var s = parseInt(surf.slice(i, i + 2), 16);
      out += Math.round(h * f + s * (1 - f)).toString(16).padStart(2, '0');
    }
    return out;
  }
  /**
   * Convert a #rrggbb hex to HSL (h in degrees, s and l in percent).
   * @param {string} hex
   * @returns {{ h: number, s: number, l: number }}
   */
  function hexToHsl(hex) {
    var r = parseInt(hex.slice(1, 3), 16) / 255;
    var g = parseInt(hex.slice(3, 5), 16) / 255;
    var b = parseInt(hex.slice(5, 7), 16) / 255;
    var mx = Math.max(r, g, b), mn = Math.min(r, g, b), l = (mx + mn) / 2, h = 0, s = 0;
    if (mx !== mn) {
      var d = mx - mn;
      s = l > 0.5 ? d / (2 - mx - mn) : d / (mx + mn);
      if (mx === r) h = (g - b) / d + (g < b ? 6 : 0);
      else if (mx === g) h = (b - r) / d + 2;
      else h = (r - g) / d + 4;
      h /= 6;
    }
    return { h: h * 360, s: s * 100, l: l * 100 };
  }
  /**
   * Convert HSL (h degrees, s and l percent) to a #rrggbb hex.
   * @param {number} h
   * @param {number} s
   * @param {number} l
   * @returns {string}
   */
  function hslToHex(h, s, l) {
    h /= 360; s /= 100; l /= 100;
    /**
     * @param {number} p
     * @param {number} q
     * @param {number} t
     * @returns {number}
     */
    function hue2(p, q, t) {
      if (t < 0) t += 1;
      if (t > 1) t -= 1;
      if (t < 1 / 6) return p + (q - p) * 6 * t;
      if (t < 1 / 2) return q;
      if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
      return p;
    }
    var r = l, g = l, b = l;
    if (s !== 0) {
      var q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
      r = hue2(p, q, h + 1 / 3); g = hue2(p, q, h); b = hue2(p, q, h - 1 / 3);
    }
    return '#' + [r, g, b].map(function (v) {
      return Math.round(v * 255).toString(16).padStart(2, '0');
    }).join('');
  }
  /**
   * Derive a readable ink for the gap role from a base color, for one theme.
   * The pill background is `mixColors(base, 16, surface)`. Scan lightness at
   * the base hue for the most vivid shade that clears AA against it, dark ink
   * for light and light ink for dark. Falls back to a clamped extreme if
   * nothing reaches AA.
   * @param {string} base
   * @param {"light" | "dark"} theme
   * @returns {string}
   */
  function deriveWarningInk(base, theme) {
    var surf = theme === 'dark' ? '#1d232a' : '#ffffff';
    var bg = mixColors(base, 16, surf);
    var hsl = hexToHsl(base);
    var sat = Math.max(hsl.s, 42);
    /** @type {{ c: string, l: number }[]} */
    var strong = [];
    /** @type {{ c: string, l: number }[]} */
    var ok = [];
    for (var L = 8; L <= 94; L++) {
      var c = hslToHex(hsl.h, sat, L), r = contrastRatio(c, bg);
      if (r >= 5.5) strong.push({ c: c, l: L });
      else if (r >= 4.5) ok.push({ c: c, l: L });
    }
    var pool = strong.length ? strong : ok;
    if (pool.length) {
      var best = pool[0];
      for (var i = 1; i < pool.length; i++) {
        var better = theme === 'dark' ? pool[i].l < best.l : pool[i].l > best.l;
        if (better) best = pool[i];
      }
      return best.c;
    }
    return hslToHex(hsl.h, sat, theme === 'dark' ? 90 : 15);
  }
  // ACCENT-DERIVE:END

  // Hand the derivation to app.js, which repaints on theme and accent changes.
  window.deriveWarningInk = deriveWarningInk;

  var accent = localStorage.getItem('accent');
  if (accent && /^#[0-9a-fA-F]{6}$/.test(accent) && accent.toLowerCase() !== '#f5a524') {
    document.documentElement.style.setProperty('--color-warning', accent);
    document.documentElement.style.setProperty('--color-warning-text', deriveWarningInk(accent, t));
  }
})();
