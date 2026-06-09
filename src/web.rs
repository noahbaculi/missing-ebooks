//! axum router, request handlers, and Maud markup. Handlers are thin: they call a
//! `service` operation and render. Handlers return `Html<String>` so Maud stays
//! decoupled from the axum version. Marker writes use htmx to swap just the
//! affected root's section; the script is vendored and served from `/static`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Deserialize;

use crate::config::SearchLink;
use crate::marker::Marker;
use crate::query::clean_query;
use crate::service::{self, FlaggedView, RootSection, RootState, ViewMode};
use crate::state::AppState;
use crate::tree::Node;

/// The vendored htmx runtime, embedded at compile time and served from `/static`.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

/// The hand-rolled stylesheet, embedded at compile time and served from `/static`.
const APP_CSS: &str = include_str!("../assets/app.css");

/// The client behavior script, embedded at compile time and served from `/static`.
const APP_JS: &str = include_str!("../assets/app.js");

/// Pre-paint theme bootstrap: resolves the saved choice, or the OS preference for
/// "system" / an unset value, and sets `data-theme` on <html> before first paint
/// so there is no flash. The interactive theme control lives in `app.js`.
const PREPAINT_THEME_JS: &str = r#"(function () {
  var saved = localStorage.getItem('theme');
  var t = (saved === 'light' || saved === 'dark')
    ? saved
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.documentElement.dataset.theme = t;
})();"#;

/// Gear glyph for the settings menu trigger. Inherits `currentColor`.
const COG_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"##;

/// Caret that rotates open when its folder is expanded. Inherits `currentColor`.
const CHEVRON_SVG: &str = r##"<svg class="chev" viewBox="0 0 16 16" fill="currentColor"><path d="M6 4l4 4-4 4z"/></svg>"##;

/// Magnifying glass for the search-links dropdown trigger. Inherits `currentColor`.
const SEARCH_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.35-4.35"/></svg>"##;

/// Folder glyph shown on every node row. Inherits `currentColor`.
const FOLDER_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"##;

/// Check mark for the "no gaps in this root" state. Inherits `currentColor`.
const CHECK_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 13l4 4L19 7"/></svg>"##;

/// Circled exclamation for a scan or write error. Inherits `currentColor`.
const ERROR_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M12 8v4M12 16h.01"/></svg>"##;

/// The favicon as an inline SVG data URI, so the tab gets an identity and the
/// browser stops requesting `/favicon.ico`. The "book wearing headphones" glyph
/// on its own, no backdrop. It draws in `currentColor`, and an embedded `<style>`
/// binds that to indigo `%23605dff` on light tab strips and a lighter indigo
/// `%23c7c5ff` on dark ones via `prefers-color-scheme`, so the mark keeps its
/// contrast either way. (Chrome, Firefox, and Edge honor the media query inside a
/// favicon; Safari ignores it and shows the light-mode indigo throughout.) The
/// source art lives at `assets/brand/favicon.svg`; keep the two in sync if the
/// mark changes.
const FAVICON_HREF: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-linecap='round' stroke-linejoin='round'%3E%3Cstyle%3Esvg{color:%23605dff}@media(prefers-color-scheme:dark){svg{color:%23c7c5ff}}%3C/style%3E%3Cpath d='M4.5 14v-2a7.5 7.5 0 0 1 15 0v2' stroke-width='2'/%3E%3Crect x='3' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Crect x='17.8' y='13' width='3.2' height='6' rx='1.6' fill='currentColor' stroke='none'/%3E%3Cpath d='M12 11.8c-1.2-.85-3-.85-4.2 0v4.8c1.2-.85 3-.85 4.2 0c1.2-.85 3-.85 4.2 0v-4.8c-1.2-.85-3-.85-4.2 0z' stroke-width='1.4'/%3E%3Cpath d='M12 11.8v4.8' stroke-width='1.2'/%3E%3C/svg%3E";

/// Vertical three-dot "more actions" glyph for the mobile per-row menu trigger.
/// Inherits `currentColor`.
const KEBAB_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/></svg>"##;

/// A "no entry" sign (circle with a horizontal bar) for the sheet's "Mark as
/// None" row. Shown only inside the mobile sheet. Inherits `currentColor`.
const NO_ENTRY_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M7 12h10"/></svg>"##;

/// A book with a small check, marking that this audiobook's ebook is accounted
/// for somewhere else rather than missing. Shown on the sheet's "Ebook
/// elsewhere" button. Inherits `currentColor`.
const EBOOK_ELSEWHERE_SVG: &str = r##"<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"/><path d="m9 9.5 2 2 4-4"/></svg>"##;

/// The `view` parameter shared by the index query string and the rescan form. A
/// lenient `Option<String>` so an absent or unknown value falls back to gaps-only
/// rather than rejecting the request.
#[derive(Deserialize)]
pub(crate) struct ViewQuery {
    #[serde(default)]
    pub(crate) view: Option<String>,
}

/// The body of a marker write: which root, which folder, which marker, and which
/// view the click came from (so the swapped section comes back in that mode).
#[derive(Deserialize)]
pub(crate) struct MarkRequest {
    pub(crate) root: usize,
    pub(crate) rel: String,
    pub(crate) kind: Marker,
    #[serde(default)]
    pub(crate) view: ViewMode,
}

/// Build the application router with the shared state attached.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/mark", post(mark))
        .route("/unmark", post(unmark))
        .route("/rescan", post(rescan))
        .route("/static/htmx.min.js", get(htmx_script))
        .route("/static/app.css", get(app_css))
        .route("/static/app.js", get(app_js))
        .with_state(state)
}

async fn index(State(state): State<Arc<AppState>>, Query(query): Query<ViewQuery>) -> Html<String> {
    let mode = ViewMode::from_query(query.view.as_deref());
    let view = service::current_view(&state, mode).await;
    Html(page(&view, &state.config.search_links, mode).into_string())
}

async fn mark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let links = &state.config.search_links;
    let mode = req.view;
    match service::mark(&state, req.root, &req.rel, req.kind, mode).await {
        Ok(outcome) => {
            let markup = render_section(&outcome.view[req.root], req.root, None, links, mode);
            let trigger = outcome.created.then(|| {
                let name = display_name(&outcome.view[req.root].path, &req.rel);
                marked_trigger(&req, &name)
            });
            section_response(markup, trigger)
        }
        Err(err) => {
            let message = format!("Could not mark {}: {err}", req.rel);
            let view = service::current_view(&state, mode).await;
            // Leave the tree intact: re-render the section with no inline alert and
            // carry the message to the toast instead.
            let markup = match view.get(req.root) {
                Some(section) => render_section(section, req.root, None, links, mode),
                None => html! { section.card.root data-root=(req.root) {} },
            };
            section_response(markup, Some(error_trigger(&message)))
        }
    }
}

