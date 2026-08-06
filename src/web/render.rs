//! All Maud markup: the page shell, the per-root section, the per-node rows, and
//! the SVG/JS constants those use. Kept separate from the router so the markup is
//! the test surface and `web.rs` stays handlers and glue.

use maud::{Markup, PreEscaped, html};

use crate::config::SearchLink;
use crate::query::{clean_query, percent_encode};
use crate::raw_view::RawView;
use crate::scanner::RootScan;
use crate::tree;
use crate::tree::Node;
use crate::tree::{RootState, ViewMode};

/// The whole read view: one section per configured library root, in config order.
type FlaggedView = Vec<RootSection>;

/// One library root's outcome, labeled with the path the scanner walked.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RootSection {
    /// The canonical root path when it resolved, else the configured path.
    path: String,
    /// What the scan found for this root.
    state: RootState,
    /// Folders under this root that directly hold audio. Zero for `Clean` and
    /// `Error`. The web layer surfaces it as `data-total-audiobooks` on the
    /// section so the strip's library coverage stays current across swaps.
    total_audiobooks: usize,
    /// Total gaps across this root's forest, precomputed by `build_forest` so
    /// the summary strip and per-root chip read a number instead of walking
    /// the tree. `Clean` and `Error` are zero by construction.
    gaps_within: usize,
    /// Directories the walk could not read for this root. Nonzero renders
    /// the "couldn't be read" partial-scan warning strip.
    skipped_dirs: usize,
    /// Subtree roots this root's walk pruned via the depth cap. Nonzero
    /// renders a separate "depth limit" warning strip, since the cause
    /// (a hardcoded ceiling, not a read failure) and remediation differ
    /// from `skipped_dirs`.
    depth_capped_dirs: usize,
}

/// Build the per-mode `FlaggedView` from the cached raw scan output. The gaps
/// path filters with `reduce_to_flagged` and builds the forest. Show-all builds
/// directly from the raw folders. Both run on the request thread (the per-folder
/// cost is bounded, see ADR-0022). Allocates a fresh `FlaggedView` per response
/// and drops it after the response writes.
fn package_view(raw: &RawView, mode: ViewMode) -> FlaggedView {
    raw.iter().map(|scan| package_section(scan, mode)).collect()
}

/// Build one `RootSection` from a raw `RootScan` for the requested mode.
///
/// The single owner of the raw-to-packaged step. `package_view` calls it on
/// the snapshot path; `packaged_section` calls it to build a `SectionHandle`
/// for every mark/unmark response. Any future per-root field lands here
/// once.
fn package_section(scan: &RootScan, mode: ViewMode) -> RootSection {
    let state = tree::build(scan, mode);
    let gaps_within = match &state {
        RootState::Forest(nodes) => nodes.iter().map(|n| n.gaps_within).sum(),
        RootState::Clean | RootState::Error(_) => 0,
    };
    RootSection {
        path: scan.display_path().to_string(),
        state,
        total_audiobooks: scan.audiobook_count(),
        gaps_within,
        skipped_dirs: match scan {
            RootScan::Walked { skipped_dirs, .. } => *skipped_dirs,
            RootScan::Failed { .. } => 0,
        },
        depth_capped_dirs: match scan {
            RootScan::Walked {
                depth_capped_dirs, ..
            } => *depth_capped_dirs,
            RootScan::Failed { .. } => 0,
        },
    }
}

/// One packaged section plus the identifying context needed to render it.
/// Constructed by `packaged_section`, which owns the raw → packaged step.
/// The handle owns its `RootSection` so callers do not name intermediate
/// types.
pub struct SectionHandle {
    section: RootSection,
    root: usize,
    mode: ViewMode,
}

impl SectionHandle {
    /// Render the section for an inline swap. `alert` shows as an
    /// in-section error banner when `Some`.
    #[must_use]
    pub fn render(&self, links: &[SearchLink], alert: Option<&str>) -> Markup {
        render_section(&self.section, self.root, alert, links, self.mode)
    }
}

/// Package one root's section from `raw`, ready to render. Panics if
/// `root >= raw.len()`; callers validate the index before reaching this
/// seam (`WriteFailure::BadRoot` in `web::mark`/`unmark`, an explicit
/// bounds check in `demo::apply_mark`).
#[must_use]
pub fn packaged_section(raw: &RawView, root: usize, mode: ViewMode) -> SectionHandle {
    let section = package_section(&raw[root], mode);
    SectionHandle {
        section,
        root,
        mode,
    }
}

/// Full HTML document for the current raw view. Assembles the packaged
/// view, then hands it to `render_view` for the shell + gap summary +
/// roots block. Single call site: the index handler (prod and demo).
///
/// `poll_interval_seconds` threads down to the page shell, where it drives
/// the client-side poll cadence embedded in the `#poll-root` marker. See
/// ADR-0034.
#[must_use]
pub fn page(
    raw: &RawView,
    links: &[SearchLink],
    mode: ViewMode,
    poll_interval_seconds: u64,
) -> Markup {
    let view = package_view(raw, mode);
    render_view(&view, links, mode, poll_interval_seconds)
}

/// Every root section as one payload for the `#roots` `innerHTML` swap.
/// Shared by the `/rescan` and `/refresh` handlers.
#[must_use]
pub fn all_sections(raw: &RawView, links: &[SearchLink], mode: ViewMode) -> Markup {
    let view = package_view(raw, mode);
    roots(&view, links, mode)
}

/// The rotating folder caret used on collapsible rows.
fn chevron() -> Markup {
    html! { (PreEscaped(include_str!("../../assets/svg/chevron.svg"))) }
}

/// The folder glyph used on every node row (every node is a folder).
fn folder_icon() -> Markup {
    html! { (PreEscaped(include_str!("../../assets/svg/folder.svg"))) }
}

/// The root sections in order. Shared by the full page and the htmx swap
/// responses (`/rescan`, `/refresh`), which target `#roots` with an
/// `innerHTML` swap.
fn roots(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        @for (root, section) in view.iter().enumerate() {
            (render_section(section, root, None, links, mode))
        }
    }
}

/// The page entry point: assembles the body content (gap summary + roots
/// block) and hands it to `page::page` for shell-wrapping. The single
/// production caller is `web::index` in `src/web.rs`.
fn render_view(
    view: &FlaggedView,
    links: &[SearchLink],
    mode: ViewMode,
    poll_interval_seconds: u64,
) -> Markup {
    let body = html! {
        (gap_summary(view))
        div.roots-wrap {
            main id="roots" {
                (roots(view, links, mode))
            }
            (super::page::scan_bar())
            (super::page::search_empty())
        }
    };
    super::page::page(mode, poll_interval_seconds, &body)
}

/// Total gaps across all roots: read directly from each section's
/// precomputed `gaps_within`. `Clean` and `Error` roots contribute zero by
/// construction. Feeds the summary hero and the session bar's load-time
/// baseline.
fn total_gaps(view: &FlaggedView) -> usize {
    view.iter().map(|section| section.gaps_within).sum()
}

/// A root's short label: the last non-empty path segment, for the per-root chips.
fn root_label(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or(path)
}

/// "gap" / "gaps".
fn gap_word(n: usize) -> &'static str {
    if n == 1 { "gap" } else { "gaps" }
}

/// Pluralize "folder" for the partial-scan warning strip.
fn folder_word(n: usize) -> &'static str {
    if n == 1 { "folder" } else { "folders" }
}

