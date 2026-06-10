// missing-ebooks client behavior. The pre-paint theme resolver runs inline in
// <head>; this file owns the rest: the theme control, the settings panel sync,
// and the marker-write confirmation. Loaded at the end of <body>, after htmx.
(function () {
  "use strict";

  var THEME_KEY = "theme";
  var CONFIRM_KEY = "confirmMarks";
  var darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

  // ---- theme ----

  // The theme to paint for a stored choice. "system" and an absent value follow
  // the OS preference.
  function resolveTheme(choice) {
    if (choice === "light" || choice === "dark") return choice;
    return darkQuery.matches ? "dark" : "light";
  }

  // The stored choice, normalized so an absent or unknown value reads as "system".
  function storedTheme() {
    var saved = localStorage.getItem(THEME_KEY);
    return saved === "light" || saved === "dark" ? saved : "system";
  }

  // Apply a choice, persist it, and highlight the matching segment.
  function setTheme(choice) {
    localStorage.setItem(THEME_KEY, choice);
    document.documentElement.dataset.theme = resolveTheme(choice);
    markActiveTheme(choice);
  }

  // Mark the active theme segment and clear the others.
  function markActiveTheme(choice) {
    var segs = document.querySelectorAll("[data-theme-choice]");
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
    }
  });

  // ---- confirm-before-marking preference ----

  // On by default: only the literal "off" disables it, so the key need not exist.
  function confirmEnabled() {
    return localStorage.getItem(CONFIRM_KEY) !== "off";
  }

  function setConfirmEnabled(on) {
    localStorage.setItem(CONFIRM_KEY, on ? "on" : "off");
  }

  // Sync the settings controls from storage. Runs on load and whenever the panel
  // opens, so the switch reflects a "Don't ask again" choice made in the dialog.
  function syncSettings() {
    markActiveTheme(storedTheme());
    var sw = document.getElementById("confirm-toggle");
    if (sw) sw.checked = confirmEnabled();
  }

  document.addEventListener("DOMContentLoaded", function () {
    syncSettings();

    var panel = document.getElementById("settings-panel");
    if (panel) panel.addEventListener("toggle", syncSettings);

    var segs = document.querySelectorAll("[data-theme-choice]");
    for (var i = 0; i < segs.length; i++) {
      segs[i].addEventListener("click", function () {
        setTheme(this.dataset.themeChoice);
      });
    }

    var sw = document.getElementById("confirm-toggle");
    if (sw) {
      sw.addEventListener("change", function () {
        setConfirmEnabled(this.checked);
      });
    }
  });

  // ---- marker-write confirmation ----

  var dialog = null;
  var pending = null;

  // Fill the dialog from the button that fired, and reveal its marker glyph.
  function fillDialog(btn) {
    var action = btn.dataset.confirmAction;
    var file = btn.dataset.confirmFile;
    document.getElementById("confirm-title").textContent = action + "?";
    document.getElementById("confirm-folder").textContent = btn.dataset.confirmFolder;
    document.getElementById("confirm-file").textContent = file;
    document.getElementById("confirm-accept-label").textContent = action;
    var icons = dialog.querySelectorAll("[data-confirm-icon]");
    for (var i = 0; i < icons.length; i++) {
      icons[i].hidden = icons[i].dataset.confirmIcon !== file;
    }
    document.getElementById("confirm-again").checked = false;
  }

  // Send the held request. Capture it before close(), since the close handler
  // clears pending.
  function acceptConfirm() {
    var held = pending;
    pending = null;
    if (document.getElementById("confirm-again").checked) {
      setConfirmEnabled(false);
    }
    dialog.close();
    if (held) held.issueRequest(true);
  }

  document.addEventListener("DOMContentLoaded", function () {
    dialog = document.getElementById("confirm-mark");
    if (!dialog) return;
    document.getElementById("confirm-accept").addEventListener("click", acceptConfirm);
    document.getElementById("confirm-cancel").addEventListener("click", function () {
      dialog.close();
    });
    // Esc, a backdrop click, or Cancel all fire close: drop the held request so
    // nothing is sent.
    dialog.addEventListener("close", function () {
      pending = null;
    });
  });

  // htmx fires htmx:confirm before every request. Only /mark uses htmx here, and
  // we still gate on the button's data, so no other request is ever intercepted.
  document.body.addEventListener("htmx:confirm", function (evt) {
    // A programmatic resend already had the user's intent; don't re-prompt.
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

  // ---- mark: collapse the leaving row ----

  // The moment a gaps-only mark request goes out, collapse the marked folder's row
  // and fade it, so the rows below glide up through normal reflow. The section's
  // swap is delayed (the marker form's hx-swap "swap:" modifier) to let this play,
  // then it reconciles the fresh section. In show-all the row stays (it flips to
  // covered in place), so that request carries view=all and we leave it alone.
  document.body.addEventListener("htmx:beforeRequest", function (evt) {
    var btn = evt.detail.elt;
    if (!btn || !btn.matches('[hx-post="/mark"]')) return;
    // The section swap removes this button. If it still holds focus when that
    // happens, the browser jumps the scroll to the document bottom (in both views),
    // so drop focus first. Focus would end up on <body> after the swap regardless.
    if (document.activeElement === btn) btn.blur();
    var form = btn.closest("form.mark");
    var view = form && form.querySelector('input[name="view"]');
    if (view && view.value === "all") return;
    var li = btn.closest("li");
    if (!li) return;
    // A resend (retry) fires beforeRequest again; the row is already collapsing, so
    // don't re-pin its height or restart the transition.
    if (li.classList.contains("leaving")) return;
    // Pin the current height, then drop to zero next frame so the transition has a
    // definite start. The .leaving class owns the timing, fade, and reduced-motion.
    li.style.maxHeight = li.scrollHeight + "px";
    li.classList.add("leaving");
    requestAnimationFrame(function () {
      li.style.maxHeight = "0";
    });
  });

  // ---- connection status: detection + banner ----

  // htmx has no request timeout by default. A generous backstop frees a truly hung
  // request without aborting a legitimately slow big-library rescan.
  if (window.htmx && window.htmx.config) window.htmx.config.timeout = 30000;

  var connBanner = null;
  var reconnectTimer = null;

  // Copy for a state, read from the banner's data-msg-* attributes
  // (data-msg-offline -> dataset.msgOffline, and so on).
  function bannerMsg(state) {
    var key = "msg" + state.charAt(0).toUpperCase() + state.slice(1);
    return (connBanner && connBanner.dataset[key]) || "";
  }

  // Reveal the banner in a state, with optional action-specific copy override.
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

  // True while the banner is showing something the user should see resolve.
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

  // ---- connection failures: classify, roll back a mark, offer a retry ----

  var KIND_LABEL = { no_ebook: "None", ebook_elsewhere: "Elsewhere" };
  var suppressConfirm = false;

  // The op a request belongs to ("mark" / "rescan"), or null if we don't manage it.
  function opOf(elt) {
    var post = elt && elt.getAttribute && elt.getAttribute("hx-post");
    if (post === "/mark") return "mark";
    if (post === "/rescan") return "rescan";
    return null;
  }

  // A failure worth retrying: a dropped connection, a timeout, or a gateway error
  // from a proxy / restarting server. A plain 4xx/5xx is a real server error.
  function isRetryable(kind, xhr) {
    if (kind === "sendError" || kind === "timeout") return true;
    if (kind === "responseError" && xhr) {
      return xhr.status === 502 || xhr.status === 503 || xhr.status === 504;
    }
    return false;
  }

  function formValues(form) {
    var v = {};
    form.querySelectorAll("input[name]").forEach(function (i) {
      v[i.name] = i.value;
    });
    return v;
  }

  // Re-send a request htmx already sent, reusing its verb, target, swap, and values.
  // suppressConfirm keeps the mark confirm dialog from re-prompting on a resend.
  function reissue(elt, op) {
    if (op === "mark") {
      // elt is the mark button; its form holds the hidden fields and the hx-swap.
      var form = elt.closest("form.mark");
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

  // Re-send a failed action after the user clicks its inline Retry.
  function manualRetry(elt, op) {
    var li = elt.closest("li");
    if (li) {
      var box = li.querySelector(":scope > .mark-failed");
      if (box) box.remove();
    }
    reissue(elt, op);
  }

  // Roll a failed mark back: undo the optimistic collapse and show an inline error
  // with a Retry that re-sends the same mark.
  function markTerminalFailure(elt) {
    var li = elt.closest("li");
    if (li) {
      li.classList.remove("leaving");
      li.style.maxHeight = "";
      var existing = li.querySelector(":scope > .mark-failed");
      if (existing) existing.remove();
      var kind = JSON.parse(elt.getAttribute("hx-vals") || "{}").kind;
      var box = document.createElement("div");
      box.className = "mark-failed";
      var msg = document.createElement("span");
      msg.className = "mark-failed-msg";
      msg.textContent = 'Couldn’t save “' + (KIND_LABEL[kind] || "this") + '”.';
      var retry = document.createElement("button");
      retry.type = "button";
      retry.className = "btn btn-outline btn-xs mark-retry";
      retry.textContent = "↻ Retry";
      retry.addEventListener("click", function () {
        manualRetry(elt, "mark");
      });
      box.appendChild(msg);
      box.appendChild(retry);
      li.appendChild(box);
    }
    showBanner("failed");
  }

  // Task 5 replaces this with bounded auto-retry. For now a failure is terminal.
  function handleFailure(elt, op, retryable) {
    if (op === "mark") markTerminalFailure(elt);
  }

  ["htmx:sendError", "htmx:timeout", "htmx:responseError"].forEach(function (type) {
    document.body.addEventListener(type, function (evt) {
      var elt = evt.detail.elt;
      var op = opOf(elt);
      if (!op) return;
      var kind = type.slice("htmx:".length);
      handleFailure(elt, op, isRetryable(kind, evt.detail.xhr));
    });
  });

  // A successful request clears a problem banner (with a brief Reconnected flash).
  document.body.addEventListener("htmx:afterRequest", function (evt) {
    if (!evt.detail.successful) return;
    if (!opOf(evt.detail.elt)) return;
    if (bannerShowsProblem()) flashReconnected();
  });
})();