async fn unmark(
    State(state): State<Arc<AppState>>,
    Form(req): Form<MarkRequest>,
) -> axum::response::Response {
    let links = &state.config.search_links;
    let mode = req.view;
    match service::unmark(&state, req.root, &req.rel, req.kind, mode).await {
        Ok(view) => {
            section_response(render_section(&view[req.root], req.root, None, links, mode), None)
        }
        Err(err) => {
            let message = format!("Could not undo {}: {err}", req.rel);
            let view = service::current_view(&state, mode).await;
            let markup = match view.get(req.root) {
                Some(section) => render_section(section, req.root, None, links, mode),
                None => html! { section.card.root data-root=(req.root) {} },
            };
            section_response(markup, Some(error_trigger(&message)))
        }
    }
}

async fn rescan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(query): Form<ViewQuery>,
) -> Response {
    let mode = ViewMode::from_query(query.view.as_deref());
    let view = service::rescan(&state, mode).await;
    if headers.contains_key("HX-Request") {
        // htmx path: swap the fresh sections into #roots, and push the mode path so
        // the address bar tracks the view without ever showing the /rescan POST URL.
        let markup = roots(&view, &state.config.search_links, mode);
        (
            [("HX-Push-Url", mode_path(mode))],
            Html(markup.into_string()),
        )
            .into_response()
    } else {
        // no-JS path: 303 See Other (Post/Redirect/Get), so a refresh does not
        // re-trigger a scan.
        Redirect::to(mode_path(mode)).into_response()
    }
}

/// The path that renders a given mode, for redirects and links.
fn mode_path(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::GapsOnly => "/",
        ViewMode::All => "/?view=all",
    }
}

/// The gaps-only / show-all view control for the navbar: a two-segment control.
/// The segment for the current view is inert and marked `aria-current`; the other
/// is a GET link that navigates to its view. Switching reshapes every root, so it
/// is a full-page navigation, and the choice is not persisted.
fn view_toggle(mode: ViewMode) -> Markup {
    html! {
        div.segmented role="group" aria-label="View" {
            @match mode {
                ViewMode::GapsOnly => {
                    span.segment.segment-active aria-current="page" { "Gaps only" }
                    a.segment href="/?view=all" { "All folders" }
                }
                ViewMode::All => {
                    a.segment href="/" { "Gaps only" }
                    span.segment.segment-active aria-current="page" { "All folders" }
                }
            }
        }
    }
}

pub(crate) async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript;charset=utf-8")],
        HTMX_JS,
    )
}

pub(crate) async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css;charset=utf-8")], APP_CSS)
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript;charset=utf-8")],
        APP_JS,
    )
}

/// The navbar settings control: a cog that opens a popover panel holding the
/// theme choice and the confirm-before-marking toggle. The native popover API
/// drives open/close; the controls' behavior lives in `app.js`. The theme
/// segments and the switch render with their default state (System, confirm on);
/// `app.js` reconciles them against localStorage once it runs.
fn settings_menu() -> Markup {
    html! {
        button.btn.btn-ghost.btn-square.settings-cog type="button"
            aria-label="Settings"
            title="Settings"
            aria-haspopup="menu"
            popovertarget="settings-panel" { (PreEscaped(COG_SVG)) }
        div.settings-panel id="settings-panel" popover="auto" aria-label="Settings" {
            div.settings-head { "Settings" }
            div.settings-row.settings-row-theme {
                span.settings-label { "Theme" }
                div.segmented role="group" aria-label="Theme" {
                    button.segment type="button" data-theme-choice="light" { "Light" }
                    button.segment type="button" data-theme-choice="dark" { "Dark" }
                    button.segment.segment-active type="button"
                        data-theme-choice="system" aria-current="true" { "System" }
                }
            }
            div.settings-row {
                span.settings-label {
                    "Confirm before marking"
                    span.settings-sub { "Ask before writing a marker" }
                }
                label.switch {
                    input id="confirm-toggle" type="checkbox" checked;
                    span.switch-track {}
                }
            }
        }
    }
}

/// The marker-write confirmation, rendered once at the page level so it survives
/// the htmx section swaps. `app.js` fills the title, folder, file chip, confirm
/// label, and the matching marker glyph from the button that fired, then opens
/// it. Without JS the dialog never opens and writes proceed as before.
fn confirm_dialog() -> Markup {
    html! {
        dialog.confirm-dialog id="confirm-mark" aria-labelledby="confirm-title" {
            h2.confirm-title id="confirm-title" { "Mark this folder?" }
            p.confirm-body {
                "Writes a "
                span.confirm-chip id="confirm-file" { ".no_ebook" }
                " file to "
                strong id="confirm-folder" { "this folder" }
                ", covering this folder and everything beneath it."
            }
            label.confirm-again-label {
                input id="confirm-again" type="checkbox";
                "Don't ask again on this device"
            }
            div.confirm-actions {
                button.btn.btn-outline id="confirm-cancel" type="button" { "Cancel" }
                button.btn.btn-primary id="confirm-accept" type="button" {
                    span.confirm-icon data-confirm-icon=".no_ebook" { (PreEscaped(NO_ENTRY_SVG)) }
                    span.confirm-icon data-confirm-icon=".ebook_elsewhere" hidden { (PreEscaped(EBOOK_ELSEWHERE_SVG)) }
                    span id="confirm-accept-label" { "Confirm" }
                }
            }
        }
    }
}

/// The rescan placeholder. It overlays `#roots` while an in-place rescan is in
/// flight: htmx adds `htmx-request` to it for the duration (its bundled indicator
/// styles fade it in), and the shimmer reads as work in progress. Hidden
/// otherwise, and never shown on the no-JS path, which does a full reload.
fn scan_skeleton() -> Markup {
    html! {
        div.scan-skeleton.htmx-indicator id="scan-skeleton" aria-hidden="true" {
            @for _ in 0..5 {
                div.sk-row { span.sk.sk-icon {} span.sk.sk-name {} }
            }
        }
    }
}