/// The gap summary strip between the navbar and the roots. Holds the hero gap
/// total on the left, a library coverage readout with its bar on the right,
/// and optional per-root chips for a multi-root setup. `app.js` keeps every
/// number current as marks land, rescans complete, and refresh polls swap
/// `#roots`. This render is the first paint.
fn gap_summary(view: &FlaggedView) -> Markup {
    let total_gaps = total_gaps(view);
    let total_audiobooks: usize = view.iter().map(|s| s.total_audiobooks).sum();
    let covered = total_audiobooks.saturating_sub(total_gaps);
    // Floor so 199 of 200 reads "99%" rather than rounding up to a false "100%"
    // beside a hero that still says "1 gap to fill". The all-clear branch
    // emits the literal "100%" itself, so the only path that prints `pct`
    // alongside a non-zero gap total is the head, where the floor is honest.
    let pct = if total_audiobooks > 0 {
        ((covered as f64 / total_audiobooks as f64) * 100.0).floor() as usize
    } else {
        0
    };
    let clear_tail_visible = total_audiobooks > 0 && total_gaps == 0;
    html! {
        section.gap-summary id="gap-summary" {
            // Both end-states render. `app.js` toggles `hidden` as the live
            // total crosses zero so the strip converges on what a reload would
            // show, and so an undo back from the last mark can bring the head
            // back. The trailing coverage span is the audiobooks-present
            // variant. A truly empty library keeps the line bare. The two
            // numeric spans inside are the only thing `app.js` rewrites on
            // recompute, so the surrounding "· 100% covered (… of … audiobooks)"
            // wording lives in one place (this template).
            p.gap-summary-clear id="gap-summary-clear" hidden[total_gaps != 0] {
                (PreEscaped(include_str!("../../assets/svg/check.svg")))
                span { "All clear. No gaps in your library." }
                span.coverage-clear id="coverage-clear" hidden[!clear_tail_visible] {
                    " · 100% covered ("
                    span id="coverage-clear-covered" { (total_audiobooks) }
                    " of "
                    span id="coverage-clear-total" { (total_audiobooks) }
                    " audiobooks)"
                }
            }
            div.gap-summary-head id="gap-summary-head" hidden[total_gaps == 0] {
                div.gap-hero {
                    span.gap-hero-num id="gap-total" { (total_gaps) }
                    span.gap-hero-label { (gap_word(total_gaps)) " to fill" }
                }
                (coverage_bar(covered, total_audiobooks, pct))
            }
            @if view.len() > 1 {
                div.gap-chips id="gap-chips" {
                    @for (root, section) in view.iter().enumerate() {
                        (root_chip(root, section))
                    }
                }
            }
        }
    }
}

/// One per-root chip: the root's short label and its own gap count, shown only in
/// a multi-root setup. A covered root reads zero, an error root reads "scan error".
/// The `data-root` hook lets the client update each chip independently.
fn root_chip(root: usize, section: &RootSection) -> Markup {
    html! {
        @match &section.state {
            RootState::Forest(_) => {
                @let n = section.gaps_within;
                span.gap-chip data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { (n) }
                }
            }
            RootState::Clean => {
                span.gap-chip.gap-chip-clean data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "0" }
                }
            }
            RootState::Error(_) => {
                span.gap-chip.gap-chip-error data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "scan error" }
                }
            }
        }
    }
}

/// The library coverage block beside the hero: a readout of covered over total
/// audiobooks (`{pct}% covered · {covered} of {total} audiobooks`) with a
/// `progressbar` so the value is announced. The block always renders, even at
/// a clean load where its head is hidden, because a rescan re-renders only
/// `#roots` and never this strip, so `app.js` reaches in to fill the bar and
/// rewrite the readout when a rescan turns up new gaps.
fn coverage_bar(covered: usize, total: usize, pct: usize) -> Markup {
    html! {
        div.gap-coverage {
            p.gap-coverage-readout {
                span.coverage-num id="coverage-pct" { (pct) } "%"
                " covered · "
                span.coverage-num id="coverage-covered" { (covered) }
                " of "
                span.coverage-num id="coverage-total" { (total) }
                " audiobooks"
            }
            // Floor the max at 1 on a zero-total render so the attribute parses
            // even though the head is hidden in that case. Matches the pattern
            // the old session bar used.
            div.gap-bar role="progressbar"
                aria-label="Library coverage"
                aria-valuemin="0" aria-valuemax=(total.max(1)) aria-valuenow=(covered) {
                span.gap-bar-fill id="coverage-bar-fill"
                    style=(format!("width: {pct}%")) {}
            }
        }
    }
}

