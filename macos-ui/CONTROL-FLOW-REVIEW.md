# Control-flow review — 2026-09-19

An independent UI designer agent reviewed the current source and before/after
screenshots in two passes. The retained direction is compact native macOS UI:
choose a computer, add an intentional rule, review the draft, then save.

Implemented from the review:

- Open Keys & Buttons directly; remove the retired MMF profile landing screen
  and pairing hooks that could re-enable sync after uninstall.
- Show the discovered computer name and explicit sending direction.
- Add rules in a native sheet with empty source/output choices, type-specific
  choices, duplicate-source exclusion and identical-choice validation.
- Keep the draft attached to one computer. Ordinary page navigation preserves
  it; an explicit switch requests confirmation before discarding unsaved work.
- Derive dirty state from actual differences; freeze editing during a save,
  retain failed drafts, offer retry, and reconcile late confirmations.
- Match menu destination order and names to the sidebar.

Validation: Swift model tests and optimized app build pass. Actual installed
window and add-rule sheet were visually inspected; empty-choice submission was
disabled and cancel returned to the unchanged editor. The second design review
approved the layout and identified the navigation/late-save cases fixed above.
No live key/button mappings were changed during this review.

Outside this pass: pairing's test screen still reads global selected-peer
liveness; it should eventually track the specific pairing target. Existing
network settings and sharing processes were not changed by the UI update.