/// The connection-status banner: a polite live region pinned to the top of the
/// page, hidden until app.js reveals it and sets a state class. The state copy is
/// carried as data attributes so it is defined (and tested) here in one place; the
/// client only chooses which message to show.
fn conn_banner() -> Markup {
    html! {
        div.conn-banner id="conn-banner" role="status" aria-live="polite" hidden
            data-msg-offline="You're offline. Changes can't be saved."
            data-msg-retrying="Lost connection. Retrying…"
            data-msg-failed="Couldn't reach the server. Your change wasn't saved."
            data-msg-failed-rescan="Couldn't reach the server. The library wasn't rescanned."
            data-msg-reconnected="Reconnected." {
            span.conn-banner-spinner aria-hidden="true" {}
            span.conn-banner-msg {}
        }
    }
}

/// The page-level toast shared by the undo and error flows, rendered once so it
/// survives the htmx section swaps. Hidden until `app.js` fills and shows it: the
/// success variant carries an Undo button, the error variant a message only.
/// Both marker glyphs are present and shown by variant in the stylesheet.
fn toast() -> Markup {
    html! {
        div.toast id="toast" role="status" aria-live="polite" hidden {
            span.toast-icon.toast-icon-success { (PreEscaped(CHECK_SVG)) }
            span.toast-icon.toast-icon-error { (PreEscaped(ERROR_SVG)) }
            span.toast-msg {}
            button.btn.btn-outline.btn-xs.toast-undo type="button" { "Undo" }
            button.toast-close type="button" aria-label="Dismiss" { "\u{00D7}" }
        }
    }
}

/// The rotating folder caret used on collapsible rows.
fn chevron() -> Markup {
    html! { (PreEscaped(CHEVRON_SVG)) }
}

/// The folder glyph used on every node row (every node is a folder).
fn folder_icon() -> Markup {
    html! { (PreEscaped(FOLDER_SVG)) }
}

/// The root sections in order. Shared by the full page and the htmx rescan
/// response, which swaps just this list into `#roots`.
fn roots(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        @for (root, section) in view.iter().enumerate() {
            (render_section(section, root, None, links, mode))
        }
    }
}

/// JSON-escape any non-ASCII char to `\uXXXX`, so an `HX-Trigger` header value is
/// pure ASCII and survives any browser header decoding. The input is valid JSON;
/// replacing a raw char with its escape keeps it valid JSON. Folder names are
/// often non-ASCII, so this is the common path, not an edge case.
fn ascii_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// Render a section response, optionally carrying an `HX-Trigger` header. A value
/// that will not encode (a control char in a folder name) is dropped rather than
/// failing the response: the swap still happens, only the toast is skipped.
fn section_response(markup: Markup, trigger: Option<String>) -> axum::response::Response {
    let mut resp = Html(markup.into_string()).into_response();
    if let Some(value) = trigger
        && let Ok(header) = HeaderValue::from_str(&value)
    {
        resp.headers_mut()
            .insert(HeaderName::from_static("hx-trigger"), header);
    }
    resp
}

/// The folder's display name for the toast: the last path segment, or the root
/// label (the section path's last component) when the target is the root itself.
fn display_name(section_path: &str, rel: &str) -> String {
    if rel == "." {
        std::path::Path::new(section_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(section_path)
            .to_string()
    } else {
        rel.rsplit('/').next().unwrap_or(rel).to_string()
    }
}

/// The `HX-Trigger` payload for a successful create: a `marked` event carrying
/// what the toast needs to describe and to undo the write.
fn marked_trigger(req: &MarkRequest, name: &str) -> String {
    let payload = serde_json::json!({
        "marked": {
            "root": req.root,
            "rel": req.rel,
            "kind": req.kind,
            "view": req.view.as_query(),
            "name": name,
        }
    });
    ascii_escape(&payload.to_string())
}

/// The `HX-Trigger` payload for a failed write: an `app-error` event carrying the
/// message for the error toast.
fn error_trigger(message: &str) -> String {
    let payload = serde_json::json!({ "app-error": { "message": message } });
    ascii_escape(&payload.to_string())
}

pub(crate) fn page(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Missing Ebooks" }
                link rel="icon" href=(FAVICON_HREF);
                script { (PreEscaped(PREPAINT_THEME_JS)) }
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                (conn_banner())
                nav.navbar {
                    h1 { "Missing Ebooks" }
                    span.spacer {}
                    (view_toggle(mode))
                    (settings_menu())
                    form method="post" action="/rescan"
                        hx-post="/rescan" hx-target="#roots" hx-swap="innerHTML"
                        hx-indicator="#scan-skeleton, #rescan-btn"
                        hx-disabled-elt="#rescan-btn" {
                        input type="hidden" name="view" value=(mode.as_query());
                        // The button sits in the nav, outside the swapped #roots, so htmx
                        // keeps it across the request: it adds htmx-request for the busy
                        // relabel and disables it so a second click cannot double-scan,
                        // clearing both once the swap settles.
                        button.btn.btn-primary id="rescan-btn" type="submit" {
                            span.btn-idle { "Rescan" }
                            span.btn-busy { "Rescanning…" }
                        }
                    }
                }
                div.roots-wrap {
                    main id="roots" {
                        (roots(view, links, mode))
                    }
                    (scan_skeleton())
                }
                (confirm_dialog())
                (toast())
                script src="/static/htmx.min.js" {}
                script src="/static/app.js" {}
            }
        }
    }
}

/// Count the gaps (`needs_ebook()` nodes) anywhere in a forest. Drives the root
/// summary badge so a collapsed root still tells you how much is unresolved.
fn count_gaps(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.needs_ebook()) + count_gaps(&n.children))
        .sum()
}

/// The badge shown on a root's summary: the gap count, a clean check, or a scan
/// error. In show-all the forest also holds covered nodes; only gaps are counted.
fn root_badge(state: &RootState) -> Markup {
    html! {
        @match state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                @if n == 0 {
                    span.root-badge.root-badge-clean { "\u{2713} no gaps" }
                } @else if n == 1 {
                    span.root-badge.root-badge-gaps { "1 gap" }
                } @else {
                    span.root-badge.root-badge-gaps { (n) " gaps" }
                }
            }
            RootState::Clean => {
                span.root-badge.root-badge-clean { "\u{2713} no gaps" }
            }
            RootState::Error(_) => {
                span.root-badge.root-badge-error { "scan error" }
            }
        }
    }
}

