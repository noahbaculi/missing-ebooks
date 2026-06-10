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

  // htmx fires htmx:confirm before every request. /mark and /unmark both go
  // through htmx, and we still gate on the button's confirm data, so the undo
  // POST and every other request flow through untouched by the dialog.
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

  // ---- mark: hold the row in place while saving, collapse only once confirmed ----

  // Collapse a row's <li> and fade it so the rows below glide up through normal
  // reflow. The section swap is delayed (the marker form's hx-swap "swap:" modifier)
  // to let this play, then it reconciles the fresh section. li.leaving owns the
  // timing, fade, and reduced-motion.
  function collapseRow(li) {
    if (!li || li.classList.contains("leaving")) return;
    // Pin the current height, then drop to zero next frame so the transition has a
    // definite start.
    li.style.maxHeight = li.scrollHeight + "px";
    li.classList.add("leaving");
    requestAnimationFrame(function () {
      li.style.maxHeight = "0";
    });
  }

  // Toggle a gaps-only row's in-flight "saving" state: dim it, hide its actions, and
  // show a spinner with a "Saving…" label. Idempotent, so a retry's beforeRequest is a
  // no-op rather than a second spinner.
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

  // True for a gaps-only mark request (the kind whose row should collapse on success).
  // A show-all mark carries view=all and stays put, flipping to covered in place.
  function isCollapsingMark(btn) {
    if (!btn || !btn.matches || !btn.matches('[hx-post="/mark"]')) return false;
    var form = btn.closest("form.mark");
    var view = form && form.querySelector('input[name="view"]');
    return !(view && view.value === "all");
  }

  // The moment a gaps-only mark goes out, hold its row in place in the "saving" state.
  // The row must not look handled before the write lands, so it stays visible and only
  // collapses once the server confirms (htmx:beforeOnLoad below).
  document.body.addEventListener("htmx:beforeRequest", function (evt) {
    var btn = evt.detail.elt;
    if (!isCollapsingMark(btn)) return;
    // The section swap removes this button. If it still holds focus when that happens,
    // the browser jumps the scroll to the document bottom, so drop focus first.
    if (document.activeElement === btn) btn.blur();
    setSaving(btn.closest(".row"), true);
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

  // A small warning glyph for the inline mark-failure strip, in the row's error color.
  var MARK_WARN_SVG =
    '<svg viewBox="0 0 24 24" width="100%" height="100%" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>';

  // A folder renders as a leaf <div.row> or a <summary.row> inside <details>; return
  // whichever this <li> has, so the failure strip can sit right under the folder's own
  // line rather than after its whole child subtree.
  function rowOf(li) {
    return li.querySelector(":scope > .row") || li.querySelector(":scope > details > .row");
  }

  // Remove the failure strip belonging to this element's row (its immediate next
  // sibling), if one is showing.
  function clearMarkFailed(elt) {
    var li = elt.closest("li");
    var row = li && rowOf(li);
    var box = row && row.nextElementSibling;
    if (box && box.classList.contains("mark-failed")) box.remove();
  }

  // Re-send a failed action after the user clicks its inline Retry. Starts a fresh
  // retry sequence.
  function manualRetry(elt, op) {
    retryState.delete(elt);
    clearMarkFailed(elt);
    reissue(elt, op);
  }

  // Roll a failed mark back: undo the optimistic collapse and attach an inline error
  // directly under the folder's own row, naming the folder, with a Retry that re-sends
  // the same mark. The original mark buttons stay on the row (the section never
  // swapped), so the user can also just pick again.
  function markTerminalFailure(elt) {
    var li = elt.closest("li");
    if (li) {
      // The row was held in the "saving" state (never collapsed), so just release it;
      // the defensive .leaving cleanup covers a manual retry that did collapse first.
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
      icon.innerHTML = MARK_WARN_SVG;
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

  // Retry a transient failure a bounded number of times with backoff; once exhausted
  // (or for a non-retryable failure) fall through to the terminal handler.
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

  ["htmx:sendError", "htmx:timeout", "htmx:responseError"].forEach(function (type) {
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
    // The rescan retry-highlight only clears when a rescan itself succeeds; an
    // unrelated successful mark must not strip the "still needs rescanning" cue.
    if (op === "rescan") {
      rescanRetryHold(false);
      var btn = document.getElementById("rescan-btn");
      if (btn) btn.classList.remove("conn-retry-hl");
    }
    if (bannerShowsProblem()) flashReconnected();
  });
  // ---- bounded auto-retry for both idempotent endpoints ----

  var MAX_RETRIES = 3;
  var BACKOFFS = [500, 1500, 3000];
  var retryState = new WeakMap(); // request element -> { attempts }

  // Hold (or release) the rescan skeleton and busy button across backoff gaps, so
  // the loading state does not flicker between attempts.
  function rescanRetryHold(on) {
    var sk = document.getElementById("scan-skeleton");
    var btn = document.getElementById("rescan-btn");
    if (sk) sk.classList.toggle("is-retrying", on);
    if (btn) {
      btn.classList.toggle("is-retrying", on);
      btn.disabled = on;
    }
  }

  // A rescan that failed for good: drop the skeleton, re-enable the Rescan button,
  // and highlight it as the retry.
  function rescanTerminalFailure() {
    rescanRetryHold(false);
    var btn = document.getElementById("rescan-btn");
    if (btn) {
      btn.disabled = false;
      btn.classList.remove("htmx-request");
      btn.classList.add("conn-retry-hl");
    }
    showBanner("failed", connBanner ? connBanner.dataset.msgFailedRescan : null);
  }

  function terminalFailure(elt, op) {
    retryState.delete(elt);
    if (op === "mark") markTerminalFailure(elt);
    else rescanTerminalFailure();
  }

  // ---- action toast stack (undo + errors) ----

  // Up to three toasts coexist; a fourth evicts the oldest. A success toast
  // offers an undo and clears after SUCCESS_MS; an error carries no undo and
  // lingers longer.
  var stack = null;
  var template = null;
  var MAX_TOASTS = 3;
  var SUCCESS_MS = 8000;
  var ERROR_MS = 15000;
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
  var KIND_LABEL = { no_ebook: "No ebook", ebook_elsewhere: "Ebook elsewhere" };

  // Remove a toast immediately, clearing its dismiss timer. Used when the stack
  // evicts to stay within MAX_TOASTS and when Escape clears everything at once.
  function hardRemove(node) {
    if (node._timer) clearTimeout(node._timer);
    node.remove();
  }

  // Animate a toast out, then remove it. Guards against a second trigger, e.g. a
  // close click landing while the auto-dismiss is already playing.
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
    while (stack.children.length >= MAX_TOASTS) {
      hardRemove(stack.firstElementChild);
    }
  }

  // A fresh toast node cloned from the page template.
  function newToastNode() {
    return template.content.firstElementChild.cloneNode(true);
  }

  // A fresh element with a class and optional text.
  function el(tag, cls, text) {
    var node = document.createElement(tag);
    node.className = cls;
    if (text != null) node.textContent = text;
    return node;
  }

  // Arm (or re-arm) a toast's auto-dismiss for `ms` from now, remembering when
  // it started and how much time is left so a pause can bank the remainder.
  function armToast(node, ms) {
    node._remaining = ms;
    node._start = Date.now();
    node._timer = setTimeout(function () {
      dismissToast(node);
    }, ms);
  }

  // Pause the auto-dismiss and bank the time left on the clock. Hover and focus
  // each pause; a second pause while already paused is a no-op.
  function pauseToast(node) {
    if (node._leaving || !node._timer) return;
    clearTimeout(node._timer);
    node._timer = null;
    node._remaining -= Date.now() - node._start;
  }

  // Resume from the banked remainder, but only once the toast is neither hovered
  // nor focused, so releasing one hold while the other still stands keeps it
  // paused.
  function resumeToast(node) {
    if (node._leaving || node._timer || node._hovered || node._focused) return;
    armToast(node, Math.max(node._remaining, 0));
  }

  // Whether the viewer asked for less motion. The reflow slide honors it the
  // same way the CSS animations do (the reduced-motion block already stills the
  // entry and exit), so a new toast simply appears in place.
  function prefersReducedMotion() {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  // Add a toast to the stack, sliding the toasts already there from their old
  // positions to their new ones instead of letting them jump up. FLIP: record
  // each current top (First), append the newcomer so the layout settles (Last),
  // offset each existing toast back to where it sat (Invert), then transition
  // that offset away on the next frame (Play).
  function reflowStack(node) {
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

  // Append a built node, wire its close button and pause-on-interaction, and
  // start its dismiss timer.
  function pushToast(node, timeoutMs) {
    evictOldest();
    node.querySelector(".toast-close").addEventListener("click", function () {
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
    // focusout also fires when focus moves between controls inside the toast;
    // only treat it as leaving when focus has actually left the node.
    node.addEventListener("focusout", function (evt) {
      if (node.contains(evt.relatedTarget)) return;
      node._focused = false;
      resumeToast(node);
    });
    armToast(node, timeoutMs);
    reflowStack(node);
  }

  // Show the success variant: an undo offer that clears after SUCCESS_MS.
  function showSuccessToast(detail) {
    if (!stack || !template || !detail) return;
    var node = newToastNode();
    node.classList.add("toast--success");
    node.setAttribute("role", "status");
    node.setAttribute("aria-live", "polite");
    node.querySelector(".toast-undo").addEventListener("click", function () {
      dismissToast(node);
      htmx.ajax("POST", "/unmark", {
        target: '[data-root="' + detail.root + '"]',
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
    node.querySelector(".toast-msg").append(name, outcome);
  }

  // Show the error variant: the server message, no undo, cleared after ERROR_MS.
  function showErrorToast(detail) {
    if (!stack || !template) return;
    var node = newToastNode();
    node.classList.add("toast--error");
    node.setAttribute("role", "alert");
    node.setAttribute("aria-live", "assertive");
    node.querySelector(".toast-undo").remove();
    pushToast(node, ERROR_MS);
    node.querySelector(".toast-msg").textContent =
      detail && detail.message ? detail.message : "Something went wrong";
  }

  // Clear the whole stack at once (Escape) without waiting on exit animations.
  function clearToasts() {
    if (!stack) return;
    while (stack.firstElementChild) hardRemove(stack.firstElementChild);
  }

  document.addEventListener("DOMContentLoaded", function () {
    stack = document.getElementById("toast-stack");
    template = document.getElementById("toast-template");
  });

  // htmx dispatches these from the HX-Trigger header on the /mark and /unmark
  // responses; they bubble to the body.
  document.body.addEventListener("marked", function (evt) {
    showSuccessToast(evt.detail);
  });
  document.body.addEventListener("app-error", function (evt) {
    showErrorToast(evt.detail);
  });
  document.addEventListener("keydown", function (evt) {
    if (evt.key === "Escape") clearToasts();
  });
})();
