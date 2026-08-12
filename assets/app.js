// @ts-check
// missing-ebooks client behavior. The pre-paint bootstrap (theme + depth opt-outs)
// runs inline in <head>. This file owns the rest: the theme control, the two depth
// toggles, the settings panel sync, and the marker-write confirmation. Loaded at
// the end of <body>, after htmx.
(function () {
  "use strict";

  /**
   * Look up an element the page always renders. Throws if it is absent, which
   * only happens if the markup and this script fall out of step.
   * @param {string} id
   * @returns {HTMLElement}
   */
  function need(id) {
    var el = document.getElementById(id);
    if (!el) throw new Error("missing #" + id);
    return el;
  }

  var THEME_KEY = "theme";
  var CONFIRM_KEY = "confirmMarks";
  var BOLD_KEY = "boldTopFolder";
  var ITALIC_KEY = "italicNestedFolders";
  var darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

  // theme

  /**
   * The theme to paint for a stored choice. "system" and an absent value follow
   * the OS preference.
   * @param {string | undefined} choice
   * @returns {"light" | "dark"}
   */
  function resolveTheme(choice) {
    if (choice === "light" || choice === "dark") return choice;
    return darkQuery.matches ? "dark" : "light";
  }

  /**
   * The stored choice, normalized so an absent or unknown value reads as "system".
   * @returns {string}
   */
  function storedTheme() {
    var saved = localStorage.getItem(THEME_KEY);
    return saved === "light" || saved === "dark" ? saved : "system";
  }

  /**
   * Apply a choice, persist it, and highlight the matching segment.
   * @param {string} choice
   */
  function setTheme(choice) {
    localStorage.setItem(THEME_KEY, choice);
    document.documentElement.dataset.theme = resolveTheme(choice);
    markActiveTheme(choice);
    applyAccent(storedAccent());
  }

  /**
   * Mark the active theme segment and clear the others.
   * @param {string} choice
   */
  function markActiveTheme(choice) {
    var segs = /** @type {NodeListOf<HTMLElement>} */ (
      document.querySelectorAll("[data-theme-choice]")
    );
    for (var i = 0; i < segs.length; i++) {
      var on = segs[i].dataset.themeChoice === choice;
      segs[i].classList.toggle("segment-active", on);
      if (on) {
        segs[i].setAttribute("aria-current", "true");
      } else {
        segs[i].removeAttribute("aria-current");
      }
    }
  }

  // While System is selected, follow the OS as it flips.
  darkQuery.addEventListener("change", function () {
    if (storedTheme() === "system") {
      document.documentElement.dataset.theme = resolveTheme("system");
      applyAccent(storedAccent());
    }
  });

  // accent color preference

  var ACCENT_KEY = "accent";
  var ACCENT_DEFAULT = "#f5a524";
  var ACCENT_RE = /^#[0-9a-fA-F]{6}$/;

  /**
   * The stored accent base, or the default when unset or malformed.
   * @returns {string}
   */
  function storedAccent() {
    var v = localStorage.getItem(ACCENT_KEY);
    return v && ACCENT_RE.test(v) ? v : ACCENT_DEFAULT;
  }

  /**
   * Ring the quick-pick dot matching the current base, clear the rest.
   * @param {string} base
   */
  function markActiveAccent(base) {
    var dots = document.querySelectorAll("[data-accent]");
    for (var i = 0; i < dots.length; i++) {
      var dot = /** @type {HTMLElement} */ (dots[i]);
      var on = (dot.dataset.accent || "").toLowerCase() === base.toLowerCase();
      dot.classList.toggle("accent-dot-active", on);
    }
  }

  /**
   * Paint the accent. The default writes no override, so the stylesheet's tuned
   * tokens apply. A custom color sets the base and the per-theme derived ink
   * inline on <html>, which outranks the stylesheet. Reflects the active dot and
   * the color input, so a preset pick moves the swatch too.
   * @param {string} base
   */
  function applyAccent(base) {
    // A malformed base would land in the inline styles. Use the default.
    if (!ACCENT_RE.test(base)) base = ACCENT_DEFAULT;
    var root = document.documentElement;
    if (base.toLowerCase() === ACCENT_DEFAULT) {
      root.style.removeProperty("--color-warning");
      root.style.removeProperty("--color-warning-text");
    } else {
      var theme = resolveTheme(storedTheme());
      root.style.setProperty("--color-warning", base);
      root.style.setProperty("--color-warning-text", window.deriveWarningInk(base, theme));
    }
    markActiveAccent(base);
    var input = /** @type {HTMLInputElement | null} */ (document.getElementById("accent-input"));
    if (input) input.value = base;
  }

  /**
   * Persist a chosen accent and apply it. The default clears the stored key.
   * @param {string} base
   */
  function setAccent(base) {
    // Don't persist a malformed pick.
    if (!ACCENT_RE.test(base)) base = ACCENT_DEFAULT;
    if (base.toLowerCase() === ACCENT_DEFAULT) {
      localStorage.removeItem(ACCENT_KEY);
    } else {
      localStorage.setItem(ACCENT_KEY, base);
    }
    applyAccent(base);
  }

  // confirm-before-marking preference

  /**
   * On by default: only the literal "off" disables it, so the key need not exist.
   * @returns {boolean}
   */
  function confirmEnabled() {
    return localStorage.getItem(CONFIRM_KEY) !== "off";
  }

  /** @param {boolean} on */
  function setConfirmEnabled(on) {
    localStorage.setItem(CONFIRM_KEY, on ? "on" : "off");
  }

  // folder-depth styling preferences

  /**
   * A depth styling preference is on by default: only the literal "off" disables
   * it, so the key need not exist.
   * @param {string} key
   * @returns {boolean}
   */
  function stylePrefEnabled(key) {
    return localStorage.getItem(key) !== "off";
  }

  /**
   * Persist a depth styling choice and reflect it on <html> via its data
   * attribute. The on state removes the attribute so the default markup stays
   * attribute-free, matching the pre-paint bootstrap. removeAttribute rather than
   * delete: dataset properties are non-optional under strict tsc, so delete trips
   * TS2790.
   * @param {string} key
   * @param {string} attr
   * @param {boolean} on
   */
  function setStylePref(key, attr, on) {
    localStorage.setItem(key, on ? "on" : "off");
    if (on) {
      document.documentElement.removeAttribute(attr);
    } else {
      document.documentElement.setAttribute(attr, "off");
    }
  }

  // Sync the settings controls from storage. Runs on load and whenever the panel
  // opens, so the switch reflects a "Don't ask again" choice made in the dialog.
  function syncSettings() {
    markActiveTheme(storedTheme());
    var sw = /** @type {HTMLInputElement | null} */ (document.getElementById("confirm-toggle"));
    if (sw) sw.checked = confirmEnabled();
    var boldSw = /** @type {HTMLInputElement | null} */ (document.getElementById("bold-top-toggle"));
    if (boldSw) boldSw.checked = stylePrefEnabled(BOLD_KEY);
    var italicSw = /** @type {HTMLInputElement | null} */ (document.getElementById("italic-nested-toggle"));
    if (italicSw) italicSw.checked = stylePrefEnabled(ITALIC_KEY);
    var accentInput = /** @type {HTMLInputElement | null} */ (document.getElementById("accent-input"));
    if (accentInput) accentInput.value = storedAccent();
    markActiveAccent(storedAccent());
  }

  document.addEventListener("DOMContentLoaded", function () {
    syncSettings();

    var panel = document.getElementById("settings-panel");
    if (panel) panel.addEventListener("toggle", syncSettings);

    var segs = document.querySelectorAll("[data-theme-choice]");
    for (var i = 0; i < segs.length; i++) {
      segs[i].addEventListener("click", function (e) {
        var el = /** @type {HTMLElement} */ (e.currentTarget);
        setTheme(el.dataset.themeChoice || "system");
      });
    }

    var sw = document.getElementById("confirm-toggle");
    if (sw) {
      sw.addEventListener("change", function (e) {
        var el = /** @type {HTMLInputElement} */ (e.currentTarget);
        setConfirmEnabled(el.checked);
      });
    }

    var boldSw = document.getElementById("bold-top-toggle");
    if (boldSw) {
      boldSw.addEventListener("change", function (e) {
        var el = /** @type {HTMLInputElement} */ (e.currentTarget);
        setStylePref(BOLD_KEY, "data-bold-top", el.checked);
      });
    }

    var italicSw = document.getElementById("italic-nested-toggle");
    if (italicSw) {
      italicSw.addEventListener("change", function (e) {
        var el = /** @type {HTMLInputElement} */ (e.currentTarget);
        setStylePref(ITALIC_KEY, "data-italic-nested", el.checked);
      });
    }

    var accentInput = document.getElementById("accent-input");
    if (accentInput) {
      accentInput.addEventListener("input", function (e) {
        var el = /** @type {HTMLInputElement} */ (e.currentTarget);
        setAccent(el.value);
      });
    }

    var accentDots = document.querySelectorAll("[data-accent]");
    for (var d = 0; d < accentDots.length; d++) {
      accentDots[d].addEventListener("click", function (e) {
        var el = /** @type {HTMLElement} */ (e.currentTarget);
        setAccent(el.dataset.accent || ACCENT_DEFAULT);
      });
    }
  });

  // intro card: dismiss is per-device and reversible. Dismissing sets the flag
  // and hides the card via the same data-intro attribute the pre-paint bootstrap
  // sets, so a reload keeps it hidden. The navbar "?" button toggles the card:
  // it shows it again (moving focus into it so keyboard and screen-reader users
  // land on the restored content) and hides it when it is already showing. The
  // button's aria-expanded mirrors the card's visibility, so assistive tech
  // hears the toggle change state.
  var INTRO_KEY = "introDismissed";

  /**
   * @param {boolean} on
   */
  function setIntroDismissed(on) {
    var root = document.documentElement;
    if (on) {
      localStorage.setItem(INTRO_KEY, "true");
      root.dataset.intro = "dismissed";
    } else {
      localStorage.removeItem(INTRO_KEY);
      root.removeAttribute("data-intro");
    }
    var help = document.getElementById("intro-help");
    if (help) help.setAttribute("aria-expanded", on ? "false" : "true");
  }

  document.addEventListener("DOMContentLoaded", function () {
    // Reconcile the toggle's aria-expanded against the per-device flag the
    // pre-paint bootstrap already reflected onto <html>. The markup ships
    // aria-expanded="true" (the default render), so this only flips when the
    // visitor previously dismissed the card.
    var helpInit = document.getElementById("intro-help");
    if (helpInit) {
      var initiallyDismissed =
        document.documentElement.dataset.intro === "dismissed";
      helpInit.setAttribute("aria-expanded", initiallyDismissed ? "false" : "true");
    }
    var dismiss = document.getElementById("intro-dismiss");
    if (dismiss) {
      dismiss.addEventListener("click", function () {
        setIntroDismissed(true);
        // The card just left. Hand focus to the control that brings it back.
        var help = document.getElementById("intro-help");
        if (help) help.focus();
      });
    }
    var help = document.getElementById("intro-help");
    if (help) {
      help.addEventListener("click", function () {
        // Toggle: hide the card if it is showing, show it if it is hidden.
        var dismissed =
          document.documentElement.dataset.intro === "dismissed";
        setIntroDismissed(!dismissed);
        if (dismissed) {
          // We just brought it back. Move focus into the restored card.
          var dismissBtn = document.getElementById("intro-dismiss");
          if (dismissBtn) dismissBtn.focus();
        }
        // If we just hid it, focus stays on the "?" button that hid it.
      });
    }
  });

  // marker-write confirmation

  /** @type {HTMLDialogElement | null} */
  var dialog = null;
  /** @type {HtmxConfirmDetail | null} */
  var pending = null;

  /**
   * Fill the dialog from the button that fired, and reveal its marker glyph.
   * @param {HTMLElement} btn
   */
  function fillDialog(btn) {
    if (!dialog) return;
    var action = /** @type {string} */ (btn.dataset.confirmAction);
    var file = btn.dataset.confirmFile;
    need("confirm-title").textContent = action + "?";
    need("confirm-folder").textContent = btn.dataset.confirmFolder || "";
    need("confirm-file").textContent = file || "";
    need("confirm-accept-label").textContent = action;
    var icons = /** @type {NodeListOf<HTMLElement>} */ (
      dialog.querySelectorAll("[data-confirm-icon]")
    );
    for (var i = 0; i < icons.length; i++) {
      icons[i].hidden = icons[i].dataset.confirmIcon !== file;
    }
    /** @type {HTMLInputElement} */ (need("confirm-again")).checked = false;
  }

  // Send the held request. Capture it before close(), since the close handler
  // clears pending.
  function acceptConfirm() {
    if (!dialog) return;
    var held = pending;
    pending = null;
    if (/** @type {HTMLInputElement} */ (need("confirm-again")).checked) {
      setConfirmEnabled(false);
    }
    dialog.close();
    if (held) held.issueRequest(true);
  }

  document.addEventListener("DOMContentLoaded", function () {
    dialog = /** @type {HTMLDialogElement | null} */ (document.getElementById("confirm-mark"));
    if (!dialog) return;
    need("confirm-accept").addEventListener("click", acceptConfirm);
    need("confirm-cancel").addEventListener("click", function () {
      if (dialog) dialog.close();
    });
    // Esc, a backdrop click, or Cancel all fire close: drop the held request so
    // nothing is sent.
    dialog.addEventListener("close", function () {
      pending = null;
    });
  });

  // htmx fires htmx:confirm before every request. /mark and /unmark both go
  // through htmx, and we still gate on the button's confirm data, so the undo
  // POST and every other request flow through untouched by the dialog.
  document.body.addEventListener("htmx:confirm", function (evt) {
    // A programmatic resend already had the user's intent. Don't re-prompt.
    if (suppressConfirm) {
      suppressConfirm = false;
      return;
    }
    var elt = evt.detail.elt;
    if (!elt || !elt.dataset || !elt.dataset.confirmAction) return;
    if (!confirmEnabled() || !dialog) return;
    evt.preventDefault();
    pending = evt.detail;
    fillDialog(elt);
    dialog.showModal();
  });

  // mark: hold the row in place while saving, collapse only once confirmed

  /**
   * The single-child spine from `li` upward: each row that is the sole
   * `:scope > li` in its list, stopping at the first list that still has
   * siblings. Both the collapse (mark) and expand (undo) animations walk this
   * so an author or series wrapper moves together with its only book.
   * @param {Element} li
   * @returns {HTMLElement[]}
   */
  function spineOf(li) {
    /** @type {HTMLElement[]} */
    var rows = [];
    /** @type {HTMLElement | null} */
    var node = /** @type {HTMLElement} */ (li);
    while (node) {
      rows.push(node);
      /** @type {HTMLElement | null} */
      var list = node.parentElement;
      if (!list || list.querySelectorAll(":scope > li").length > 1) break;
      node = list.parentElement && list.parentElement.closest("li");
    }
    return rows;
  }

  /**
   * Collapse a row's <li> and fade it so the rows below glide up through normal
   * reflow. The section swap is delayed (the marker form's hx-swap "swap:" modifier)
   * to let this play, then it reconciles the fresh section. li.leaving owns the
   * timing, fade, and reduced-motion.
   * @param {Element | null} li
   */
  function collapseRow(li) {
    if (!li || li.classList.contains("leaving")) return;
    // The spine above the leaf, truncated at the first row already
    // mid-collapse so a concurrent collapse is not re-pinned and restarted.
    /** @type {HTMLElement[]} */
    var rows = [];
    var spine = spineOf(li);
    for (var i = 0; i < spine.length && !spine[i].classList.contains("leaving"); i++) {
      rows.push(spine[i]);
    }
    // Pin each row's height, then drop them all to zero next frame so the transitions
    // share a definite start. `.leaving` owns the timing, fade, and reduced-motion.
    rows.forEach(function (row) {
      row.style.maxHeight = row.scrollHeight + "px";
      row.classList.add("leaving");
    });
    requestAnimationFrame(function () {
      rows.forEach(function (row) {
        row.style.maxHeight = "0";
      });
    });
  }

  // undo: expand the restored row back in

  // How long expandRow holds its inline max-height/opacity before cleanup, just
  // past the 250ms max-height transition in app.css so the slide finishes first.
  var ENTER_CLEANUP_MS = 300;

  /**
   * Slide a restored row, and the single-child spine above it, back into the gaps
   * list: the inverse of collapseRow. Undo swaps the whole section in at full
   * height, so we measure each spine row, drop it to zero before paint, then
   * release it next frame. li.entering owns the timing, fade, and reduced-motion.
   * @param {Element | null} li
   */
  function expandRow(li) {
    if (!li || li.classList.contains("entering")) return;
    var rows = spineOf(li);
    // Measure every row at its natural height before zeroing any, so an outer
    // wrapper's target height already includes its inner content.
    /** @type {number[]} */
    var heights = rows.map(function (row) {
      return row.scrollHeight;
    });
    // Drop to zero in the same synchronous pass, before the browser paints the
    // freshly swapped section, so the row never flashes at full height first.
    rows.forEach(function (row) {
      row.classList.add("entering");
      row.style.maxHeight = "0";
      row.style.opacity = "0";
    });
    requestAnimationFrame(function () {
      rows.forEach(function (row, i) {
        row.style.maxHeight = heights[i] + "px";
        row.style.opacity = "1";
      });
    });
    rows.forEach(finishEntering);
  }

  /**
   * Clear a row's expand-in once it finishes: drop the inline max-height/opacity
   * and the .entering class, or a leftover overflow:hidden with a fixed max-height
   * would clip a later fold-open. transitionend fires the moment the slide ends. A
   * timeout backs it up for reduced-motion, where the transition never fires.
   * @param {HTMLElement} row
   */
  function finishEntering(row) {
    var done = false;
    function clear() {
      if (done) return;
      done = true;
      row.removeEventListener("transitionend", onEnd);
      row.classList.remove("entering");
      row.style.maxHeight = "";
      row.style.opacity = "";
    }
    /** @param {TransitionEvent} evt */
    function onEnd(evt) {
      if (evt.target === row && evt.propertyName === "max-height") clear();
    }
    row.addEventListener("transitionend", onEnd);
    setTimeout(clear, ENTER_CLEANUP_MS);
  }

  /**
   * Toggle a gaps-only row's in-flight "saving" state: dim it, hide its actions, and
   * show a spinner with a "Saving…" label. Idempotent, so a retry's beforeRequest is a
   * no-op rather than a second spinner.
   * @param {Element | null} row
   * @param {boolean} on
   */
  function setSaving(row, on) {
    if (!row) return;
    row.classList.toggle("is-saving", on);
    var existing = row.querySelector(":scope > .row-saving");
    if (on && !existing) {
      var s = document.createElement("span");
      s.className = "row-saving";
      s.setAttribute("aria-hidden", "true");
      s.innerHTML = '<span class="row-saving-spinner"></span>Saving…';
      row.appendChild(s);
    } else if (!on && existing) {
      existing.remove();
    }
  }

  /**
   * True for a gaps-only mark request (the kind whose row should collapse on success).
   * A show-all mark carries view=all and stays put, flipping to covered in place.
   * @param {Element | null} btn
   * @returns {boolean}
   */
  function isCollapsingMark(btn) {
    if (opOf(btn) !== "mark") return false;
    var form = /** @type {Element} */ (btn).closest("form.mark");
    var view = form && /** @type {HTMLInputElement | null} */ (form.querySelector('input[name="view"]'));
    return !(view && view.value === "all");
  }

  // The moment a mark goes out, drop focus from its button: the section swap removes it,
  // and a focused element vanishing jumps the scroll to the document bottom in either
  // view. A gaps-only mark also holds its row in the "saving" state so it stays visible
  // until the server confirms, never looking handled before the write lands.
  document.body.addEventListener("htmx:beforeRequest", function (evt) {
    var btn = evt.detail.elt;
    if (opOf(btn) !== "mark") return;
    if (document.activeElement === btn) btn.blur();
    if (isCollapsingMark(btn)) setSaving(btn.closest(".row"), true);
  });

  // A confirmed save (2xx) is the only thing that may hide the row: collapse it now and
  // let the delayed section swap reconcile after the glide. Transient failures never
  // reach here, so a folder is never hidden before its mark is actually written.
  document.body.addEventListener("htmx:beforeOnLoad", function (evt) {
    var btn = evt.detail.elt;
    if (!isCollapsingMark(btn)) return;
    var xhr = evt.detail.xhr;
    if (!xhr || xhr.status < 200 || xhr.status >= 300) return;
    collapseRow(btn.closest("li"));
  });

  // connection status: detection + banner

  // Backstop for /mark and /refresh, both fast. /rescan opts out via `hx-request`
  // in `src/web/page.rs`: a legitimate big-library walk can exceed 30 s and a
  // timeout there mislabels a successful rescan as failed.
  htmx.config.timeout = 30000;

  /** @type {HTMLElement | null} */
  var connBanner = null;
  /** @type {ReturnType<typeof setTimeout> | null} */
  var reconnectTimer = null;

  /**
   * Copy for a state, read from the banner's data-msg-* attributes
   * (data-msg-offline -> dataset.msgOffline, and so on).
   * @param {string} state
   * @returns {string}
   */
  function bannerMsg(state) {
    var key = "msg" + state.charAt(0).toUpperCase() + state.slice(1);
    return (connBanner && connBanner.dataset[key]) || "";
  }

  /**
   * Reveal the banner in a state, with optional action-specific copy override.
   * @param {string} state
   * @param {string | null} [override]
   */
  function showBanner(state, override) {
    if (!connBanner) return;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    connBanner.className = "conn-banner is-" + state;
    // Unhide before setting the message so the change lands while the live region is
    // in the accessibility tree, where a polite announcement can fire.
    connBanner.hidden = false;
    var msg = connBanner.querySelector(".conn-banner-msg");
    if (msg) msg.textContent = override || bannerMsg(state);
  }

  function hideBanner() {
    if (!connBanner) return;
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    connBanner.hidden = true;
    connBanner.className = "conn-banner";
  }

  // Brief green confirmation, then hide. Shown when connectivity returns after a
  // problem was on screen.
  function flashReconnected() {
    showBanner("reconnected");
    reconnectTimer = setTimeout(hideBanner, 2000);
  }

  /**
   * True while the banner is showing something the user should see resolve.
   * @returns {boolean}
   */
  function bannerShowsProblem() {
    return (
      !!connBanner &&
      !connBanner.hidden &&
      (connBanner.classList.contains("is-offline") ||
        connBanner.classList.contains("is-retrying") ||
        connBanner.classList.contains("is-failed"))
    );
  }

  document.addEventListener("DOMContentLoaded", function () {
    connBanner = document.getElementById("conn-banner");
    if (navigator.onLine === false) showBanner("offline");
  });

  window.addEventListener("offline", function () {
    showBanner("offline");
  });
  window.addEventListener("online", function () {
    if (bannerShowsProblem()) flashReconnected();
    else hideBanner();
  });

  // connection failures: classify, roll back a mark, offer a retry

  var suppressConfirm = false;

  /**
   * The op a request belongs to ("mark" / "rescan"), or null if we don't manage it.
   * @param {Element | null} elt
   * @returns {"mark" | "rescan" | null}
   */
  function opOf(elt) {
    var post = elt && elt.getAttribute && elt.getAttribute("hx-post");
    if (post === "/mark") return "mark";
    if (post === "/rescan") return "rescan";
    return null;
  }

  /**
   * A failure worth retrying: a dropped connection, a timeout, or a gateway error
   * from a proxy / restarting server. A plain 4xx/5xx is a real server error.
   * @param {string} kind
   * @param {XMLHttpRequest} [xhr]
   * @returns {boolean}
   */
  function isRetryable(kind, xhr) {
    if (kind === "sendError" || kind === "timeout") return true;
    if (kind === "responseError" && xhr) {
      return xhr.status === 502 || xhr.status === 503 || xhr.status === 504;
    }
    return false;
  }

  /**
   * @param {HTMLElement} form
   * @returns {Record<string, string>}
   */
  function formValues(form) {
    /** @type {Record<string, string>} */
    var v = {};
    // File values cannot occur: the mark forms hold only hidden inputs.
    new FormData(/** @type {HTMLFormElement} */ (form)).forEach(function (value, name) {
      if (typeof value === "string") v[name] = value;
    });
    return v;
  }

  /**
   * Re-send a request htmx already sent, reusing its verb, target, swap, and values.
   * suppressConfirm keeps the mark confirm dialog from re-prompting on a resend.
   * @param {HTMLElement} elt
   * @param {string} op
   */
  function reissue(elt, op) {
    if (op === "mark") {
      // elt is the mark button. Its form holds the hidden fields and the hx-swap.
      var form = /** @type {HTMLElement | null} */ (elt.closest("form.mark"));
      // Bail before arming suppressConfirm if the form is gone, so a stray failure
      // on a detached button can't leave the flag stuck and mute the next confirm.
      if (!form) return;
      suppressConfirm = true;
      var values = formValues(form);
      var hv = JSON.parse(elt.getAttribute("hx-vals") || "{}");
      Object.keys(hv).forEach(function (k) {
        values[k] = hv[k];
      });
      window.htmx.ajax("POST", "/mark", {
        source: elt,
        target: elt.closest("section.root"),
        swap: form.getAttribute("hx-swap"),
        values: values
      });
    } else {
      // elt IS the rescan form (hx-post="/rescan" lives on the form, not a button),
      // so its own hx-swap and inputs drive the resend.
      suppressConfirm = true;
      window.htmx.ajax("POST", "/rescan", {
        source: elt,
        target: document.getElementById("roots"),
        swap: elt.getAttribute("hx-swap"),
        values: formValues(elt)
      });
    }
  }

  // The mark-failure strip's warn glyph is served as a hidden <template> in
  // src/web/page.rs (sourced from assets/svg/warning.svg) and cloned here, so
  // the SVG bytes live in one place. The renderer below falls back to nothing
  // if the template is missing rather than re-inlining the asset.

  /**
   * Return this <li>'s own row line, whether a leaf <div.row> or the <summary.row>
   * inside <details>, so the failure strip can sit right under the folder's own
   * line rather than after its whole child subtree.
   * @param {Element} li
   * @returns {Element | null}
   */
  function rowOf(li) {
    return li.querySelector(":scope > .row") || li.querySelector(":scope > details > .row");
  }

  /**
   * Remove the failure strip belonging to this element's row (its immediate next
   * sibling), if one is showing.
   * @param {Element} elt
   */
  function clearMarkFailed(elt) {
    var li = elt.closest("li");
    var row = li && rowOf(li);
    var box = row && row.nextElementSibling;
    if (box && box.classList.contains("mark-failed")) box.remove();
  }

  /**
   * Re-send a failed action after the user clicks its inline Retry. Starts a fresh
   * retry sequence.
   * @param {HTMLElement} elt
   * @param {string} op
   */
  function manualRetry(elt, op) {
    retryState.delete(elt);
    clearMarkFailed(elt);
    reissue(elt, op);
  }

  /**
   * Roll a failed mark back: undo the optimistic collapse and attach an inline error
   * directly under the folder's own row, naming the folder, with a Retry that re-sends
   * the same mark. The original mark buttons stay on the row (the section never
   * swapped), so the user can also just pick again.
   * @param {HTMLElement} elt
   */
  function markTerminalFailure(elt) {
    var li = elt.closest("li");
    if (li) {
      // The row was held in the "saving" state (never collapsed), so just release it.
      // The defensive .leaving cleanup covers a manual retry that did collapse first.
      li.classList.remove("leaving");
      li.style.maxHeight = "";
      setSaving(rowOf(li), false);
      clearMarkFailed(elt);
      var folder = elt.dataset.confirmFolder || "this folder";
      var box = document.createElement("div");
      box.className = "mark-failed";
      box.setAttribute("role", "alert");
      var icon = document.createElement("span");
      icon.className = "mark-failed-icon";
      icon.setAttribute("aria-hidden", "true");
      var warnTpl = document.getElementById("mark-warn-tpl");
      if (warnTpl && warnTpl instanceof HTMLTemplateElement) {
        icon.appendChild(warnTpl.content.cloneNode(true));
      }
      var msg = document.createElement("span");
      msg.className = "mark-failed-msg";
      msg.textContent = 'Couldn’t save “' + folder + '”.';
      var retry = document.createElement("button");
      retry.type = "button";
      retry.className = "btn btn-outline btn-xs mark-retry";
      retry.textContent = "↻ Retry";
      retry.addEventListener("click", function () {
        manualRetry(elt, "mark");
      });
      box.appendChild(icon);
      box.appendChild(msg);
      box.appendChild(retry);
      var row = rowOf(li);
      if (row) row.insertAdjacentElement("afterend", box);
      else li.appendChild(box);
    }
    showBanner("failed");
  }

  /**
   * Retry a transient failure a bounded number of times with backoff. Once exhausted
   * (or for a non-retryable failure) fall through to the terminal handler.
   * @param {HTMLElement} elt
   * @param {string} op
   * @param {boolean} retryable
   */
  function handleFailure(elt, op, retryable) {
    var st = retryState.get(elt) || { attempts: 0 };
    if (retryable && st.attempts < MAX_RETRIES) {
      var delay = BACKOFFS[st.attempts];
      st.attempts += 1;
      retryState.set(elt, st);
      showBanner("retrying");
      if (op === "rescan") rescanRetryHold(true);
      setTimeout(function () {
        reissue(elt, op);
      }, delay);
    } else {
      terminalFailure(elt, op);
    }
  }

  /** @type {("htmx:sendError" | "htmx:timeout" | "htmx:responseError")[]} */
  var FAILURE_EVENTS = ["htmx:sendError", "htmx:timeout", "htmx:responseError"];
  FAILURE_EVENTS.forEach(function (type) {
    document.body.addEventListener(type, function (evt) {
      var elt = evt.detail.elt;
      var op = opOf(elt);
      if (!op) return;
      var kind = type.slice("htmx:".length);
      handleFailure(elt, op, isRetryable(kind, evt.detail.xhr));
    });
  });

  // A successful request ends any retry sequence: clear the held indicators and the
  // button highlight, and clear a problem banner with a brief Reconnected flash.
  document.body.addEventListener("htmx:afterRequest", function (evt) {
    if (!evt.detail.successful) return;
    var op = opOf(evt.detail.elt);
    if (!op) return;
    retryState.delete(evt.detail.elt);
    // The rescan retry-highlight only clears when a rescan itself succeeds. An
    // unrelated successful mark must not strip the "still needs rescanning" cue.
    if (op === "rescan") {
      rescanRetryHold(false);
      var btn = document.getElementById("rescan-btn");
      if (btn) btn.classList.remove("conn-retry-hl");
    }
    if (bannerShowsProblem()) flashReconnected();
  });
  // bounded auto-retry for both idempotent endpoints

  var MAX_RETRIES = 3;
  var BACKOFFS = [500, 1500, 3000];
  /** @type {WeakMap<Element, { attempts: number }>} */
  var retryState = new WeakMap(); // request element -> { attempts }

  /**
   * Hold (or release) the rescan bar and busy button across backoff gaps, so
   * the loading state does not flicker between attempts.
   * @param {boolean} on
   */
  function rescanRetryHold(on) {
    var sk = document.getElementById("scan-bar");
    var btn = /** @type {HTMLButtonElement | null} */ (document.getElementById("rescan-btn"));
    if (sk) sk.classList.toggle("is-retrying", on);
    // Holding it disabled carries the dim across the gaps. :disabled is its busy hook.
    if (btn) btn.disabled = on;
  }

  // A rescan that failed for good: drop the bar, re-enable the Rescan button,
  // and highlight it as the retry.
  function rescanTerminalFailure() {
    rescanRetryHold(false);
    var btn = /** @type {HTMLButtonElement | null} */ (document.getElementById("rescan-btn"));
    if (btn) {
      btn.disabled = false;
      btn.classList.remove("htmx-request");
      btn.classList.add("conn-retry-hl");
    }
    showBanner("failed", connBanner ? connBanner.dataset.msgFailedRescan : null);
  }

  /**
   * @param {HTMLElement} elt
   * @param {string} op
   */
  function terminalFailure(elt, op) {
    retryState.delete(elt);
    if (op === "mark") markTerminalFailure(elt);
    else rescanTerminalFailure();
  }

  // success toast stack (undo offers)

  // Up to three toasts coexist. A fourth evicts the oldest. Each one offers an undo
  // and clears after SUCCESS_MS. Write failures are shown inline by the row instead.
  /** @type {HTMLElement | null} */
  var stack = null;
  /** @type {HTMLTemplateElement | null} */
  var template = null;
  var MAX_TOASTS = 3;
  var SUCCESS_MS = 8000;
  // The exit animation length. Must match the `toast-out` duration in app.css,
  // since dismissToast removes the node after this delay.
  var EXIT_MS = 480;
  // How long the toasts already in the stack take to slide to their new spot
  // when one is added. JS-only: the reflow is an inline transition, not a CSS
  // animation, so it has no stylesheet counterpart to stay in step with.
  var REFLOW_MS = 350;

  // Map each marker kind to the label shown in the success toast. "No ebook"
  // spells out the row's short "None" button, which has no column header to lean
  // on once it is lifted into a toast.
  /** @type {Record<string, string>} */
  var KIND_LABEL = { no_ebook: "No ebook", ebook_elsewhere: "Ebook elsewhere" };

  /**
   * The toast nodes carry private dismiss-timer bookkeeping as expando fields.
   * @typedef {HTMLElement & {
   *   _timer?: ReturnType<typeof setTimeout> | null,
   *   _remaining?: number,
   *   _start?: number,
   *   _leaving?: boolean,
   *   _hovered?: boolean,
   *   _focused?: boolean,
   * }} ToastNode
   */

  /**
   * Remove a toast immediately, clearing its dismiss timer. Used when the stack
   * evicts to stay within MAX_TOASTS and when Escape clears everything at once.
   * @param {ToastNode} node
   */
  function hardRemove(node) {
    if (node._timer) clearTimeout(node._timer);
    node.remove();
  }

  /**
   * Animate a toast out, then remove it. Guards against a second trigger, e.g. a
   * close click landing while the auto-dismiss is already playing.
   * @param {ToastNode} node
   */
  function dismissToast(node) {
    if (node._leaving) return;
    node._leaving = true;
    if (node._timer) clearTimeout(node._timer);
    node.classList.add("toast--out");
    setTimeout(function () {
      node.remove();
    }, EXIT_MS);
  }

  // Drop the oldest toasts until appending one more stays within MAX_TOASTS.
  function evictOldest() {
    if (!stack) return;
    while (stack.children.length >= MAX_TOASTS) {
      hardRemove(/** @type {ToastNode} */ (stack.firstElementChild));
    }
  }

  /**
   * A fresh toast node cloned from the page template.
   * @returns {ToastNode}
   */
  function newToastNode() {
    if (!template) throw new Error("missing #toast-template");
    return /** @type {ToastNode} */ (
      /** @type {Element} */ (template.content.firstElementChild).cloneNode(true)
    );
  }

  /**
   * A fresh element with a class and optional text.
   * @param {string} tag
   * @param {string} cls
   * @param {string | null} [text]
   * @returns {HTMLElement}
   */
  function el(tag, cls, text) {
    var node = document.createElement(tag);
    node.className = cls;
    if (text != null) node.textContent = text;
    return node;
  }

  /**
   * Arm (or re-arm) a toast's auto-dismiss for `ms` from now, remembering when
   * it started and how much time is left so a pause can bank the remainder.
   * @param {ToastNode} node
   * @param {number} ms
   */
  function armToast(node, ms) {
    node._remaining = ms;
    node._start = Date.now();
    node._timer = setTimeout(function () {
      dismissToast(node);
    }, ms);
  }

  /**
   * Pause the auto-dismiss and bank the time left on the clock. Hover and focus
   * each pause. A second pause while already paused is a no-op.
   * @param {ToastNode} node
   */
  function pauseToast(node) {
    if (node._leaving || !node._timer) return;
    clearTimeout(node._timer);
    node._timer = null;
    node._remaining =
      /** @type {number} */ (node._remaining) - (Date.now() - /** @type {number} */ (node._start));
  }

  /**
   * Resume from the banked remainder, but only once the toast is neither hovered
   * nor focused, so releasing one hold while the other still stands keeps it
   * paused.
   * @param {ToastNode} node
   */
  function resumeToast(node) {
    if (node._leaving || node._timer || node._hovered || node._focused) return;
    armToast(node, Math.max(/** @type {number} */ (node._remaining), 0));
  }

  /**
   * Whether the viewer asked for less motion. The reflow slide honors it the
   * same way the CSS animations do (the reduced-motion block already stills the
   * entry and exit), so a new toast simply appears in place.
   * @returns {boolean}
   */
  function prefersReducedMotion() {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  /**
   * Add a toast to the stack, sliding the toasts already there from their old
   * positions to their new ones instead of letting them jump up. FLIP: record
   * each current top (First), append the newcomer so the layout settles (Last),
   * offset each existing toast back to where it sat (Invert), then transition
   * that offset away on the next frame (Play).
   * @param {ToastNode} node
   */
  function reflowStack(node) {
    if (!stack) return;
    if (prefersReducedMotion()) {
      stack.appendChild(node);
      return;
    }
    var olds = Array.prototype.slice.call(stack.children);
    var firstTops = olds.map(function (el) {
      return el.getBoundingClientRect().top;
    });
    stack.appendChild(node);
    olds.forEach(function (el, i) {
      var dy = firstTops[i] - el.getBoundingClientRect().top;
      if (!dy) return;
      // The settled class drops the filled `toast-in` animation, whose pinned
      // end-state would otherwise win over this inline transform.
      el.classList.add("toast--settled");
      el.style.transition = "none";
      el.style.transform = "translateY(" + dy + "px)";
    });
    // Flush the inverted offsets so the browser paints them before they play.
    void stack.offsetHeight;
    requestAnimationFrame(function () {
      olds.forEach(function (el) {
        el.style.transition = "transform " + REFLOW_MS + "ms ease";
        el.style.transform = "";
      });
    });
  }

  /**
   * Append a built node, wire its close button and pause-on-interaction, and
   * start its dismiss timer.
   * @param {ToastNode} node
   * @param {number} timeoutMs
   */
  function pushToast(node, timeoutMs) {
    evictOldest();
    /** @type {Element} */ (node.querySelector(".toast-close")).addEventListener("click", function () {
      dismissToast(node);
    });
    node.addEventListener("mouseenter", function () {
      node._hovered = true;
      pauseToast(node);
    });
    node.addEventListener("mouseleave", function () {
      node._hovered = false;
      resumeToast(node);
    });
    node.addEventListener("focusin", function () {
      node._focused = true;
      pauseToast(node);
    });
    // focusout also fires when focus moves between controls inside the toast.
    // Only treat it as leaving when focus has actually left the node.
    node.addEventListener("focusout", function (evt) {
      if (node.contains(/** @type {Node | null} */ (evt.relatedTarget))) return;
      node._focused = false;
      resumeToast(node);
    });
    armToast(node, timeoutMs);
    reflowStack(node);
  }

  /**
   * Show the success variant: an undo offer that clears after SUCCESS_MS.
   * @param {MarkedDetail} detail
   */
  function showSuccessToast(detail) {
    if (!stack || !template || !detail) return;
    var node = newToastNode();
    node.setAttribute("role", "status");
    node.setAttribute("aria-live", "polite");
    /** @type {Element} */ (node.querySelector(".toast-undo")).addEventListener("click", function () {
      dismissToast(node);
      // Qualify by `section.root` so the swap lands on the root's section, not a
      // matching `data-root` chip in the gap-summary strip up top. Without the
      // tag, the bare `[data-root]` selector hits the chip first and the
      // section markup swaps in for the chip, duplicating row state and
      // throwing the live gap-hero count off.
      htmx.ajax("POST", "/unmark", {
        target: 'section.root[data-root="' + CSS.escape(detail.root) + '"]',
        swap: "outerHTML",
        values: {
          root: detail.root,
          rel: detail.rel,
          kind: detail.kind,
          view: detail.view,
        },
      });
    });
    pushToast(node, SUCCESS_MS);
    // Fill the message after the node is in the DOM so its live region announces
    // the change: the folder name on top, the outcome and marker label beneath.
    var label = KIND_LABEL[detail.kind] || detail.kind;
    var name = el("div", "toast-name", detail.name);
    name.title = detail.name;
    var outcome = el("div", "toast-detail");
    outcome.append("Marked as ", el("span", "toast-kind", label));
    /** @type {Element} */ (node.querySelector(".toast-msg")).append(name, outcome);
  }

  // Clear the whole stack at once (Escape) without waiting on exit animations.
  function clearToasts() {
    if (!stack) return;
    while (stack.firstElementChild) hardRemove(/** @type {ToastNode} */ (stack.firstElementChild));
  }

  document.addEventListener("DOMContentLoaded", function () {
    stack = document.getElementById("toast-stack");
    template = /** @type {HTMLTemplateElement | null} */ (document.getElementById("toast-template"));
  });

  // htmx dispatches `marked` from the HX-Trigger header on a successful /mark
  // response, which bubbles to the body. Write failures are shown inline by the row.
  document.body.addEventListener("marked", function (evt) {
    showSuccessToast(evt.detail);
  });
  document.addEventListener("keydown", function (evt) {
    if (evt.key === "Escape") clearToasts();
  });

  // search / filter

  // The whole tree filters client-side over the DOM already present. A node stays
  // visible when its own name matches the query or any descendant matches, so the
  // path to a match reads correctly. Non-matching branches collapse. The summary is
  // never touched here: filtering changes what is visible, not how many gaps exist.
  /** @type {HTMLInputElement | null} */
  var searchInput = null;
  /** @type {HTMLElement | null} */
  var searchEmpty = null;
  /** @type {HTMLElement | null} */
  var searchClear = null; // the themed × button, shown only when the box holds text
  /** @type {HTMLAnchorElement | null} */
  var viewLink = null; // the inactive view-toggle segment (an <a>), or null
  var viewLinkBase = ""; // its pristine href, before any filter query is appended

  // Enable the navbar filter once JS is running. It renders disabled so the load
  // window never offers a dead box. Clearing `disabled` here, after the input handler
  // is wired, hands over a live filter without ever reflowing the box in.
  function enableSearch() {
    if (searchInput) searchInput.disabled = false;
  }

  /**
   * The visible name on a row's own line (leaf div.row or folder summary.row),
   * lowercased for a case-insensitive compare.
   * @param {Element} li
   * @returns {string}
   */
  function rowName(li) {
    var row = li.querySelector(":scope > .row, :scope > details > summary.row");
    var name = row && row.querySelector(".name");
    return name && name.textContent ? name.textContent.toLowerCase() : "";
  }

  /**
   * The child <li> nodes under a node, whether it is a leaf (none) or a folder.
   * @param {Element} li
   * @returns {ArrayLike<Element>}
   */
  function childItems(li) {
    var list = li.querySelector(":scope > details > ul, :scope > ul");
    return list ? list.querySelectorAll(":scope > li") : [];
  }

  /**
   * Filter one <li> against the lowercased query, recursing into its children.
   * Force a folder on the path to a deeper match open, tagging it so clearFilter
   * can re-close only the folds the filter forced. Returns whether the <li> stays.
   * @param {Element} li
   * @param {string} query
   * @returns {boolean}
   */
  function filterItem(li, query) {
    var selfMatch = rowName(li).indexOf(query) !== -1;
    var kids = childItems(li);
    var descendantMatch = false;
    for (var i = 0; i < kids.length; i++) {
      if (filterItem(kids[i], query)) descendantMatch = true;
    }
    var visible = selfMatch || descendantMatch;
    li.classList.toggle("filter-hidden", !visible);
    var details = /** @type {HTMLDetailsElement | null} */ (li.querySelector(":scope > details"));
    if (details && descendantMatch && !details.open) {
      details.open = true;
      details.dataset.filterOpened = "1";
    }
    return visible;
  }

  /**
   * Run the filter across every root. Return how many top-level items stay visible,
   * so the caller can decide whether to show the "no matches" line.
   * @param {string} query
   * @returns {number}
   */
  function filterTree(query) {
    var q = query.toLowerCase();
    var tops = document.querySelectorAll("#roots .menu > li");
    var visible = 0;
    for (var i = 0; i < tops.length; i++) {
      if (filterItem(tops[i], q)) visible++;
    }
    return visible;
  }

  // Show the clear button only while the filter box holds text. Driven by applyFilter
  // (the input handler and the carried-q load) and by clearFilter (Escape, the
  // empty-query branch, and the button's own click).
  function toggleClear() {
    if (searchClear) searchClear.hidden = !searchInput || searchInput.value === "";
  }

  // Restore the full tree: drop every collapse mark and re-close the folders the
  // filter forced open, leaving the user's own folds untouched, and hide the line.
  function clearFilter() {
    var hidden = document.querySelectorAll("#roots .filter-hidden");
    for (var i = 0; i < hidden.length; i++) {
      hidden[i].classList.remove("filter-hidden");
    }
    var opened = /** @type {NodeListOf<HTMLDetailsElement>} */ (
      document.querySelectorAll("#roots details[data-filter-opened]")
    );
    for (var j = 0; j < opened.length; j++) {
      opened[j].open = false;
      delete opened[j].dataset.filterOpened;
    }
    if (searchEmpty) searchEmpty.hidden = true;
    syncViewLink();
    toggleClear();
  }

  // Apply the current query, restoring the tree on empty and toggling the line.
  function applyFilter() {
    if (!searchInput) return;
    toggleClear();
    var query = searchInput.value.trim();
    if (query === "") {
      clearFilter();
      return;
    }
    var visible = filterTree(query);
    if (searchEmpty) searchEmpty.hidden = visible > 0;
    syncViewLink();
  }

  // Carry the live filter on the view-toggle link so switching views (a full-page
  // navigation) lands with the filter still applied. The query rides the URL as `q`,
  // the way the view itself does. An empty query drops the param.
  function syncViewLink() {
    if (!viewLink) return;
    var query = searchInput ? searchInput.value.trim() : "";
    var url = new URL(viewLinkBase, location.origin);
    if (query) url.searchParams.set("q", query);
    viewLink.href = url.pathname + url.search;
  }

  document.addEventListener("DOMContentLoaded", function () {
    searchInput = /** @type {HTMLInputElement | null} */ (document.getElementById("search-input"));
    searchEmpty = document.getElementById("search-empty");
    searchClear = document.getElementById("search-clear");
    viewLink = /** @type {HTMLAnchorElement | null} */ (
      document.querySelector('.segmented[aria-label="View"] a.segment')
    );
    if (viewLink) viewLinkBase = viewLink.getAttribute("href") || "";
    if (searchInput) {
      searchInput.addEventListener("input", applyFilter);
      if (searchClear) {
        // Empty the box, run the clear path (restore the tree, hide the no-matches
        // line, drop q, hide this button), and hand focus back to the input.
        searchClear.addEventListener("click", function () {
          if (!searchInput) return;
          searchInput.value = "";
          clearFilter();
          searchInput.focus();
        });
      }
      // A filter carried across a view switch arrives as the q param: re-apply it,
      // which also re-syncs the toggle link so the next switch keeps it too, and
      // reveals the clear button since applyFilter toggles it.
      var carried = new URLSearchParams(location.search).get("q");
      if (carried) {
        searchInput.value = carried;
        applyFilter();
      }
      // Wired and any carried filter applied: hand over the live box.
      enableSearch();
    }
  });

  // gap summary recompute

  // The summary always reflects the whole library, not the active filter, so the
  // recompute counts flagged rows regardless of visibility, excluding only rows
  // mid-collapse (already resolved) so the count leads the delayed section swap.
  /** @type {HTMLElement | null} */
  var summary = null;

  /**
   * Gap rows under `scope`, mid-collapse rows excluded. `.leaving` rides the
   * collapsing ancestor <li>, not the flagged row inside it, so test the ancestor:
   * this lets the count drop the moment a row starts leaving, leading the delayed
   * swap, rather than waiting for the fresh section to land.
   * @param {Element} scope
   * @returns {number}
   */
  function countGapRows(scope) {
    var rows = scope.querySelectorAll(".row.flagged");
    var n = 0;
    for (var i = 0; i < rows.length; i++) {
      if (!rows[i].closest(".leaving")) n++;
    }
    return n;
  }

  // Every gap row in the tree, mid-collapse rows excluded.
  function currentGapTotal() {
    var roots = document.getElementById("roots");
    return roots ? countGapRows(roots) : 0;
  }

  /**
   * @param {string} id
   * @param {string} text
   */
  function setText(id, text) {
    var el = document.getElementById(id);
    if (el) el.textContent = text;
  }

  // Repaint the strip from the DOM: the hero total, the per-root chips, and the
  // library coverage readout and bar.
  function recomputeSummary() {
    if (!summary) return;
    var total = currentGapTotal();
    // Converge on the end-state a reload shows: the all-clear line once the live
    // total hits zero, the hero-and-bar head otherwise (an undo can bring it back).
    var clear = document.getElementById("gap-summary-clear");
    var head = document.getElementById("gap-summary-head");
    if (clear) clear.hidden = total !== 0;
    if (head) head.hidden = total === 0;
    setText("gap-total", String(total));
    var chips = document.querySelectorAll("#gap-chips .gap-chip");
    for (var i = 0; i < chips.length; i++) {
      if (chips[i].classList.contains("gap-chip-error")) continue;
      var root = chips[i].getAttribute("data-root") || "";
      var section = document.querySelector('section.root[data-root="' + CSS.escape(root) + '"]');
      var num = chips[i].querySelector(".gap-chip-num");
      if (section && num) {
        num.textContent = String(countGapRows(section));
      }
    }
    updateLibraryCoverage(total);
  }

  /**
   * Recompute the library-coverage readout from `data-total-audiobooks` on
   * each `<section class="root">`, with covered as `total - totalGaps`.
   * Errored sections carry `0` so they fold out of the sum. A section that
   * leaves the DOM stops counting. Percentage is `Math.floor`ed so 199 of
   * 200 reads "99%", never a false "100%". The two numeric spans in the
   * all-clear tail get rewritten in place, so the wording stays in the
   * server template.
   * @param {number} totalGaps
   */
  function updateLibraryCoverage(totalGaps) {
    if (!summary) return;
    /** @type {NodeListOf<HTMLElement>} */
    var sections = document.querySelectorAll("section.root");
    var total = 0;
    for (var i = 0; i < sections.length; i++) {
      total += parseInt(sections[i].dataset.totalAudiobooks || "0", 10) || 0;
    }
    var covered = Math.max(total - totalGaps, 0);
    var pct = total > 0 ? (covered / total) * 100 : 0;

    // Floor so 199 of 200 reads "99%" rather than a false "100%" beside a
    // hero that still says "1 gap to fill". Matches the server's `coverage_bar`
    // floor. The bar width stays the fractional `pct` so the fill is visually
    // accurate.
    setText("coverage-pct", String(Math.floor(pct)));
    setText("coverage-covered", String(covered));
    setText("coverage-total", String(total));

    var fill = document.getElementById("coverage-bar-fill");
    if (fill) fill.style.width = pct + "%";
    var bar = summary.querySelector(".gap-bar");
    if (bar) {
      // Floor max at 1 so a zero-total render keeps a valid ARIA range. The
      // strip is hidden in that case but the attribute still has to parse.
      bar.setAttribute("aria-valuemax", String(Math.max(total, 1)));
      bar.setAttribute("aria-valuenow", String(covered));
    }

    // The all-clear line carries the trailing "· 100% covered (T of T audiobooks)"
    // when the library has audiobooks but no gaps. For a truly empty library the
    // line stays bare so it does not read "0 of 0". Only the two numeric spans
    // get rewritten so the surrounding wording lives in the server template
    // (`render::gap_summary`) and never drifts between Rust and JS.
    var clearTail = document.getElementById("coverage-clear");
    if (clearTail) {
      if (total > 0 && totalGaps === 0) {
        setText("coverage-clear-covered", String(total));
        setText("coverage-clear-total", String(total));
        clearTail.hidden = false;
      } else {
        clearTail.hidden = true;
      }
    }
  }

  document.addEventListener("DOMContentLoaded", function () {
    summary = document.getElementById("gap-summary");
  });

  // A confirmed mark fires `marked`. Recompute now that the resolved row is
  // mid-collapse and excluded, so the count is right before the delayed swap.
  document.body.addEventListener("marked", function () {
    recomputeSummary();
  });

  /**
   * After an undo's section swap lands in gaps view, find the restored leaf and
   * slide it, and its spine, back in. Gated to the /unmark request: a mark or
   * rescan swap is left alone, and show-all undo flips a covered row in place.
   * @param {CustomEvent<HtmxAfterSwapDetail>} evt
   */
  function animateUndoRestore(evt) {
    var cfg = evt.detail && evt.detail.requestConfig;
    if (!cfg || cfg.path !== "/unmark" || !cfg.parameters) return;
    if (cfg.parameters.view !== "gaps") return;
    var section = document.querySelector(
      'section.root[data-root="' + CSS.escape(cfg.parameters.root) + '"]'
    );
    if (!section) return;
    // rel can carry quotes or slashes; CSS.escape makes it selector-safe, so
    // the restored leaf is found by its marker form's hidden input directly.
    var input = section.querySelector(
      'form.mark input[name="rel"][value="' + CSS.escape(cfg.parameters.rel) + '"]'
    );
    if (input) expandRow(input.closest("li"));
  }

  // An undo and the delayed mark swap both land as a section swap. A rescan swaps
  // all of #roots. Recompute after any of them, re-apply an active filter to the
  // fresh rows, and slide the restored row in when the swap was an undo.
  document.body.addEventListener("htmx:afterSwap", function (evt) {
    recomputeSummary();
    if (searchInput && searchInput.value.trim() !== "") applyFilter();
    animateUndoRestore(evt);
  });

  // keyboard shortcuts

  // Additive to the existing behavior. j/k move a single highlight through the
  // visible gap rows. r rescans. / focuses the filter and Enter leaves it. ? toggles
  // the settings popover. Escape clears the filter or, with an empty box, drops the
  // highlight. The highlight is a real focus target so keyboard and screen-reader
  // users land on the same row.
  /** @type {HTMLElement | null} */
  var activeRow = null;

  /**
   * A target we must not hijack typing from.
   * @param {EventTarget | null} el
   * @returns {boolean}
   */
  function isEditable(el) {
    if (!el) return false;
    var node = /** @type {HTMLElement} */ (el);
    var tag = node.tagName;
    return (
      tag === "INPUT" ||
      tag === "TEXTAREA" ||
      tag === "SELECT" ||
      node.isContentEditable
    );
  }

  /**
   * Visible gap rows in document order: flagged, not mid-collapse, and on screen
   * (offsetParent is null when hidden by the filter or inside a closed fold).
   * @returns {HTMLElement[]}
   */
  function visibleGapRows() {
    var all = /** @type {NodeListOf<HTMLElement>} */ (
      document.querySelectorAll("#roots .row.flagged:not(.leaving)")
    );
    /** @type {HTMLElement[]} */
    var out = [];
    for (var i = 0; i < all.length; i++) {
      if (all[i].offsetParent !== null) out.push(all[i]);
    }
    return out;
  }

  /**
   * Move the highlight to a row: clear the old, mark and focus the new so keyboard
   * and screen-reader users land together, and scroll it into view (instant under
   * reduced motion).
   * @param {HTMLElement | null} row
   */
  function setActiveRow(row) {
    if (activeRow && activeRow !== row) {
      activeRow.classList.remove("row-active");
      activeRow.removeAttribute("tabindex");
    }
    activeRow = row || null;
    if (!activeRow) return;
    activeRow.classList.add("row-active");
    activeRow.setAttribute("tabindex", "-1");
    activeRow.focus();
    activeRow.scrollIntoView({
      block: "nearest",
      behavior: prefersReducedMotion() ? "auto" : "smooth",
    });
  }

  // Drop the highlight entirely (Escape with an empty filter box).
  function dropHighlight() {
    if (!activeRow) return;
    activeRow.classList.remove("row-active");
    activeRow.removeAttribute("tabindex");
    if (document.activeElement === activeRow) activeRow.blur();
    activeRow = null;
  }

  /**
   * Step the highlight forward (+1) or backward (-1) through the visible gap rows,
   * clamped at both ends. With nothing highlighted, either direction lands first.
   * @param {number} delta
   */
  function moveHighlight(delta) {
    var rows = visibleGapRows();
    if (!rows.length) return;
    var idx = activeRow ? rows.indexOf(activeRow) : -1;
    var next = idx < 0 ? 0 : Math.min(Math.max(idx + delta, 0), rows.length - 1);
    setActiveRow(rows[next]);
  }

  document.addEventListener("keydown", function (evt) {
    // Escape is allowed even from the filter box: clear the query if it holds one,
    // otherwise drop the highlight. Toasts are cleared by their own listener.
    if (evt.key === "Escape") {
      if (searchInput && searchInput.value) {
        searchInput.value = "";
        clearFilter();
      } else {
        dropHighlight();
      }
      return;
    }
    // Enter in the filter box just drops focus: the filter is already live, so this
    // commits nothing and leaves the query in place, handing the keyboard back to
    // j/k navigation without Escape clearing it.
    if (evt.key === "Enter" && searchInput && evt.target === searchInput) {
      evt.preventDefault();
      searchInput.blur();
      return;
    }
    // Every other shortcut is suppressed while typing in a field.
    if (isEditable(evt.target)) return;
    // Don't fight browser or OS chords.
    if (evt.metaKey || evt.ctrlKey || evt.altKey) return;
    switch (evt.key) {
      case "j":
        evt.preventDefault();
        moveHighlight(1);
        break;
      case "k":
        evt.preventDefault();
        moveHighlight(-1);
        break;
      case "r":
        evt.preventDefault();
        var rescanBtn = /** @type {HTMLButtonElement | null} */ (document.getElementById("rescan-btn"));
        if (rescanBtn && !rescanBtn.disabled) rescanBtn.click();
        break;
      case "/":
        evt.preventDefault();
        if (searchInput) {
          searchInput.focus();
          searchInput.select();
        }
        break;
      case "?":
        evt.preventDefault();
        var settingsPanel = document.getElementById("settings-panel");
        if (settingsPanel && typeof settingsPanel.togglePopover === "function") {
          settingsPanel.togglePopover();
        }
        break;
      default:
        break;
    }
  });

  // refresh swap guards
  //
  // The poll below replaces #roots innerHTML wholesale. A bare swap resets
  // every <details> to its server-rendered default (root folds pop back open)
  // and, when focus sits inside #roots, removes the focused element, which
  // jumps scroll to the document bottom (the same hazard the mark path blurs
  // around). Three guards: skip the swap when the response matches the last
  // one applied, blur focus out of #roots before a real swap, and put each
  // fold back in its pre-swap state after it.

  /**
   * Last /refresh body applied to #roots, for the identical-response skip.
   * @type {string | null}
   */
  var lastRefreshBody = null;

  /**
   * Fold state captured before a refresh swap, keyed by `foldKey`.
   * @type {{ [key: string]: boolean } | null}
   */
  var savedFolds = null;

  /**
   * True when a swap event belongs to the background /refresh poll.
   * @param {Event} evt
   * @returns {boolean}
   */
  function isRefreshSwap(evt) {
    var detail = /** @type {CustomEvent} */ (evt).detail;
    var path = detail.pathInfo && detail.pathInfo.requestPath;
    return typeof path === "string" && path.indexOf("/refresh") === 0;
  }

  /**
   * Stable identity for a fold across an innerHTML swap: the section id plus
   * the summary labels on the ancestor `<details>` chain.
   * @param {Element} details
   * @returns {string}
   */
  function foldKey(details) {
    /** @type {string[]} */
    var parts = [];
    for (var el = /** @type {Element | null} */ (details); el && el.id !== "roots"; el = el.parentElement) {
      if (el.tagName === "DETAILS") {
        var label = el.querySelector(":scope > summary .name, :scope > summary h2");
        parts.unshift(label ? label.textContent || "" : "");
      } else if (el.classList.contains("root")) {
        parts.unshift(el.id);
      }
    }
    return parts.join("/");
  }

  document.body.addEventListener("htmx:beforeSwap", function (evt) {
    var detail = /** @type {CustomEvent} */ (evt).detail;
    if (!detail.shouldSwap || !isRefreshSwap(evt)) return;
    if (detail.serverResponse === lastRefreshBody) {
      detail.shouldSwap = false;
      return;
    }
    lastRefreshBody = detail.serverResponse;
    var roots = document.getElementById("roots");
    if (!roots) return;
    var active = document.activeElement;
    if (active instanceof HTMLElement && roots.contains(active)) active.blur();
    savedFolds = {};
    var folds = roots.querySelectorAll("details");
    for (var i = 0; i < folds.length; i++) {
      savedFolds[foldKey(folds[i])] = /** @type {HTMLDetailsElement} */ (folds[i]).open;
    }
  });

  document.body.addEventListener("htmx:afterSwap", function (evt) {
    if (!savedFolds || !isRefreshSwap(evt)) return;
    var roots = document.getElementById("roots");
    if (roots) {
      var folds = roots.querySelectorAll("details");
      for (var i = 0; i < folds.length; i++) {
        var open = savedFolds[foldKey(folds[i])];
        if (typeof open === "boolean") /** @type {HTMLDetailsElement} */ (folds[i]).open = open;
      }
    }
    savedFolds = null;
  });

  // client-driven refresh: see ADR-0034
  //
  // On load, read the poll marker's data-poll-interval and data-view. When
  // the interval is nonzero, start a setInterval that hits /refresh?view=...
  // and swaps the response into #roots, gated on document.visibilityState so
  // a hidden tab pays zero wire cost. A visibilitychange listener fires an
  // immediate poll on 'visible' so a refocused tab does not sit stale for
  // up to poll_interval_seconds.
  document.addEventListener("DOMContentLoaded", function () {
    var found = document.getElementById("poll-root");
    if (!found) return;
    var root = found;
    var intervalSecs = parseInt(root.dataset.pollInterval || "0", 10);
    if (!(intervalSecs > 0)) return;
    var inFlight = false;
    function pollOnce() {
      if (inFlight) return;
      if (document.visibilityState !== "visible") return;
      var view = root.dataset.view;
      if (!view) return;
      inFlight = true;
      var done = function () {
        inFlight = false;
      };
      htmx
        .ajax("GET", "/refresh?view=" + encodeURIComponent(view), {
          target: "#roots",
          swap: "innerHTML",
        })
        .then(done, done);
    }
    setInterval(pollOnce, intervalSecs * 1000);
    document.addEventListener("visibilitychange", function () {
      if (document.visibilityState === "visible") pollOnce();
    });
  });
})();