pub(crate) fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    let counter = std::cell::Cell::new(0usize);
    html! {
        section.card.root data-root=(root) {
            details.root-fold open {
                summary.root-head {
                    (chevron())
                    h2 { (section.path) }
                    span.spring {}
                    (root_badge(&section.state))
                }
                @if let Some(message) = error {
                    div.alert.alert-error { (PreEscaped(ERROR_SVG)) span { (message) } }
                }
                @match &section.state {
                    RootState::Forest(nodes) => {
                        @if nodes.is_empty() {
                            // Show-all yields an empty forest only for a root with no
                            // folders at all. Gaps-only sets Clean instead.
                            div.empty { span { "Nothing here" } }
                        } @else {
                            ul.menu {
                                @for node in nodes { (render_node(node, root, links, mode, &counter)) }
                            }
                        }
                    }
                    RootState::Clean => {
                        div.empty { (PreEscaped(CHECK_SVG)) span { "No missing ebooks in this root" } }
                    }
                    RootState::Error(message) => {
                        div.alert.alert-error {
                            (PreEscaped(ERROR_SVG)) span { "Could not scan this root: " (message) }
                        }
                    }
                }
            }
        }
    }
}

/// The show-all status marker for a row: a success check on a covered folder. Gaps
/// are already flagged by the amber icon and the badge, and plain containers need
/// no marker, so neither gets one. Rendered only in show-all mode.
fn status_icon(node: &Node) -> Markup {
    html! {
        @if !node.missing_ebook {
            span.status title="covered" { (PreEscaped(CHECK_SVG)) }
        }
    }
}

/// The covering ebook and marker filenames for a covered row, in muted text just
/// after the status check. Show-all only; empty for gaps and for folders covered
/// from above, so nothing renders there.
fn cover_files_span(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if mode == ViewMode::All && !node.cover_files.is_empty() {
            span.cover-files title="covering files" { (node.cover_files.join(", ")) }
        }
    }
}

fn render_node(
    node: &Node,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    // A covered row dims only in show-all; gaps-only never holds covered nodes.
    let covered = mode == ViewMode::All && !node.missing_ebook;
    // Buttons and links appear only where there is a gap to act on. In gaps-only
    // every node qualifies, so the output is unchanged.
    let act = node.has_gap_within();
    html! {
        @if node.children.is_empty() {
            li {
                div.row.flagged[node.needs_ebook()].covered[covered] {
                    span.leaf-pad {}
                    (folder_icon())
                    span.name { (node.name) }
                    @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                    @if mode == ViewMode::All { (status_icon(node)) }
                    (cover_files_span(node, mode))
                    span.spring {}
                    @if act {
                        (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                    }
                }
            }
        } @else {
            li {
                details open {
                    summary.row.flagged[node.needs_ebook()].covered[covered] {
                        (chevron())
                        (folder_icon())
                        span.name { (node.name) }
                        @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                        @if act {
                            (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                        }
                    }
                    ul {
                        @for child in &node.children { (render_node(child, root, links, mode, counter)) }
                    }
                }
            }
        }
    }
}

/// The per-row action cluster: a kebab trigger plus the marker buttons and the
/// search links, wrapped in a group that doubles as a native popover. On desktop
/// the trigger is hidden and the group is `display: contents`, so its children
/// flow inline in the row as before; on mobile the kebab opens the group as a
/// bottom action sheet over a dimmed backdrop. The browser provides the toggle,
/// one-open-at-a-time, light-dismiss, and Esc.
fn row_actions(
    root: usize,
    rel: &str,
    name: &str,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    let group_id = next_id("acts", root, counter);
    html! {
        button.actions-trigger type="button"
            aria-label="Actions"
            aria-haspopup="menu"
            popovertarget=(group_id)
            onclick="event.stopPropagation()" { (PreEscaped(KEBAB_SVG)) }
        div.actions-group id=(group_id) popover="auto" aria-label=(name) {
            div.sheet-header {
                span.sheet-grip aria-hidden="true" {}
                span.sheet-title { (name) }
            }
            (marker_buttons(root, rel, name, mode))
            (search_links(links, name, root, counter))
        }
    }
}

fn marker_buttons(root: usize, rel: &str, name: &str, mode: ViewMode) -> Markup {
    // In gaps-only the marked folder leaves the list, so app.js collapses its row and
    // the section swap waits for that to play. In show-all the row stays and flips to
    // covered in place, so the swap is immediate. The delay matches the CSS transition.
    let swap = match mode {
        ViewMode::GapsOnly => "outerHTML swap:250ms",
        ViewMode::All => "outerHTML",
    };
    html! {
        form.mark.actions hx-target="closest section.root" hx-swap=(swap) {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            input type="hidden" name="view" value=(mode.as_query());
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                data-confirm-action="Mark as None"
                data-confirm-file=".no_ebook"
                data-confirm-folder=(name)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(NO_ENTRY_SVG)) }
                    span.label-long { "Mark as None" }
                    span.label-short { "None" }
                }
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                data-confirm-action="Ebook elsewhere"
                data-confirm-file=".ebook_elsewhere"
                data-confirm-folder=(name)
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(EBOOK_ELSEWHERE_SVG)) }
                    span.label-long { "Ebook elsewhere" }
                    span.label-short { "Elsewhere" }
                }
        }
    }
}

/// Next DOM-safe id with the given prefix for a row's popover or action group.
/// The relative path can hold slashes and spaces, so a root index plus a
/// per-render counter is used instead. The counter is reset per
/// `render_section`, so ids are unique within one render.
fn next_id(prefix: &str, root: usize, counter: &std::cell::Cell<usize>) -> String {
    let n = counter.get();
    counter.set(n + 1);
    format!("{prefix}-{root}-{n}")
}

