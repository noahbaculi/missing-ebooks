# Show-all is a second scan mode with its own cache slot

Show-all renders the full library tree, covered folders included. It is backed by
a second scan mode (`scanner::scan_all` and `tree::build_all`) with its own cache
slot, not by filtering one shared full walk at render time. Gaps-only stays the
default landing view and the cheap common path; the full walk is built only when a
viewer first toggles to show-all.

We considered one full walk feeding both views through a render filter. We rejected
it so the common path keeps paying nothing for the fuller walk, which matters on a
large networked library where the walk costs seconds. Gaps-only prunes the moment
it hits coverage, so it stays as cheap as it is today. The cost of two modes is a
second cache slot and a second walk for the viewer who asks for show-all.

The two slots live behind one mutex, so the ADR-0002 serialization still holds: a
marker write and a TTL rescan cannot interleave. A marker write edits both warm
slots in place under that one lock. In the gaps-only slot the marked subtree is
removed and emptied containers prune (today's behavior). In the all slot the marked
folder and its descendants flip to covered and stay visible. A cold slot is left
cold; it rebuilds correctly on next access because the marker is already on disk.

Covered rows are minimal: a dimmed name with a check, no buttons and no links. The
scanner does not track why a folder is covered, so there is no "ebook here" versus
"covered above" annotation. Marker buttons and search links appear on a row only
when there is a gap at or below it, which suppresses them on covered and fully
covered branches; coverage is monotonic, so a covered container can have no gap
beneath it.

The model change that enables this: `tree::Node` carried one `flagged: bool`, which
cannot express a covered folder. It now carries two facts, `directly_holds_audio`
and `missing_ebook`, and the gap is the derived `needs_ebook()`. Gaps-only output
is unchanged, because there `needs_ebook()` reproduces the old `flagged` value.
