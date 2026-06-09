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
    // Pin the current height, then drop to zero next frame so the transition has a
    // definite start. The .leaving class owns the timing, fade, and reduced-motion.
    li.style.maxHeight = li.scrollHeight + "px";
    li.classList.add("leaving");
    requestAnimationFrame(function () {
      li.style.maxHeight = "0";
    });
  });
})();