/// The badge shown on a root's summary: the gap count, a clean check, or a scan
/// error. In show-all the forest also holds covered nodes, but only gaps are
/// counted. Reads the precomputed `section.gaps_within`.
fn root_badge(section: &RootSection) -> Markup {
    html! {
        @match &section.state {
            RootState::Forest(_) => {
                @let n = section.gaps_within;
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

/// Render one root's section with an optional inline alert. Called by
/// `SectionHandle::render` (mark responses) and `all_sections` (rescan
/// and refresh responses) so every path emits byte-identical section
/// HTML.
fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    let counter = std::cell::Cell::new(0usize);
    let section_id = format!("root-{root}-section");
    html! {
        section.card.root id=(section_id) data-root=(root)
            data-total-audiobooks=(section.total_audiobooks) {
            details.root-fold open {
                summary.root-head {
                    (chevron())
                    h2 { (section.path) }
                    span.spring {}
                    (root_badge(section))
                }
                @if let Some(message) = error {
                    div.alert.alert-error { (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { (message) } }
                }
                @if section.skipped_dirs > 0 {
                    div.alert.alert-warning {
                        (PreEscaped(include_str!("../../assets/svg/warning.svg")))
                        span {
                            (section.skipped_dirs) " " (folder_word(section.skipped_dirs))
                            " couldn't be read; results for this root may be incomplete."
                        }
                    }
                }
                @if section.depth_capped_dirs > 0 {
                    div.alert.alert-warning {
                        (PreEscaped(include_str!("../../assets/svg/warning.svg")))
                        span {
                            (section.depth_capped_dirs) " " (folder_word(section.depth_capped_dirs))
                            " exceeded the scan depth limit and were skipped; results for this root may be incomplete."
                        }
                    }
                }
                @match &section.state {
                    RootState::Forest(nodes) => {
                        @if nodes.is_empty() {
                            // The normal empty-root path renders as Clean
                            // (the arm below). This arm is reached only for the
                            // loose-root edge case where the walk emits one
                            // entry with rel_path = "", which tree::build
                            // skips because insert_all has no components to descend.
                            div.empty { span { "Nothing here" } }
                        } @else {
                            ul.menu {
                                @for node in nodes { (render_node(node, root, links, mode, &counter, 0)) }
                            }
                        }
                    }
                    RootState::Clean => {
                        div.empty { (PreEscaped(include_str!("../../assets/svg/check.svg"))) span { "No missing ebooks in this root" } }
                    }
                    RootState::Error(message) => {
                        div.alert.alert-error {
                            (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { "Could not scan this root: " (message) }
                        }
                    }
                }
            }
        }
    }
}

/// A standalone error card for a root whose section could not be looked up (its
/// index is out of range), carrying the same alert the in-fold error uses. Used by
/// the failed-write path when the view has no section to render into. The zero
/// `data-total-audiobooks` matches the invariant that errored sections carry
/// `data-total-audiobooks="0"` so they fold out of the client-side sum without
/// a special case.
pub(crate) fn error_section(root: usize, message: &str) -> Markup {
    let section_id = format!("root-{root}-section");
    html! {
        section.card.root id=(section_id) data-root=(root) data-total-audiobooks="0" {
            div.alert.alert-error { (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { (message) } }
        }
    }
}

/// The row's mark slot: the amber "needs ebook" pill on a gap, the success check
/// on a covered folder, nothing on a plain container. Its own flex item rather
/// than the tail of the text run, so the mark holds one column down the list
/// however long the names beside it run. Checks are show-all only: gaps-only
/// renders no covered rows, and a gap is already flagged by the amber icon and
/// the pill.
fn row_mark(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if node.needs_ebook() {
            span.row-mark { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
        } @else if mode == ViewMode::All && !node.missing_ebook {
            span.row-mark.row-mark-done {
                span.status title="covered" { (PreEscaped(include_str!("../../assets/svg/check.svg"))) }
            }
        }
    }
}

/// The covering ebook and marker filenames for a covered row, in muted text at
/// the end of the name's run. Show-all only, and empty for gaps and folders
/// covered from above, so nothing renders there.
fn cover_files_span(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if mode == ViewMode::All && !node.cover_files.is_empty() {
            span.cover-files title="covering files" { (node.cover_files.join(", ")) }
        }
    }
}

/// The structural-smell microlabel for a flagged row. A folder that directly holds
/// audio and also has gap subfolders reads as mixed (a book and a shelf at once). A
/// flagged leaf with no container around it (depth 0, including the root-itself gap)
/// reads as loose. A gap filed under a container at depth 1 or deeper gets nothing.
/// Non-flagged nodes never reach a true branch.
fn smell_label(node: &Node, depth: usize) -> Markup {
    html! {
        @if node.needs_ebook() {
            @if !node.children.is_empty() {
                span.smell.smell-mixed { "holds audio + subfolders" }
            } @else if depth == 0 {
                span.smell.smell-loose { "loose at top" }
            }
        }
    }
}

/// The muted "N files" count for a flagged row. Gated on a gap so it appears only
/// where the file disclosure does, never on a covered or container row.
fn file_count(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() && !node.audio_files.is_empty() {
            @let n = node.audio_files.len();
            span.file-count {
                @if n == 1 { "1 file" } @else { (n) " files" }
            }
        }
    }
}

/// The row's text run: the folder name plus the muted notes that read as part of
/// it, ending with the covering filenames. One box at both widths, so a phone-width
/// row flows them inline after the name rather than beside its box, which lets a
/// short filename share the name's line and a long one drop below it. Every note
/// keeps the gate it carries on its own, so the three row branches share one call.
fn row_label(node: &Node, mode: ViewMode, depth: usize) -> Markup {
    html! {
        span.row-label {
            span.name { (node.name) }
            (smell_label(node, depth))
            (file_count(node))
            (cover_files_span(node, mode))
        }
    }
}

/// The audio-file rows for a flagged folder, each a muted, non-actionable line with a
/// music glyph. Emits nothing on a container or covered row, so it is safe to call
/// unconditionally inside the container branch where only a mixed node has files.
fn file_rows(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() {
            @for name in &node.audio_files {
                li.file-row {
                    (PreEscaped(include_str!("../../assets/svg/music.svg")))
                    span.file-name { (name) }
                }
            }
        }
    }
}

fn render_node(
    node: &Node,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
    depth: usize,
) -> Markup {
    // A covered row dims only in show-all. Gaps-only never holds covered nodes.
    let covered = mode == ViewMode::All && !node.missing_ebook;
    // Buttons and links appear only where there is a gap to act on.
    let act = node.has_gap_within();
    html! {
        @if node.children.is_empty() {
            @if node.needs_ebook() {
                // A flagged leaf: an expandable row whose audio files sit hidden under
                // it until opened. It renders as a `<summary>` like a flagged
                // container, a shape `app.js` already handles (see `rowOf`).
                li {
                    details.node-files {
                        summary.row.flagged {
                            (chevron())
                            (folder_icon())
                            (row_label(node, mode, depth))
                            span.spring {}
                            (row_mark(node, mode))
                            (row_actions(root, &node.rel_path, &node.name, links, mode, counter, act))
                        }
                        ul.files { (file_rows(node)) }
                    }
                }
            } @else {
                // A non-flagged leaf (a covered or plain folder in show-all) stays a
                // static row.
                li {
                    div.row.covered[covered] {
                        span.leaf-pad {}
                        (folder_icon())
                        (row_label(node, mode, depth))
                        span.spring {}
                        (row_mark(node, mode))
                        (row_actions(root, &node.rel_path, &node.name, links, mode, counter, act))
                    }
                }
            }
        } @else {
            li {
                details open {
                    summary.row
                        .flagged[node.needs_ebook()]
                        .covered[covered]
                        .container-top[!node.needs_ebook() && depth == 0]
                        .container-nested[!node.needs_ebook() && depth > 0] {
                        (chevron())
                        (folder_icon())
                        (row_label(node, mode, depth))
                        span.spring {}
                        (row_mark(node, mode))
                        (row_actions(root, &node.rel_path, &node.name, links, mode, counter, act))
                    }
                    ul {
                        (file_rows(node))
                        @for child in &node.children { (render_node(child, root, links, mode, counter, depth + 1)) }
                    }
                }
            }
        }
    }
}

/// The per-row action slot: a kebab trigger plus the marker buttons and search
/// links, wrapped in a group that doubles as a native popover. On desktop the
/// trigger is hidden and the group is `display: contents`, so its children flow
/// inline in the slot. On mobile the kebab opens the group as a bottom action
/// sheet over a dimmed backdrop. The browser provides the toggle,
/// one-open-at-a-time, light-dismiss, and Esc.
///
/// The slot renders on every row and `act` gates only its contents, so a row with
/// nothing to act on still reserves the column. That is the horizontal twin of the
/// row's reserved `min-height`: without it the mark beside it would shift on
/// exactly the rows that carry no buttons.
fn row_actions(
    root: usize,
    rel: &str,
    name: &str,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
    act: bool,
) -> Markup {
    html! {
        span.row-actions {
            @if act {
                @let group_id = next_id("acts", root, counter);
                button.actions-trigger type="button"
                    aria-label="Actions"
                    aria-haspopup="menu"
                    popovertarget=(group_id)
                    onclick="event.stopPropagation()" { (PreEscaped(include_str!("../../assets/svg/kebab.svg"))) }
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
    }
}

fn marker_buttons(root: usize, rel: &str, name: &str, mode: ViewMode) -> Markup {
    // In gaps-only the marked folder leaves the list, so `app.js` collapses its row
    // and the section swap waits for that to play. In show-all the row stays and
    // flips to covered in place, so the swap is immediate, and the row's reserved
    // min-height keeps the flip from shifting the rows below.
    let swap = match mode {
        ViewMode::GapsOnly => "outerHTML swap:250ms",
        ViewMode::All => "outerHTML",
    };
    html! {
        form.mark.actions hx-target="closest section.root" hx-swap=(swap) {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            input type="hidden" name="view" value=(mode.as_query());
            button.btn.btn-outline.btn-xs.btn-square type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                data-confirm-action="No ebook"
                data-confirm-file=".no_ebook"
                data-confirm-folder=(name)
                data-tip="No ebook"
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/no-entry.svg"))) }
                    span.label-long { "No ebook" }
                    span.label-short { "No ebook" }
                }
            button.btn.btn-outline.btn-xs.btn-square type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                data-confirm-action="Ebook elsewhere"
                data-confirm-file=".ebook_elsewhere"
                data-confirm-folder=(name)
                data-tip="Ebook elsewhere"
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/ebook-elsewhere.svg"))) }
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
            @let query = percent_encode(&clean_query(name));
            @let id = next_id("links", root, counter);
            span.links {
                span.sheet-divider { "Search" }
                button.btn.btn-outline.btn-xs.btn-square.links-toggle type="button"
                    popovertarget=(id)
                    aria-label="Search for this book"
                    data-tip="Search links"
                    onclick="event.stopPropagation()" { (PreEscaped(include_str!("../../assets/svg/search.svg"))) }
                div.links-menu popover="auto" id=(id) onclick="event.stopPropagation()" {
                    @for link in links {
                        a href=(link.url.replace("{query}", &query))
                            target="_blank" rel="noopener noreferrer" {
                                span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/search.svg"))) }
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

    /// A directly-flagged leaf: holds audio, missing an ebook, audio filenames
    /// as given. Cover files empty.
    fn flagged_leaf(name: &str, rel: &str, audio: &[&str]) -> Node {
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: Vec::new(),
            cover_files: Vec::new(),
            audio_files: audio.iter().map(|s| (*s).into()).collect(),
            gaps_within: 1,
        }
    }

    /// A covered leaf: holds audio AND has at least one cover file, so it is
    /// not flagged. Used by all-view tests that pin cover-file rendering.
    fn covered_leaf(name: &str, rel: &str, cover_files: &[&str]) -> Node {
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: true,
            missing_ebook: false,
            children: Vec::new(),
            cover_files: cover_files.iter().map(|s| (*s).into()).collect(),
            audio_files: Vec::new(),
            gaps_within: 0,
        }
    }

    /// A container row: no audio of its own, holds children.
    fn container(name: &str, rel: &str, children: Vec<Node>) -> Node {
        let gaps_within = children.iter().map(|c| c.gaps_within).sum();
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: false,
            missing_ebook: false,
            children,
            cover_files: Vec::new(),
            audio_files: Vec::new(),
            gaps_within,
        }
    }

    /// Wrap a forest of root-level nodes into the `RootState::Forest` arm.
    fn forest(roots: Vec<Node>) -> RootState {
        RootState::Forest(roots)
    }

    /// One library root labeled with its display path, the given state, and
    /// the audiobook count `render_section` emits as `data-total-audiobooks`.
    fn section(path: &str, state: RootState, total: usize) -> RootSection {
        let gaps_within = match &state {
            RootState::Forest(nodes) => nodes.iter().map(|n| n.gaps_within).sum(),
            RootState::Clean | RootState::Error(_) => 0,
        };
        RootSection {
            path: path.into(),
            state,
            total_audiobooks: total,
            gaps_within,
            skipped_dirs: 0,
            depth_capped_dirs: 0,
        }
    }

    /// A root the scanner walked and found nothing missing in.
    fn clean(path: &str, total: usize) -> RootSection {
        section(path, RootState::Clean, total)
    }

    /// A root that errored at canonicalization or walk time.
    fn errored(path: &str, message: &str) -> RootSection {
        section(path, RootState::Error(message.into()), 0)
    }

    #[test]
    fn index_renders_a_flagged_folder() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains("Book"));
    }

    #[test]
    fn index_tags_container_rows_by_depth() {
        // Author (top container) -> Series (nested container) -> Book (flagged leaf).
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![flagged_leaf("Book", "Author/Series/Book", &["01.mp3"])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The top container is tagged for bold, the nested one for italic.
        assert!(html.contains(r#"class="row container-top""#));
        assert!(html.contains(r#"class="row container-nested""#));
        // The flagged leaf keeps exactly its existing class, with no depth tag.
        assert!(html.contains(r#"class="row flagged""#));
    }

    #[test]
    fn index_leaves_a_deep_gap_unmarked() {
        // A properly filed gap two levels down carries no smell.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![flagged_leaf("Book", "Author/Series/Book", &["01.mp3"])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(!html.contains("loose at top"));
        assert!(!html.contains("holds audio + subfolders"));
    }

    #[test]
    fn show_all_keeps_depth_tags_on_covered_containers() {
        // Audio under a series with an ebook at the author level covers the
        // whole branch in show-all. The containers stay (no audio of their own,
        // so not flagged), and the book leaf flips from flagged to covered.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![covered_leaf("Book", "Author/Series/Book", &[])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        // The covered top and nested containers still carry their depth tags, so the
        // depth cue survives the view switch and composes with the covered class.
        assert!(html.contains(r#"class="row covered container-top""#));
        assert!(html.contains(r#"class="row covered container-nested""#));
        // The covered leaf book carries the bare covered class with no depth tag
        // (the trailing quote rules out a `covered container-*` prefix match).
        assert!(html.contains(r#"class="row covered""#));
    }

    #[test]
    fn section_carries_a_data_root_hook() {
        // The toast's Undo targets the section by index, so each section names it.
        let view = section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        );
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains(r#"data-root="0""#));
    }

    #[test]
    fn index_marks_loose_and_mixed_flagged_folders() {
        // A loose gap: a flagged leaf at the top of the root.
        let loose = flagged_leaf("The Hobbit", "The Hobbit", &["01.mp3"]);

        // A mixed gap: a parent that itself holds audio AND has a flagged child.
        let mixed = Node {
            name: "Terry Pratchett".into(),
            rel_path: "Terry Pratchett".into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: vec![flagged_leaf(
                "Going Postal",
                "Terry Pratchett/Going Postal",
                &["01.mp3"],
            )],
            cover_files: Vec::new(),
            audio_files: vec!["01.mp3".into()],
            gaps_within: 2,
        };

        let view = vec![section("/lib", forest(vec![loose, mixed]), 3)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();

        assert!(
            html.contains("loose at top"),
            "the top-level book is marked loose"
        );
        assert!(
            html.contains("holds audio + subfolders"),
            "the half-sorted author is marked mixed"
        );
    }

    #[test]
    fn index_shows_a_file_count_and_a_collapsed_file_list_on_a_flagged_leaf() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Book",
                "Book",
                &["01 - The Gunslinger.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The count sits on the row, and the file row is present but inside a closed
        // <details> (no `open`), so the names are hidden until the row is expanded.
        assert!(html.contains("1 file"));
        assert!(html.contains(r#"<details class="node-files">"#));
        assert!(html.contains("01 - The Gunslinger.mp3"));
    }

    #[test]
    fn index_pluralizes_the_file_count() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Book",
                "Book",
                &["01.mp3", "02.mp3", "03.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains("3 files"));
        // The trailing space pins this to the singular row text. A bare
        // "3 file" would also match the plural "3 files", so we anchor on
        // the space that follows on a row that says e.g. "3 file ▸".
        assert!(
            !html.contains("3 file "),
            "rendered singular instead of plural"
        );
    }

    #[test]
    fn mixed_node_shows_its_own_files_above_its_child_gap() {
        // A mixed author: holds its own loose audio file AND has a flagged child.
        let mixed = Node {
            name: "Terry Pratchett".into(),
            rel_path: "Terry Pratchett".into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: vec![flagged_leaf(
                "Going Postal",
                "Terry Pratchett/Going Postal",
                &["01.mp3"],
            )],
            cover_files: Vec::new(),
            audio_files: vec!["01 - The Colour of Magic.mp3".into()],
            gaps_within: 2,
        };
        let view = vec![section("/lib", forest(vec![mixed]), 2)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The mixed author's own loose file renders as a file row, and the child gap
        // still renders as a folder row carrying its badge.
        assert!(html.contains("01 - The Colour of Magic.mp3"));
        assert!(html.contains(r#"class="file-row""#));
        assert!(html.contains("Going Postal"));
    }

    #[test]
    fn index_wraps_the_sections_in_a_roots_container() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The root sections live inside a positioned wrapper so the rescan bar
        // can pin above them, and inside #roots so htmx can swap them in place.
        assert!(html.contains(r#"class="roots-wrap""#));
        assert!(html.contains(r#"id="roots""#));
        // The sections themselves are unchanged.
        assert!(html.contains(r#"class="card root""#));
    }

    #[test]
    fn each_root_renders_a_collapsible_summary_with_a_gap_count() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Book", "Author/Book", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The root head is now a <summary> inside a collapsible <details>.
        assert!(html.contains(r#"class="root-fold""#));
        assert!(html.contains("root-head"));
        // One gap under this root, so the badge reads "1 gap".
        assert!(html.contains("1 gap"));
    }

    #[test]
    fn a_clean_root_badge_reads_no_gaps() {
        let view = vec![clean("/lib", 1)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains("no gaps"));
    }

    #[test]
    fn all_view_shows_nothing_here_for_a_root_with_no_folders() {
        // A walked-but-empty root in show-all keeps the `Forest(vec![])` arm
        // so the "Nothing here" branch fires for the loose-root edge case.
        let view = vec![section("/lib", forest(vec![]), 0)];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        assert!(html.contains("Nothing here"));
    }

    #[test]
    fn index_shows_the_clean_message_for_a_covered_root() {
        let view = vec![clean("/lib", 1)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains("No missing ebooks in this root"));
    }

    #[test]
    fn section_open_tag_carries_total_audiobooks_data_attr() {
        // Two audiobook folders under a shared parent. The renderer takes
        // the count verbatim from RootSection.total_audiobooks.
        let view = section(
            "/lib",
            forest(vec![container(
                "A",
                "A",
                vec![
                    flagged_leaf("B1", "A/B1", &["01.mp3"]),
                    covered_leaf("B2", "A/B2", &["B2.epub"]),
                ],
            )]),
            2,
        );
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        // Walked root carries the audiobook total on its outer <section>.
        assert!(html.contains(r#"data-total-audiobooks="2""#));
    }

    #[test]
    fn section_open_tag_carries_zero_total_audiobooks_for_errored_root() {
        let view = errored("/no/such/root/xyz123", "no such file or directory");
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        // Errored root still carries the attribute so the JS aggregator
        // never has to special-case missing attrs.
        assert!(html.contains(r#"data-total-audiobooks="0""#));
    }

    #[test]
    fn a_root_with_skipped_directories_renders_the_partial_scan_warning() {
        let mut view = section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        );
        view.skipped_dirs = 3;
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("alert-warning"));
        assert!(
            html.contains("3 folders couldn't be read; results for this root may be incomplete.")
        );
        assert!(html.contains("Book"), "the readable rows still render");
    }

    #[test]
    fn a_root_with_depth_capped_directories_renders_a_distinct_warning() {
        let mut view = section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        );
        view.depth_capped_dirs = 2;
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("alert-warning"));
        assert!(
            html.contains(
                "2 folders exceeded the scan depth limit and were skipped; results for this root may be incomplete."
            ),
            "the depth-cap warning names its own cause rather than reusing the unreadable-directory wording"
        );
        assert!(
            !html.contains("couldn't be read"),
            "a depth-capped root did not also fail to read anything"
        );
    }

    #[test]
    fn a_fully_read_root_renders_no_partial_scan_warning() {
        let view = section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        );
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(!html.contains("alert-warning"));
    }

    #[test]
    fn one_skipped_directory_reads_in_the_singular() {
        let mut view = section("/lib", RootState::Clean, 1);
        view.skipped_dirs = 1;
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(
            html.contains("1 folder couldn't be read; results for this root may be incomplete.")
        );
    }

    /// Default search-link set, matching what `Config::default()` ships and
    /// what the in-repo router tests historically asserted against.
    fn default_links() -> Vec<SearchLink> {
        vec![
            SearchLink {
                label: "Goodreads".into(),
                url: "https://www.goodreads.com/search?q={query}".into(),
            },
            SearchLink {
                label: "OceanofPDF".into(),
                url: "https://oceanofpdf.com/?s={query}".into(),
            },
        ]
    }

    #[test]
    fn marker_form_delays_the_swap_only_in_gaps_only() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        // Gaps-only: the marked folder leaves the list, so the section swap is delayed
        // to let app.js play the row's collapse before the fresh section lands.
        let gaps = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(gaps.contains(r#"hx-swap="outerHTML swap:250ms""#));
        // Show-all: the row flips to covered in place, so the swap is immediate. The
        // reserved row height keeps the flip from shifting the rows below.
        let all = render_view(&view, &[], ViewMode::All, 0).into_string();
        assert!(all.contains(r#"hx-swap="outerHTML""#));
        assert!(!all.contains("swap:250ms"));
    }

    #[test]
    fn index_renders_the_marker_buttons() {
        // Renderer-half of the original `index_renders_the_marker_buttons_and_script`.
        // The body-end `<script>` assertions live in `page::tests` (P1).
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains(r#"hx-post="/mark""#));
        assert!(html.contains(">No ebook<"));
    }

    #[test]
    fn elsewhere_button_uses_the_book_check_icon() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The "Ebook elsewhere" button now carries a book-and-check glyph (the
        // checkmark path), not the old open-external-link arrow.
        assert!(html.contains("m9 9.5 2 2 4-4"));
        assert!(!html.contains("M10 14L21 3"));
    }

    #[test]
    fn marker_buttons_carry_confirm_metadata() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // Each marker button names its action, file, and folder for the dialog.
        assert!(html.contains(r#"data-confirm-action="No ebook""#));
        assert!(html.contains(r#"data-confirm-file=".no_ebook""#));
        assert!(html.contains(r#"data-confirm-action="Ebook elsewhere""#));
        assert!(html.contains(r#"data-confirm-file=".ebook_elsewhere""#));
        assert!(html.contains(r#"data-confirm-folder="Book""#));
        // Each button names itself in a CSS tooltip. Native `title` is off: its
        // delay is browser and OS controlled, and leaving it stacks two tooltips.
        // The full sentence lives in the confirm dialog, which is read before
        // anything is written.
        assert!(html.contains(r#"data-tip="No ebook""#));
        assert!(html.contains(r#"data-tip="Ebook elsewhere""#));
        assert!(!html.contains("Covers this folder and everything beneath it"));
    }

    #[test]
    fn all_view_dims_covered_rows_and_omits_their_buttons() {
        // A covered container (series epub) whose books are all covered.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Series",
                "Series",
                vec![covered_leaf("Book", "Series/Book", &[])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        // Covered rows carry the success check and the covered class.
        assert!(html.contains(r#"title="covered""#));
        assert!(html.contains(r#"covered""#));
        // A fully covered branch carries no marker buttons.
        assert!(!html.contains(r#"hx-post="/mark""#));
    }

    #[test]
    fn all_view_keeps_buttons_on_a_container_above_a_gap() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Gap", "Author/Gap", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        // The author is a plain container above a gap, so it still gets buttons.
        assert!(html.contains(r#"hx-post="/mark""#));
        assert!(html.contains("Gap"));
    }

    #[test]
    fn each_actionable_row_has_an_actions_trigger() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        // A labelled kebab that opens the per-row action sheet via the native
        // popover API, and the group that is that popover.
        assert!(html.contains(r#"class="actions-trigger""#));
        assert!(html.contains(r#"aria-label="Actions""#));
        assert!(html.contains(r#"aria-haspopup="menu""#));
        assert!(html.contains("popovertarget"));
        assert!(html.contains(r#"class="actions-group""#));
        assert!(html.contains(r#"popover="auto""#));
        // The group is labelled with the folder name and titles the sheet with it.
        assert!(html.contains(r#"aria-label="Book""#));
        assert!(html.contains(r#"class="sheet-title""#));
        // The marker buttons and search links still render inside the group.
        assert!(html.contains(r#"hx-post="/mark""#));
        assert!(html.contains(">No ebook<"));
        assert!(html.contains("Goodreads"));
    }

    #[test]
    fn the_action_sheet_titles_with_the_folder_and_shows_verbose_labels() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The sheet header titles the sheet with the folder name.
        assert!(html.contains(r#"class="sheet-title">Book<"#));
        // The elsewhere marker keeps a verbose sheet label distinct from its
        // compact pill. The no-ebook marker reads "No ebook" in both registers.
        assert!(html.contains("Ebook elsewhere"));
        // The compact labels render with their exact pill text.
        assert!(html.contains(">No ebook<"));
        assert!(html.contains(">Elsewhere<"));
    }

    #[test]
    fn the_action_sheet_marks_the_search_section() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        // A sheet-only "Search" divider separates the marker rows from the links.
        assert!(html.contains(r#"class="sheet-divider""#));
        // The links still resolve to their configured search URLs.
        assert!(html.contains("https://www.goodreads.com/search?q=Book"));
    }

    #[test]
    fn a_covered_row_has_no_actions_trigger() {
        // A fully covered branch: the book has its own ebook, nothing to act on.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Series",
                "Series",
                vec![covered_leaf("Book", "Series/Book", &[])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        // No gap under this branch, so no trigger and no group are emitted.
        assert!(!html.contains(r#"class="actions-trigger""#));
        assert!(!html.contains(r#"class="actions-group""#));
    }

    #[test]
    fn marking_in_all_mode_shows_the_written_marker_on_the_row() {
        // After a no-ebook mark lands, the row flips from flagged to covered
        // and the marker file shows up in `cover_files`. Re-rendering the
        // section in show-all then carries the marker on the row.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![covered_leaf("Book", "Author/Book", &[".no_ebook"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        assert!(html.contains(r#"class="cover-files""#));
        assert!(html.contains(".no_ebook"));
    }

    #[test]
    fn index_renders_the_search_links() {
        // Goodreads ships as a default link. The (Unabridged) suffix is stripped from
        // the query, so the href ends in `q=Book`, and the links open in a new tab.
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Book (Unabridged)",
                "Book (Unabridged)",
                &["01.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains(r#"target="_blank""#));
        assert!(html.contains("https://www.goodreads.com/search?q=Book"));
        assert!(html.contains("Goodreads"));
    }

    #[test]
    fn index_renders_every_configured_link() {
        // The defaults ship two links. Both must render, not just the first.
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        assert!(html.contains("https://www.goodreads.com/search?q=Book"));
        assert!(html.contains("https://oceanofpdf.com/?s=Book"));
        assert!(html.contains("OceanofPDF"));
    }

    #[test]
    fn index_omits_the_links_span_when_none_are_configured() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // No links means no `span.links` is emitted, and no search popover menu.
        // The kebab still carries `popovertarget` and is the sheet trigger now.
        assert!(!html.contains(r#"class="links""#));
        assert!(!html.contains(r#"class="links-menu""#));
        assert!(!html.contains(r#"title="Search links""#));
    }

    #[test]
    fn search_links_render_inside_a_popover_menu() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        // A magnifying-glass button opens a popover that holds the links.
        assert!(html.contains("popovertarget"));
        assert!(html.contains(r#"class="links-menu""#));
        // The link itself is unchanged, just relocated into the menu.
        assert!(html.contains("https://www.goodreads.com/search?q=Book"));
        assert!(html.contains(r#"target="_blank""#));
    }

    #[test]
    fn search_link_query_percent_encodes_spaces() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Author Name",
                "Author Name",
                &["01.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::GapsOnly, 0).into_string();
        // Spaces in the cleaned query are percent-encoded, so the href carries `%20`.
        assert!(html.contains("q=Author%20Name"));
    }

    #[test]
    fn all_view_lists_the_covering_ebook_on_a_covered_row() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![covered_leaf("Covered", "Author/Covered", &["Covered.epub"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        assert!(html.contains(r#"class="cover-files""#));
        assert!(html.contains("Covered.epub"));
    }

    #[test]
    fn gaps_only_view_lists_no_cover_files() {
        // In gaps-only the cached view drops covered nodes entirely, so the
        // tree feeds the renderer only the flagged leaf.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Gap", "Author/Gap", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(!html.contains(r#"class="cover-files""#));
    }

    #[test]
    fn gaps_only_view_has_no_status_icons_or_covered_rows() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Book", "Author/Book", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // No status markers and no covered rows in the gaps-only output.
        assert!(!html.contains(r#"class="status""#));
        assert!(!html.contains(r#" covered""#));
        // The gap and its buttons are still there.
        assert!(html.contains("Book"));
        assert!(html.contains(r#"hx-post="/mark""#));
    }

    #[test]
    fn all_view_renders_covered_folders_that_gaps_only_drops() {
        // Gaps-only's cached view drops the covered book entirely.
        let gaps_view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Gap", "Author/Gap", &["01.mp3"])],
            )]),
            2,
        )];
        let gaps = render_view(&gaps_view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(!gaps.contains("Covered"));

        // Show-all keeps the covered book beside the flagged one.
        let all_view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![
                    covered_leaf("Covered", "Author/Covered", &["Covered.epub"]),
                    flagged_leaf("Gap", "Author/Gap", &["01.mp3"]),
                ],
            )]),
            2,
        )];
        let all = render_view(&all_view, &[], ViewMode::All, 0).into_string();
        assert!(all.contains("Covered"));
        assert!(all.contains("Gap"));
    }

    #[test]
    fn index_renders_the_gap_summary_strip() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Book", "Author/Book", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The strip renders server-side, between the navbar and the roots.
        assert!(html.contains(r#"id="gap-summary""#));
        // The hero gap total has its own hook. The library coverage readout
        // and bar carry the new coverage-* ids.
        assert!(html.contains(r#"id="gap-total""#));
        assert!(html.contains(r#"id="coverage-pct""#));
        assert!(html.contains(r#"id="coverage-covered""#));
        assert!(html.contains(r#"id="coverage-total""#));
        assert!(html.contains("audiobooks"));
        // The all-clear line renders too, hidden until the live total reaches zero.
        assert!(html.contains(r#"id="gap-summary-clear" hidden"#));
    }

    #[test]
    fn gap_summary_initial_paint_carries_library_coverage_readout() {
        // Three audiobooks, one of them a gap, two covered.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "A",
                "A",
                vec![
                    flagged_leaf("B1", "A/B1", &["01.mp3"]),
                    covered_leaf("B2", "A/B2", &["B2.epub"]),
                    covered_leaf("B3", "A/B3", &["B3.epub"]),
                ],
            )]),
            3,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The strip carries the three coverage hooks the JS keeps current.
        assert!(html.contains(r#"id="coverage-pct""#));
        assert!(html.contains(r#"id="coverage-covered""#));
        assert!(html.contains(r#"id="coverage-total""#));
        assert!(html.contains(r#"id="coverage-bar-fill""#));
        // Bar values match the load: covered=2, total=3, label is the library.
        assert!(html.contains(r#"aria-label="Library coverage""#));
        assert!(html.contains(r#"aria-valuenow="2""#));
        assert!(html.contains(r#"aria-valuemax="3""#));
    }

    #[test]
    fn coverage_percentage_floors_on_a_199_of_200_fixture() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Gap", "Gap", &["01.mp3"])]),
            200,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // 199 covered of 200 floors to 99, never rounds to a false 100
        assert!(html.contains(r#"id="coverage-pct">99<"#));
        assert!(html.contains(r#"aria-valuenow="199""#));
        assert!(html.contains(r#"aria-valuemax="200""#));
    }

    #[test]
    fn gap_summary_all_clear_with_audiobooks_shows_trailing_coverage_fragment() {
        // Two covered audiobooks, no gaps: the cached gaps-only view collapses
        // an empty forest to `Clean`, so the fixture mirrors that.
        let view = vec![clean("/lib", 2)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // All-clear branch is visible, the trailing coverage span shows the
        // T of T fragment and is not hidden. The numbers ride in their own
        // child spans so app.js only rewrites the digits and the surrounding
        // wording stays in the server template.
        assert!(html.contains(r#"id="gap-summary-clear">"#));
        assert!(html.contains(r#"id="gap-summary-head" hidden"#));
        assert!(html.contains(r#"<span class="coverage-clear" id="coverage-clear">"#));
        assert!(html.contains("100% covered ("));
        assert!(html.contains(r#"id="coverage-clear-covered">2</span>"#));
        assert!(html.contains(r#"id="coverage-clear-total">2</span>"#));
        assert!(html.contains("audiobooks)"));
    }

    #[test]
    fn gap_summary_empty_library_keeps_coverage_fragment_hidden() {
        // No audio at all. The strip is in its empty-library state.
        let view = vec![clean("/lib", 0)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The all-clear line shows but the coverage trailing fragment stays
        // hidden so the line does not read "0 of 0".
        assert!(html.contains("All clear"));
        assert!(html.contains(r#"id="coverage-clear" hidden"#));
        assert!(html.contains(r#"id="gap-summary-head" hidden"#));
    }

    #[test]
    fn gap_summary_excludes_errored_roots_from_the_coverage_total() {
        // 100 audiobooks under the good root, all of them gaps, plus an
        // errored root that contributes neither audiobooks nor gaps.
        let mut leaves = Vec::new();
        for i in 0..100 {
            let name = format!("B{i:03}");
            leaves.push(flagged_leaf(&name, &name, &["01.mp3"]));
        }
        let view = vec![
            section("/good", forest(leaves), 100),
            errored("/no/such/root/xyz123", "no such file or directory"),
        ];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // Total reads 100 (errored root contributes zero). Covered 0, pct 0.
        assert!(html.contains(r#"aria-valuemax="100""#));
        assert!(html.contains(r#"aria-valuenow="0""#));
        // The readout text reflects the same numbers.
        assert!(html.contains(r#"id="coverage-total">100"#));
        assert!(html.contains(r#"id="coverage-covered">0"#));
    }

    #[test]
    fn gap_summary_shows_all_clear_for_a_covered_library() {
        let view = vec![clean("/lib", 1)];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // Total zero: the all-clear line shows and the head loads hidden so an
        // undo back from the last mark can bring it back.
        assert!(html.contains("All clear"));
        assert!(html.contains(r#"id="gap-summary-clear">"#));
        assert!(html.contains(r#"id="gap-summary-head" hidden"#));
        // The trailing coverage fragment carries the audiobook count and is
        // visible because the library has audiobooks but no gaps. The numbers
        // ride in their own child spans.
        assert!(html.contains("100% covered ("));
        assert!(html.contains(r#"id="coverage-clear-covered">1</span>"#));
        assert!(html.contains(r#"id="coverage-clear-total">1</span>"#));
        assert!(html.contains("audiobooks)"));
        // The hidden bar still floors its max at 1, never a degenerate max-of-zero.
        assert!(html.contains(r#"aria-valuemax="1""#));
        assert!(!html.contains(r#"aria-valuemax="0""#));
    }

    #[test]
    fn gap_summary_renders_a_chip_per_root_for_a_multi_root_config() {
        let view = vec![
            section(
                "/a",
                forest(vec![flagged_leaf("BookA", "BookA", &["01.mp3"])]),
                1,
            ),
            section(
                "/b",
                forest(vec![flagged_leaf("BookB", "BookB", &["01.mp3"])]),
                1,
            ),
        ];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // One chip per root, each with its own gap count and a data-root hook the
        // client recompute updates.
        assert!(html.contains(r#"id="gap-chips""#));
        assert!(html.contains(r#"class="gap-chip" data-root="0""#));
        assert!(html.contains(r#"data-root="1""#));
    }

    #[test]
    fn gap_summary_omits_chips_for_a_single_root() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(!html.contains(r#"id="gap-chips""#));
    }

    #[test]
    fn gap_summary_chips_handle_a_clean_and_an_error_root() {
        let view = vec![
            clean("/good", 1),
            errored("/no/such/root/xyz123", "no such file or directory"),
        ];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // Total is zero, so the all-clear message shows, and a multi-root setup still
        // gets its chips, the error root labelled.
        assert!(html.contains("All clear"));
        assert!(html.contains(r#"id="gap-chips""#));
        assert!(html.contains("gap-chip-clean"));
        assert!(html.contains("gap-chip-error"));
        assert!(html.contains("scan error"));
    }

    #[test]
    fn gap_summary_renders_a_library_coverage_progressbar() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Book", "Author/Book", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // A progressbar that measures the library: covered over total audiobooks.
        // With one audiobook and one gap, covered=0 of total=1.
        assert!(html.contains(r#"role="progressbar""#));
        assert!(html.contains(r#"aria-label="Library coverage""#));
        assert!(html.contains(r#"aria-valuenow="0""#));
        assert!(html.contains(r#"aria-valuemax="1""#));
        assert!(html.contains(r#"aria-valuemin="0""#));
        assert!(html.contains(r#"id="coverage-bar-fill""#));
    }

    #[test]
    fn index_renders_the_menu_with_a_flagged_badge() {
        // `ul.menu`, `section.card.root`, and the `needs ebook` badge are all
        // emitted by the renderer (render_section + render_node). The original
        // test reached through the router to assert on them.
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The tree is now a `menu`, and the styled section keeps the `root` hook.
        assert!(html.contains(r#"class="menu""#));
        assert!(html.contains(r#"class="card root""#));
        // A flagged folder carries the warning badge.
        assert!(html.contains("needs ebook"));
    }

    #[test]
    fn the_flagged_badge_carries_a_hover_title() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The mobile dot has no visible text, so the badge gets a title that names
        // it on hover. The literal label is still emitted as the badge's content.
        assert!(html.contains(r#"title="needs ebook""#));
        assert!(html.contains("needs ebook"));
    }

    // Integration-test helpers for the packaging tests below. These build a
    // real `RawView` via `raw_view::build_view`, then exercise `package_view` /
    // `package_section` against it. The synthetic-fixture helpers above are
    // for markup tests. These are for packaging tests.

    use crate::config::Config;
    use crate::raw_view::build_view;
    use crate::scanner::{DirIndex, ScanSettings};
    use crate::scenarios::touch;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_config(roots: Vec<PathBuf>, ttl_seconds: u64) -> Config {
        Config {
            library_roots: roots,
            ttl_seconds,
            ..Default::default()
        }
    }

    fn test_settings() -> Arc<ScanSettings> {
        Arc::new(ScanSettings::compile(Config::default().scan_inputs()).unwrap())
    }

    fn test_indices(roots: usize) -> Vec<Arc<DirIndex>> {
        (0..roots).map(|_| Arc::new(DirIndex::new())).collect()
    }

    #[tokio::test]
    async fn package_view_root_with_a_gap_yields_a_matching_forest() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("Author/Book/01.mp3"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert_eq!(view.len(), 1);
        match &view[0].state {
            RootState::Forest(nodes) => {
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].name, "Author");
            }
            other => panic!("expected Forest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn package_view_root_with_no_audio_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Empty")).unwrap();
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Clean));
    }

    #[tokio::test]
    async fn package_view_missing_root_is_error_and_other_roots_still_render() {
        let good = tempfile::tempdir().unwrap();
        touch(&good.path().join("Book/01.mp3"));
        let cfg = test_config(
            vec![
                PathBuf::from("/no/such/root/xyz123"),
                good.path().to_path_buf(),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), &test_indices(2)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);
        assert!(matches!(view[0].state, RootState::Error(_)));
        assert!(matches!(view[1].state, RootState::Forest(_)));
    }

    #[tokio::test]
    async fn package_view_computes_total_audiobooks_per_root() {
        let walked = tempfile::tempdir().unwrap();
        // Two audiobooks under one author, plus a covered one.
        touch(&walked.path().join("A/B1/01.mp3"));
        touch(&walked.path().join("A/B2/01.mp3"));
        touch(&walked.path().join("A/B2/B2.epub"));

        let clean = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(clean.path().join("Empty")).unwrap();

        let cfg = test_config(
            vec![
                walked.path().to_path_buf(),
                clean.path().to_path_buf(),
                PathBuf::from("/no/such/root/xyz123"),
            ],
            60,
        );
        let raw = build_view(&cfg, &test_settings(), &test_indices(3)).await;
        let view = package_view(&raw, ViewMode::GapsOnly);

        assert_eq!(view[0].total_audiobooks, 2, "two audiobook folders");
        assert_eq!(view[1].total_audiobooks, 0, "clean root");
        assert_eq!(view[2].total_audiobooks, 0, "errored root");
    }

    #[tokio::test]
    async fn package_view_all_mode_builds_the_full_tree_including_covered_folders() {
        let dir = tempfile::tempdir().unwrap();
        // A gap and a covered book under the same author.
        touch(&dir.path().join("Author/Gap/01.mp3"));
        touch(&dir.path().join("Author/Covered/01.mp3"));
        touch(&dir.path().join("Author/Covered/Covered.epub"));
        let cfg = test_config(vec![dir.path().to_path_buf()], 60);
        let raw = build_view(&cfg, &test_settings(), &test_indices(1)).await;
        let view = package_view(&raw, ViewMode::All);
        let RootState::Forest(nodes) = &view[0].state else {
            panic!("show-all always yields a Forest");
        };
        let author = &nodes[0];
        assert_eq!(author.name, "Author");
        let names: Vec<&str> = author.children.iter().map(|n| n.name.as_str()).collect();
        // Both books appear, unlike gaps-only which would drop Covered.
        assert_eq!(names, vec!["Covered", "Gap"]);
    }

    /// Render byte-equality is a load-bearing invariant: two reads of the
    /// same mode must serialize identically, a mode flip must change the
    /// bytes, and a mark+undo round trip on the same folder must restore the
    /// packaged view byte-for-byte.
    #[tokio::test]
    async fn render_is_byte_equal_across_hits_and_a_mark_undo_round_trip() {
        use crate::marker::Marker;
        use crate::scenarios;
        use crate::state::AppState;
        use crate::tree::Node;

        let dir = tempfile::tempdir().unwrap();
        let scenario = scenarios::find_scenario("mixed-forest").expect("scenario exists");
        let roots = scenarios::materialize(&(scenario.spec)(), dir.path());

        let config = Config {
            library_roots: roots,
            ttl_seconds: 600,
            ..Config::default()
        };
        let links = config.search_links.clone();
        let settings = ScanSettings::compile(config.scan_inputs()).unwrap();
        let state = std::sync::Arc::new(AppState::new(config, settings));

        // Two reads of the same mode on a warm cache must serialize identically.
        let raw_one = state.store.current().await;
        let gaps_one = package_view(&raw_one, ViewMode::GapsOnly);
        let raw_two = state.store.current().await;
        let gaps_two = package_view(&raw_two, ViewMode::GapsOnly);
        assert_eq!(
            render_view(&gaps_one, &links, ViewMode::GapsOnly, 0).into_string(),
            render_view(&gaps_two, &links, ViewMode::GapsOnly, 0).into_string(),
            "two reads of the same mode must produce byte-equal renders",
        );

        // A mode flip on the same warm cache must produce a different shape.
        let all_one = package_view(&raw_one, ViewMode::All);
        assert_ne!(
            render_view(&gaps_one, &links, ViewMode::GapsOnly, 0).into_string(),
            render_view(&all_one, &links, ViewMode::All, 0).into_string(),
            "gaps and show-all must render to different bytes on a non-clean scenario",
        );

        // Pick the first flagged leaf the scenario exposes, mark it, then undo.
        // After undo the gaps view must match the pre-mark gaps view byte-for-byte.
        let (root_idx, rel) = first_flagged(&gaps_one).expect("scenario has at least one gap");
        let applied = state
            .store
            .write_mark(root_idx, &rel, Marker::NoEbook)
            .await
            .expect("mark succeeds");
        assert!(applied.created, "the picked leaf was not already marked");
        let after_mark = package_view(&applied.raw, ViewMode::GapsOnly);
        assert_ne!(
            render_view(&gaps_one, &links, ViewMode::GapsOnly, 0).into_string(),
            render_view(&after_mark, &links, ViewMode::GapsOnly, 0).into_string(),
            "the mark must change the gaps view",
        );

        let restored_raw = state
            .store
            .remove_mark(root_idx, &rel, Marker::NoEbook)
            .await
            .expect("unmark succeeds");
        let restored = package_view(&restored_raw, ViewMode::GapsOnly);
        assert_eq!(
            render_view(&gaps_one, &links, ViewMode::GapsOnly, 0).into_string(),
            render_view(&restored, &links, ViewMode::GapsOnly, 0).into_string(),
            "undoing the mark must restore the gaps view byte-for-byte",
        );

        /// Walk the rendered gaps view for the first `(root, rel)` whose state
        /// names a flagged leaf. Returns `None` if every root is clean.
        fn first_flagged(view: &FlaggedView) -> Option<(usize, String)> {
            fn first_leaf(node: &Node) -> Option<String> {
                if node.directly_holds_audio && node.missing_ebook {
                    return Some(node.rel_path.clone());
                }
                node.children.iter().find_map(first_leaf)
            }
            for (idx, section) in view.iter().enumerate() {
                if let RootState::Forest(nodes) = &section.state
                    && let Some(rel) = nodes.iter().find_map(first_leaf)
                {
                    return Some((idx, rel));
                }
            }
            None
        }
    }

    #[test]
    fn container_and_leaf_each_emit_their_own_actions_group() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Gap", "Author/Gap", &["01.mp3"])],
            )]),
            1,
        )];
        for mode in [ViewMode::GapsOnly, ViewMode::All] {
            let html = render_view(&view, &default_links(), mode, 0).into_string();
            assert_eq!(
                html.matches(r#"class="actions-group""#).count(),
                2,
                "container and leaf each carry one actions group in {} view",
                mode.as_query(),
            );
        }
    }

    #[test]
    fn index_wraps_the_flagged_row_label() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The name and the muted notes render as one inline run, so a wrapped name
        // at phone width has nothing sitting beside its box. The badge is not in
        // it: it holds the mark column at the row's right edge instead.
        assert!(html.contains(concat!(
            r#"<span class="row-label">"#,
            r#"<span class="name">Book</span>"#,
            r#"<span class="smell smell-loose">loose at top</span>"#,
            r#"<span class="file-count">1 file</span>"#,
            r#"</span>"#,
        )));
    }

    #[test]
    fn index_wraps_the_covered_row_label() {
        let view = vec![section(
            "/lib",
            forest(vec![covered_leaf("Book", "Book", &["Book.epub"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All, 0).into_string();
        // The covering filenames stay in the run and share the name's line when it
        // has room for them. The check has left the run for the mark slot.
        assert!(html.contains(concat!(
            r#"<span class="row-label">"#,
            r#"<span class="name">Book</span>"#,
            r#"<span class="cover-files" title="covering files">Book.epub</span>"#,
            r#"</span>"#,
        )));
    }

    #[test]
    fn index_gives_each_mark_its_own_slot() {
        let flagged = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&flagged, &[], ViewMode::GapsOnly, 0).into_string();
        // A gap's pill sits in the mark slot, after the spring that pushes it right.
        assert!(html.contains(concat!(
            r#"<span class="spring"></span>"#,
            r#"<span class="row-mark">"#,
            r#"<span class="badge badge-warning" title="needs ebook">needs ebook</span>"#,
            r#"</span>"#,
        )));

        let covered = vec![section(
            "/lib",
            forest(vec![covered_leaf("Book", "Book", &["Book.epub"])]),
            1,
        )];
        let html = render_view(&covered, &[], ViewMode::All, 0).into_string();
        // A covered row's check takes the same slot, tagged so the desktop rule can
        // retire it past the action slot.
        assert!(html.contains(concat!(
            r#"<span class="row-mark row-mark-done">"#,
            r#"<span class="status" title="covered">"#,
        )));

        // Gaps-only renders no checks at all, so the slot only ever holds the pill.
        let html = render_view(&flagged, &[], ViewMode::GapsOnly, 0).into_string();
        assert!(!html.contains("row-mark-done"));
    }

    #[test]
    fn every_row_reserves_an_action_slot() {
        // A covered leaf carries no buttons, but it still gets the slot, so the
        // mark column above it cannot slide sideways from row to row.
        let view = vec![section(
            "/lib",
            forest(vec![
                covered_leaf("Done", "Done", &["Done.epub"]),
                flagged_leaf("Gap", "Gap", &["01.mp3"]),
            ]),
            1,
        )];
        let html = render_view(&view, &default_links(), ViewMode::All, 0).into_string();
        assert_eq!(
            html.matches(r#"class="row-actions""#).count(),
            2,
            "the covered row and the flagged row each carry an action slot",
        );
        // Only the flagged row fills it.
        assert_eq!(html.matches(r#"class="actions-group""#).count(), 1);
    }

    #[test]
    fn marker_buttons_keep_their_labels_in_the_dom() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly, 0).into_string();
        // The buttons render icon-only, but the label is still in the markup: the
        // stylesheet clips it, so each button keeps an accessible name.
        assert!(html.contains(r#"<span class="label-short">No ebook</span>"#));
        assert!(html.contains(r#"<span class="label-short">Elsewhere</span>"#));
        // Square sizing, so the three row buttons read as one set of icons.
        assert_eq!(
            html.matches("btn-outline btn-xs btn-square").count(),
            2,
            "both marker buttons",
        );
    }
}