fn search_links(
    links: &[SearchLink],
    name: &str,
    root: usize,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            @let id = next_id("links", root, counter);
            span.links {
                span.sheet-divider { "Search" }
                button.btn.btn-outline.btn-xs.links-toggle type="button"
                    popovertarget=(id)
                    aria-label="Search for this book"
                    title="Search links"
                    onclick="event.stopPropagation()" { (PreEscaped(SEARCH_SVG)) }
                div.links-menu popover="auto" id=(id) onclick="event.stopPropagation()" {
                    @for link in links {
                        a href=(link.url.replace("{query}", &query))
                            target="_blank" rel="noopener noreferrer" {
                                span.sheet-icon { (PreEscaped(SEARCH_SVG)) }
                                (link.label)
                            }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::config::Config;
    use crate::scanner::ScanSettings;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    fn app_for(root: &Path) -> Router {
        app_for_with_links(root, Config::default().search_links)
    }

    fn app_for_with_links(root: &Path, search_links: Vec<SearchLink>) -> Router {
        let cfg = Config {
            library_roots: vec![root.to_path_buf()],
            ttl_seconds: 60,
            search_links,
            ..Default::default()
        };
        let settings = ScanSettings::compile(cfg.scan_inputs()).unwrap();
        router(Arc::new(AppState::new(cfg, settings)))
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn index_renders_a_flagged_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("Book"));
    }

    #[tokio::test]
    async fn index_wraps_the_sections_in_a_roots_container() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The root sections live inside a positioned wrapper so the rescan skeleton
        // can overlay them, and inside #roots so htmx can swap them in place.
        assert!(body.contains(r#"class="roots-wrap""#));
        assert!(body.contains(r#"id="roots""#));
        // The sections themselves are unchanged.
        assert!(body.contains(r#"class="card root""#));
    }

    #[tokio::test]
    async fn marker_form_delays_the_swap_only_in_gaps_only() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        // Gaps-only: the marked folder leaves the list, so the section swap is delayed
        // to let app.js play the row's collapse before the fresh section lands.
        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(gaps.contains(r#"hx-swap="outerHTML swap:250ms""#));
        // Show-all: the row stays and flips to covered in place, so there is nothing to
        // collapse and the swap is immediate.
        let all = body_string(
            app.oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(all.contains(r#"hx-swap="outerHTML""#));
        assert!(!all.contains("swap:250ms"));
    }

    #[tokio::test]
    async fn app_script_collapses_the_leaving_row() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Before a gaps-only mark request goes out, the script collapses the leaving
        // row so the rows below glide up; the delayed htmx swap reconciles after.
        assert!(body.contains("htmx:beforeRequest"));
        assert!(body.contains("leaving"));
    }

    #[tokio::test]
    async fn app_script_blurs_the_mark_button_before_the_swap() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The section swap removes the focused mark button. Left focused, the browser
        // jumps the scroll to the page bottom (true in both views), so the script
        // drops focus before the swap.
        assert!(body.contains("blur"));
    }

    #[tokio::test]
    async fn stylesheet_collapses_the_leaving_row_and_respects_reduced_motion() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // A leaving row collapses its height and fades; motion-sensitive users get the
        // instant removal instead.
        assert!(body.contains(".leaving"));
        assert!(body.contains("max-height"));
        assert!(body.contains("prefers-reduced-motion"));
    }

    #[tokio::test]
    async fn rescan_is_an_in_place_htmx_swap_with_a_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // Rescan posts via htmx and swaps the fresh sections into #roots.
        assert!(body.contains(r#"hx-post="/rescan""#));
        assert!(body.contains(r##"hx-target="#roots""##));
        // The skeleton and the button both light up as indicators, and the button
        // is disabled for the request so a second click cannot fire a second scan.
        assert!(body.contains(r##"hx-indicator="#scan-skeleton, #rescan-btn""##));
        assert!(body.contains(r##"hx-disabled-elt="#rescan-btn""##));
        // The skeleton overlay is present and wired as the htmx indicator.
        assert!(body.contains(r#"id="scan-skeleton""#));
        assert!(body.contains("htmx-indicator"));
        // The button carries both labels and relabels itself while the scan runs.
        assert!(body.contains(r#"id="rescan-btn""#));
        assert!(body.contains("Rescanning"));
        // The plain form action survives for the no-JS path.
        assert!(body.contains(r#"action="/rescan""#));
    }

    #[tokio::test]
    async fn rescan_returns_sections_for_an_htmx_request() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("HX-Request", "true")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // No redirect: the fresh sections come back to swap into #roots.
        assert_eq!(response.status(), StatusCode::OK);
        // The address bar is pushed to the requested view, not the POST URL.
        assert_eq!(response.headers().get("HX-Push-Url").unwrap(), "/?view=all");
        let body = body_string(response).await;
        assert!(body.contains(r#"class="card root""#));
        assert!(body.contains("Book"));
    }

    #[tokio::test]
    async fn stylesheet_carries_the_scan_skeleton_shimmer() {
        let dir = tempfile::tempdir().unwrap();
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/static/app.css")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The rescan placeholder is a positioned overlay with a shimmer animation.
        assert!(body.contains(".scan-skeleton"));
        assert!(body.contains("@keyframes shimmer"));
        // The wrapper is positioned so the overlay can pin to it.
        assert!(body.contains(".roots-wrap"));
        // The rescan button carries its in-flight relabel styles.
        assert!(body.contains("#rescan-btn.htmx-request .btn-busy"));
    }

    #[tokio::test]
    async fn index_links_an_inline_favicon() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // An inline SVG data-URI favicon, so the browser stops requesting
        // /favicon.ico and the tab gets an identity.
        assert!(body.contains(r#"rel="icon""#));
        assert!(body.contains("data:image/svg+xml,"));
        // A backdrop-less audiobook glyph that recolors with the OS theme: indigo
        // on light tab strips, a lighter indigo on dark ones. Pin the indigo and
        // the prefers-color-scheme rule so a revert to the old book glyph or to a
        // single static color is caught, and assert the rounded tile is gone (its
        // rect was 22x22).
        assert!(body.contains("%23605dff"));
        assert!(body.contains("prefers-color-scheme:dark"));
        assert!(!body.contains("width='22'"));
    }

    #[tokio::test]
    async fn index_shows_the_clean_message_for_a_covered_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("No missing ebooks in this root"));
    }

    #[tokio::test]
    async fn index_renders_the_marker_buttons_and_script() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains(r#"src="/static/htmx.min.js""#));
        assert!(body.contains(r#"src="/static/app.js""#));
        assert!(body.contains(">None<"));
    }

    #[tokio::test]
    async fn elsewhere_button_uses_the_book_check_icon() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The "Ebook elsewhere" button now carries a book-and-check glyph (the
        // checkmark path), not the old open-external-link arrow.
        assert!(body.contains("m9 9.5 2 2 4-4"));
        assert!(!body.contains("M10 14L21 3"));
    }

    #[tokio::test]
    async fn index_renders_the_search_links() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book (Unabridged)/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Goodreads ships as a default link. The (Unabridged) suffix is stripped from
        // the query, so the href ends in `q=Book`, and the links open in a new tab.
        assert!(body.contains(r#"target="_blank""#));
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains("Goodreads"));
    }

    #[tokio::test]
    async fn index_renders_every_configured_link() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        // The defaults ship two links; both must render, not just the first.
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains("https://oceanofpdf.com/?s=Book"));
        assert!(body.contains("OceanofPDF"));
    }

    #[tokio::test]
    async fn index_omits_the_links_span_when_none_are_configured() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for_with_links(dir.path(), Vec::new())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // No links means no `span.links` is emitted, and no search popover menu.
        // The kebab still carries `popovertarget`; it is the sheet trigger now.
        assert!(!body.contains(r#"class="links""#));
        assert!(!body.contains(r#"class="links-menu""#));
        assert!(!body.contains(r#"title="Search links""#));
    }

    #[tokio::test]
    async fn search_links_render_inside_a_popover_menu() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A magnifying-glass button opens a popover that holds the links.
        assert!(body.contains("popovertarget"));
        assert!(body.contains(r#"class="links-menu""#));
        // The link itself is unchanged, just relocated into the menu.
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
        assert!(body.contains(r#"target="_blank""#));
    }

    #[tokio::test]
    async fn search_link_query_percent_encodes_spaces() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author Name/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Spaces in the cleaned query are percent-encoded, so the href carries `%20`.
        assert!(body.contains("q=Author%20Name"));
    }

    #[tokio::test]
    async fn index_renders_the_menu_with_a_flagged_badge() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The tree is now a `menu`, and the styled section keeps the `root` hook.
        assert!(body.contains(r#"class="menu""#));
        assert!(body.contains(r#"class="card root""#));
        // A flagged folder carries the warning badge.
        assert!(body.contains("needs ebook"));
    }

    #[tokio::test]
    async fn index_links_the_stylesheet_and_inits_the_theme() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The external stylesheet replaces the old inline <style> block.
        assert!(body.contains(r#"href="/static/app.css""#));
        // The pre-paint theme script is present and reads the OS preference.
        assert!(body.contains("prefers-color-scheme"));
        // The theme toggle moved into the settings menu: a labelled cog, with the
        // theme choices inside the panel.
        assert!(body.contains(r#"aria-label="Settings""#));
        assert!(body.contains(r#"data-theme-choice="system""#));
    }

    #[tokio::test]
    async fn static_route_serves_the_stylesheet() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type.to_str().unwrap().contains("text/css"));
        let body = body_string(response).await;
        assert!(body.contains("--color-base-100"));
    }

    #[tokio::test]
    async fn stylesheet_carries_the_mobile_layout_rules() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("@media (max-width: 600px)"));
        assert!(body.contains(".actions-trigger"));
        // The action group is now a bottom sheet driven by the popover API, not
        // the old `data-actions-open` toggle.
        assert!(body.contains(":popover-open"));
        assert!(body.contains("::backdrop"));
        assert!(!body.contains("data-actions-open"));
    }

    #[tokio::test]
    async fn stylesheet_lays_out_marker_tiles_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The two marker buttons share a row as equal-width tiles.
        assert!(body.contains(".actions-group .mark .btn"));
        assert!(body.contains("flex-direction: row"));
    }

    #[tokio::test]
    async fn stylesheet_left_aligns_the_sheet_search_links() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The links column stretches to full width instead of centering, so the
        // search links share the marker buttons' left edge.
        assert!(body.contains("align-items: stretch"));
    }

    #[tokio::test]
    async fn stylesheet_collapses_the_flagged_badge_and_keeps_rows_on_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // On mobile the "needs ebook" pill collapses to an amber dot. The label is
        // pushed out of the box with the image-replacement idiom (text-indent), not
        // removed, so it stays in the HTML for screen readers.
        assert!(body.contains("text-indent: 100%"));
        // Non-covered rows stop wrapping, so the dot and kebab stay on the first
        // line and a long name wraps inside its own box instead.
        assert!(body.contains(".row:not(.covered)"));
        assert!(body.contains("overflow-wrap: anywhere"));
    }

    #[tokio::test]
    async fn stylesheet_stacks_the_navbar_view_toggle_into_a_full_width_row() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The segmented view toggle drops to its own row at full width, with the
        // two segments sharing it as equal-width halves. A child combinator scopes
        // the rule to the navbar's own control, so the settings panel's nested
        // theme segmented control can't inherit the full-width row layout.
        assert!(body.contains(".navbar > .segmented"));
        assert!(body.contains("flex-basis: 100%"));
        assert!(body.contains(".navbar > .segmented .segment"));
        // The settings cog is ordered by a dedicated class, not a `> button`
        // child selector, so a later navbar button can't drift into its row.
        assert!(body.contains(".navbar .settings-cog"));
        assert!(!body.contains(".navbar > button"));
    }

    #[tokio::test]
    async fn stylesheet_indents_the_mobile_cover_files_past_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // Covering filenames drop below the folder name and indent past where the
        // name starts, so they read as subordinate rather than lining up flush.
        assert!(body.contains(".cover-files"));
        assert!(body.contains("padding-left: 3.5rem"));
    }

    #[tokio::test]
    async fn the_flagged_badge_carries_a_hover_title() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The mobile dot has no visible text, so the badge gets a title that names
        // it on hover. The literal label is still emitted as the badge's content.
        assert!(body.contains(r#"title="needs ebook""#));
        assert!(body.contains("needs ebook"));
    }

    #[tokio::test]
    async fn static_route_serves_the_htmx_script() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/htmx.min.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type.to_str().unwrap().contains("javascript"));
    }

    #[tokio::test]
    async fn static_route_serves_the_app_script() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type.to_str().unwrap().contains("javascript"));
    }

    #[tokio::test]
    async fn app_script_defines_the_theme_setter() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("setTheme"));
        assert!(body.contains("confirmMarks"));
    }

    #[tokio::test]
    async fn navbar_renders_a_settings_cog_with_theme_and_confirm_controls() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A labelled cog opens the settings panel via the native popover API.
        assert!(body.contains(r#"class="btn btn-ghost btn-square settings-cog""#));
        assert!(body.contains(r#"aria-label="Settings""#));
        assert!(body.contains(r#"popovertarget="settings-panel""#));
        assert!(body.contains(r#"id="settings-panel""#));
        // The theme segmented control offers all three states.
        assert!(body.contains(r#"data-theme-choice="light""#));
        assert!(body.contains(r#"data-theme-choice="dark""#));
        assert!(body.contains(r#"data-theme-choice="system""#));
        // The confirm-before-marking switch.
        assert!(body.contains(r#"id="confirm-toggle""#));
        // The old two-state toggle is gone.
        assert!(!body.contains("toggleTheme()"));
    }

    #[tokio::test]
    async fn index_renders_the_hidden_connection_banner() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A polite live region, hidden until the connection JS reveals it. The
        // `hidden` check is anchored to the banner's own attributes so it can't pass
        // on an unrelated `aria-hidden` elsewhere on the page.
        assert!(body.contains(r#"id="conn-banner""#));
        assert!(body.contains(r#"role="status""#));
        assert!(body.contains(r#"role="status" aria-live="polite" hidden"#));
        // Copy lives in data attributes so it is locked here and read by app.js.
        assert!(body.contains(r#"data-msg-offline="You're offline. Changes can't be saved.""#));
        assert!(body.contains(r#"data-msg-retrying="Lost connection. Retrying…""#));
        assert!(
            body.contains(
                r#"data-msg-failed="Couldn't reach the server. Your change wasn't saved.""#
            )
        );
        assert!(body.contains(
            r#"data-msg-failed-rescan="Couldn't reach the server. The library wasn't rescanned.""#
        ));
        assert!(body.contains(r#"data-msg-reconnected="Reconnected.""#));
        // The message slot the JS fills.
        assert!(body.contains(r#"class="conn-banner-msg""#));
    }

    #[tokio::test]
    async fn index_renders_the_confirm_dialog() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A single page-level dialog the confirm flow fills and opens.
        assert!(body.contains(r#"id="confirm-mark""#));
        assert!(body.contains("Don't ask again"));
        assert!(body.contains(r#"id="confirm-accept""#));
        assert!(body.contains(r#"id="confirm-cancel""#));
    }

    #[tokio::test]
    async fn index_renders_the_toast_element() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A single page-level toast the success and error flows share.
        assert!(body.contains(r#"id="toast""#));
        // The Undo button carries the toast-undo class alongside its btn classes.
        assert!(body.contains("toast-undo"));
        assert!(body.contains(r#"class="toast-msg""#));
    }

    #[tokio::test]
    async fn marker_buttons_carry_confirm_metadata() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // Each marker button names its action, file, and folder for the dialog.
        assert!(body.contains(r#"data-confirm-action="Mark as None""#));
        assert!(body.contains(r#"data-confirm-file=".no_ebook""#));
        assert!(body.contains(r#"data-confirm-action="Ebook elsewhere""#));
        assert!(body.contains(r#"data-confirm-file=".ebook_elsewhere""#));
        assert!(body.contains(r#"data-confirm-folder="Book""#));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_confirm_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The dialog is themed and dims the page behind it.
        assert!(body.contains(".confirm-dialog"));
        assert!(body.contains(".confirm-dialog::backdrop"));
        // The non-matching marker glyph hides via the `hidden` attribute. The
        // explicit `.confirm-icon` display must honor it, or both glyphs show.
        assert!(body.contains(".confirm-icon[hidden]"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_toast_and_its_variants() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains(".toast"));
        // Each variant reveals its own glyph; the others stay hidden.
        assert!(body.contains(".toast--success .toast-icon-success"));
        assert!(body.contains(".toast--error .toast-icon-error"));
    }

    #[tokio::test]
    async fn stylesheet_styles_the_settings_panel_and_switch() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The settings popover, the switch, and the mobile bottom-sheet form.
        assert!(body.contains(".settings-panel"));
        assert!(body.contains(".switch-track"));
        assert!(body.contains(".settings-panel:popover-open"));
        // The mobile sheet resets the desktop anchor positioning; without this the
        // @supports position-area rule keeps the panel pinned under the cog at
        // partial width instead of spanning the full bottom of the viewport.
        assert!(body.contains("position-area: none"));
    }

    #[tokio::test]
    async fn stylesheet_neutralizes_native_button_chrome_on_segments() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The theme control renders its segments as <button>, the view toggle as
        // <span>/<a>. Without an appearance reset the buttons inherit the user
        // agent's native control chrome (grey fills, beveled borders) while the
        // view toggle stays flat, so the two diverge. The .segment rule must drop
        // that chrome so both render identically.
        assert!(body.contains(".segment"));
        assert!(body.contains("appearance: none"));
    }

    #[tokio::test]
    async fn app_script_intercepts_marker_writes() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("htmx:confirm"));
    }

    #[tokio::test]
    async fn app_script_defines_the_toast_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/static/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        // The success and error listeners and the undo POST to /unmark.
        assert!(body.contains(r#"addEventListener("marked""#));
        assert!(body.contains(r#"addEventListener("app-error""#));
        assert!(body.contains("/unmark"));
    }

    #[tokio::test]
    async fn rescan_redirects_to_root() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/");
    }

    #[tokio::test]
    async fn mark_writes_the_file_and_swaps_the_section() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the cache so the mark exercises the in-place update.
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("HX-Request", "true")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("No missing ebooks in this root"));
        assert!(dir.path().join("Book/.no_ebook").exists());
    }

    #[tokio::test]
    async fn mark_sets_the_marked_trigger_on_a_create_and_omits_it_on_a_remark() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());

        // First mark creates the file: the response carries the marked trigger.
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let trigger = first
            .headers()
            .get("hx-trigger")
            .map(|v| v.to_str().unwrap().to_string());
        let trigger = trigger.expect("a create sets HX-Trigger");
        assert!(trigger.contains("marked"));
        assert!(trigger.contains("\"name\":\"Book\""));
        assert!(trigger.contains("\"kind\":\"no_ebook\""));

        // Second mark of the same folder is a no-op create: no marked trigger.
        let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(second.headers().get("hx-trigger").is_none());
    }

    #[tokio::test]
    async fn all_view_renders_covered_folders_that_gaps_only_drops() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let app = app_for(dir.path());

        // Gaps-only drops the covered book.
        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!gaps.contains("Covered"));

        // Show-all renders it.
        let all = body_string(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(all.contains("Covered"));
        assert!(all.contains("Gap"));
    }

    #[tokio::test]
    async fn mark_in_all_mode_keeps_the_covered_folder_visible() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the all slot.
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Author/Book&kind=no_ebook&view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        // The book stays in the tree (covered), not removed.
        assert!(body.contains("Book"));
        assert!(dir.path().join("Author/Book/.no_ebook").exists());
    }

    #[tokio::test]
    async fn rescan_preserves_the_current_view() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rescan")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/?view=all");
    }

    #[tokio::test]
    async fn section_carries_a_data_root_hook() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The toast's Undo targets the section by index, so each section names it.
        assert!(body.contains(r#"data-root="0""#));
    }

    #[tokio::test]
    async fn mark_failure_triggers_an_error_toast_and_keeps_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=..&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let trigger = response
            .headers()
            .get("hx-trigger")
            .map(|v| v.to_str().unwrap().to_string())
            .expect("a failed mark sets HX-Trigger");
        assert!(trigger.contains("app-error"));
        assert!(trigger.contains("Could not mark"));
        let body = body_string(response).await;
        // The tree is intact and carries no inline alert.
        assert!(body.contains("Book"));
        assert!(!body.contains("alert-error"));
    }

    #[tokio::test]
    async fn unmark_route_deletes_the_file_and_swaps_the_section_back() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let app = app_for(dir.path());
        app.clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        // Mark first so there is a file to remove.
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(dir.path().join("Book/.no_ebook").exists());

        // Undo: the file is gone and the swapped section shows the gap again.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/unmark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Book&kind=no_ebook&view=gaps"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(!dir.path().join("Book/.no_ebook").exists());
        assert!(body.contains("Book"));
        assert!(body.contains("needs ebook"));
    }

    #[tokio::test]
    async fn all_view_dims_covered_rows_and_omits_their_buttons() {
        let dir = tempfile::tempdir().unwrap();
        // A covered container (series epub) whose books are all covered.
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Covered rows carry the success check and the covered class.
        assert!(body.contains(r#"title="covered""#));
        assert!(body.contains(r#"covered""#));
        // A fully covered branch carries no marker buttons.
        assert!(!body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn all_view_keeps_buttons_on_a_container_above_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // The author is a plain container above a gap, so it still gets buttons.
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains("Gap"));
    }

    #[tokio::test]
    async fn the_view_control_marks_the_active_segment() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());

        let gaps = body_string(
            app.clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // Gaps-only is the active view; "All folders" is the link to the other view.
        assert!(gaps.contains(r#"class="segmented""#));
        assert!(gaps.contains("Gaps only"));
        assert!(gaps.contains("All folders"));
        assert!(gaps.contains(r#"href="/?view=all""#));

        let all = body_string(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // Show-all is active; "Gaps only" links back to /.
        assert!(all.contains(r#"href="/""#));
        assert!(all.contains(r#"aria-current="page""#));
    }

    #[tokio::test]
    async fn each_root_renders_a_collapsible_summary_with_a_gap_count() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // The root head is now a <summary> inside a collapsible <details>.
        assert!(body.contains(r#"class="root-fold""#));
        assert!(body.contains("root-head"));
        // One gap under this root, so the badge reads "1 gap".
        assert!(body.contains("1 gap"));
    }

    #[tokio::test]
    async fn a_clean_root_badge_reads_no_gaps() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        touch(&dir.path().join("Book/Book.epub"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("no gaps"));
    }

    #[tokio::test]
    async fn all_view_shows_nothing_here_for_a_root_with_no_folders() {
        let dir = tempfile::tempdir().unwrap();
        // An empty root: no subfolders, no audio.
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("Nothing here"));
    }

    #[tokio::test]
    async fn all_view_lists_the_covering_ebook_on_a_covered_row() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains(r#"class="cover-files""#));
        assert!(body.contains("Covered.epub"));
    }

    #[tokio::test]
    async fn gaps_only_view_lists_no_cover_files() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Gap/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert!(!body.contains(r#"class="cover-files""#));
    }

    #[tokio::test]
    async fn marking_in_all_mode_shows_the_written_marker_on_the_row() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let app = app_for(dir.path());
        // Warm the all slot.
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/?view=all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mark")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("root=0&rel=Author/Book&kind=no_ebook&view=all"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains(r#"class="cover-files""#));
        assert!(body.contains(".no_ebook"));
    }

    #[tokio::test]
    async fn gaps_only_view_has_no_status_icons_or_covered_rows() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // No status markers and no covered rows in the gaps-only output.
        assert!(!body.contains(r#"class="status""#));
        assert!(!body.contains(r#"covered""#));
        // The gap and its buttons are still there.
        assert!(body.contains("Book"));
        assert!(body.contains(r#"hx-post="/mark""#));
    }

    #[tokio::test]
    async fn each_actionable_row_has_an_actions_trigger() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // A labelled kebab that opens the per-row action sheet via the native
        // popover API, and the group that is that popover.
        assert!(body.contains(r#"class="actions-trigger""#));
        assert!(body.contains(r#"aria-label="Actions""#));
        assert!(body.contains(r#"aria-haspopup="menu""#));
        assert!(body.contains("popovertarget"));
        assert!(body.contains(r#"class="actions-group""#));
        assert!(body.contains(r#"popover="auto""#));
        // The group is labelled with the folder name and titles the sheet with it.
        assert!(body.contains(r#"aria-label="Book""#));
        assert!(body.contains(r#"class="sheet-title""#));
        // The marker buttons and search links still render inside the group.
        assert!(body.contains(r#"hx-post="/mark""#));
        assert!(body.contains(">None<"));
        assert!(body.contains("Goodreads"));
        // The bespoke toggle is gone; the browser drives open/close now.
        assert!(!body.contains("toggleRowActions"));
        assert!(!body.contains("aria-expanded"));
    }

    #[tokio::test]
    async fn the_action_sheet_titles_with_the_folder_and_shows_verbose_labels() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let response = app_for(dir.path())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(response).await;
        // The sheet header titles the sheet with the folder name.
        assert!(body.contains(r#"class="sheet-title">Book<"#));
        // The verbose, sheet-only marker labels sit alongside the compact ones.
        assert!(body.contains("Mark as None"));
        assert!(body.contains("Ebook elsewhere"));
        // The compact labels keep the exact text the marker write asserts on.
        assert!(body.contains(">None<"));
        assert!(body.contains(">Elsewhere<"));
    }

    #[tokio::test]
    async fn the_action_sheet_marks_the_search_section() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        // A sheet-only "Search" divider separates the marker rows from the links.
        assert!(body.contains(r#"class="sheet-divider""#));
        // The links still resolve to their configured search URLs.
        assert!(body.contains("https://www.goodreads.com/search?q=Book"));
    }

    #[tokio::test]
    async fn a_covered_row_has_no_actions_trigger() {
        let dir = tempfile::tempdir().unwrap();
        // A fully covered branch: the book has its own ebook, nothing to act on.
        touch(&dir.path().join("Series/Series.epub"));
        touch(&dir.path().join("Series/Book/01.mp3"));
        let body = body_string(
            app_for(dir.path())
                .oneshot(
                    Request::builder()
                        .uri("/?view=all")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // No gap under this branch, so no trigger and no group are emitted.
        assert!(!body.contains(r#"class="actions-trigger""#));
        assert!(!body.contains(r#"class="actions-group""#));
    }
}
