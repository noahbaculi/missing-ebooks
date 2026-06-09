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
})();
