# Light editor — manual verification checklist

The editor branch (merged 2026-08-17) was verified by harnesses over the real
modules, but **nothing ran in the real GUI and no remote path touched a real
SSH host**. This is the human pass the final review asked for. Run in order;
stop and report at the first mismatch.

Special checks first — the two things no harness could see:

- [ ] **⌘Q quits.** The predefined Quit menu item was replaced with a custom
  one (so unsaved editors can be checked); if it mis-renders or the
  accelerator fails to bind, quit is broken for everyone.
- [ ] **"Restart Now" relaunches; ⌘Q does not.** The updater's restart is now
  polled through the same close guard; its Quit/Restart arm mapping is
  untestable in unit tests.
- [ ] **The bundle loads in the real WKWebView**: open a scratch (⌘N) and
  confirm highlighting renders. Separately, a `cargo run` build (no
  beforeBuildCommand) should show the "editor bundle missing" toast rather
  than doing nothing. `[superseded — see section G: ⌘N now opens an untitled
  buffer, not a scratch file; the highlighting check still applies]`

## A. Local pane (no SSH host needed)

1. Files panel, local pane: double-click a `.js` or `.py` file → one editor
   tab, correct syntax highlighting.
2. Keyboard-select a text file, press **Enter** → same. (New behaviour from
   the activateEntry wiring.)
3. Type into it, ⌘S, `cat` the file in a terminal → the edit is there.
4. Double-click the same file again → the existing tab focuses; no second
   tab; an unsaved edit survives.
5. Double-click a `.png` → "Cannot Open File" toast, no tab.
6. Double-click a file over 5 MB (`mkfile 6m /tmp/big.txt`) → rejection toast
   naming the size, no tab.
7. Double-click a directory → navigates. Enter on a directory → navigates.

## B. Remote pane, happy path

8. Connect to a host, double-click a small remote text file → downloads and
   opens. Expect the accepted cosmetic noise (explorer progress toast,
   duplicate "Transfer Complete", a briefly-marked same-named local row).
9. Edit, ⌘S → "Uploaded" toast reading `user@host:/path`; `cat` on the host
   confirms the change landed.
10. ⌘S again with no edits → still uploads cleanly.
11. Double-click the same remote file again → existing tab focuses; no second
    download.
12. Second host, same file path → a separate tab; edits do not cross.

## C. Remote rejections (watch that no transfer starts)

13. Remote file over 5 MB → rejection AND no progress bar ever appears.
14. Remote `.jar` → same.
15. Binary file named `x.txt` on the host → downloads, then rejects; confirm
    no temp file remains:
    `find "$(node -e 'console.log(require("os").tmpdir())')/termlab-sftp-edits" -type f`

## D. The failure cases that matter most

16. Open a remote file, edit, kill the SSH session, ⌘S → "Upload Failed" and
    "Save Failed" toasts; tab still dirty; temp file still exists; ⌘W on the
    tab PROMPTS rather than closing.
17. Restore the connection, ⌘S → uploads, tab goes clean, host has the edit.
18. **Known limitation, confirm the diagnosis:** double-click a large-ish
    remote file and double-click it again mid-download in a SECOND WINDOW →
    both windows get a tab on one temp path; closing one breaks the other's
    save ("No such file or directory"). Same-window double-clicks are guarded;
    cross-window is documented as Known Limitation.
19. Close a remote editor tab cleanly → its temp file and emptied parent dirs
    are gone (find above returns nothing for that host).
20. **Split-pane leak (deferred minor):** remote editor focused, ⌘D to split,
    ⌘⇧W to close the editor pane → its temp file is expected to REMAIN until
    quit (known deferral, confirm it is the only residue).
21. Two dirty remote editors on a dead host, ⌘Q → up to ~60s per pane of no
    feedback, then "Save Failed" and the quit is refused; both temp files
    intact.
22. Everything saved and closed, quit → `termlab-sftp-edits` is gone entirely.

## E. Editor within the app

23. Open Settings from a focused editor, close it, type → keystrokes land in
    the editor (DOM focus, not just the border).
24. Command palette open/dismiss from an editor → same.
25. Change theme and font size in Settings → the editor follows live.
26. ⌘D beside an editor → terminal and editor side by side, both work; drag
    the editor tab to reorder.
27. Modify a scratch, then: close tab / close window / ⌘Q — each prompts
    Save / Don't Save / Cancel; Cancel aborts and the pane stays dirty.
    `[superseded — see section G: this is now an untitled buffer; the
    prompt/Cancel behaviour described is unchanged]`
28. ⌘S in a plain terminal pane → nothing is consumed; the shell receives it.

## F. Editor polish branch — terminal font, host labels, the file dialog, vim

Added by the editor-polish plan (four features: editor font, host-labelled
remote tabs, the unified Open/Save As dialog, optional vim keybindings).
Nothing below was run in a GUI by the implementer; every step is a human
pass. **Steps marked [SSH] need a real SSH host**; the rest need only a local
build. Run in order within each block; stop and report at the first mismatch.

### F1. The editor uses the terminal font

29. Settings → Terminal: note the terminal font family and size. Open a
    scratch (⌘N) → the editor text is that same monospace family at that
    size, not the UI font. `[superseded — see section G: ⌘N opens an
    untitled buffer now, not a scratch file]`
30. With the editor still open, change the terminal font family in Settings
    and Apply → the OPEN editor pane re-renders in the new family live (no
    reopen). Change the size → same.
31. Set the terminal font to a family that does not exist on this machine →
    the editor falls back to a monospace stack, never to a proportional UI
    font.

### F2. Remote tabs name their host

32. [SSH] Open a remote file → the tab reads `name — user@host` (and
    `user@host:port` if the port is not 22). Hover it → the tooltip is the
    full `user@host:/absolute/remote/path`.
33. Open a local file → the tab is the bare basename; the tooltip is the
    absolute local path.
34. [SSH] The same filename open from two different hosts → two tabs, each
    naming its own host; edits do not cross.
35. Edit a remote file → the dirty dot appears AFTER the label and does not
    disturb the host suffix.

### F3. Vim keybindings

36. Settings → Editor → "Vim keybindings" ON with an editor already open →
    the open pane enters vim mode live (Escape then `i` toggles insert), and
    the document, cursor position and undo history all survive the switch.
37. `dd`, `p`, `u`, `/pattern` behave as vim; with the setting OFF again,
    typing `i` inserts the letter `i` and nothing else changed.
38. `:w` on a local file → saves; `cat` confirms. [SSH] `:w` on a remote file
    → the "Uploaded" toast, and the host has the bytes.
39. `:q` on a DIRTY pane → the app's Save / Don't Save / Cancel prompt, not a
    vim error. Cancel leaves the tab open and still dirty.
40. `:wq` on a dirty pane → saves, then closes. With the host down (see F4),
    `:wq` must NOT close the tab.
41. **Escape interplay:** with vim ON and the editor focused, Escape returns
    to normal mode and does NOT close a dialog or leave zen mode. With the
    editor NOT focused, Escape still reaches the app (open a dialog from a
    terminal pane and dismiss it with Escape).
42. **Note, not a defect:** `:q!` and `:wq!` are not special-cased — the
    force variants still route through the app's unsaved-changes prompt, so a
    dirty `:q!` asks rather than discarding. Confirm it asks.

### F4. The file dialog — Open (⌘O)

43. ⌘O from a terminal pane (no editor open) → the chooser appears. The scope
    bar reads `This Mac` first, then one button per CONNECTED host.
44. Browse: double-click and Enter descend; the ↑ button and the breadcrumbs
    go back up; the filter box narrows; the Hidden checkbox reveals dotfiles.
45. Paste `/etc/hosts` into the path field and press Enter → the file opens
    (not just its directory). Paste a directory → it navigates there.
46. Select a directory → the Open button stays DISABLED (Enter descends
    instead). Select a file → it enables.
47. Pick a `.png` → the "Cannot Open File" toast from the editor service; no
    tab. Cancel / Escape / a backdrop click → no tab, no toast.
48. [SSH] Switch to a host scope → it starts in that session's home directory
    (resolved server-side). Open a remote file → the tab carries the host
    label from F2.
49. [SSH] Disconnect a host while browsing it → an inline error appears where
    the list was, INSIDE the dialog, and the other scope buttons still work.
50. [SSH] Two panes connected to the SAME host → two scope buttons, each
    suffixed `(pane N)`. Opening from either produces ONE tab per file.

### F5. Save As (⌘⇧S) — the risky one

51. ⌘⇧S with a terminal pane focused → nothing happens and the SHELL receives
    the keystroke (the binding is editor-scoped). File → Save File As… is
    likewise inert with a terminal focused.
52. Scratch → ⌘⇧S → the chooser opens in save mode: title "Save File As", a
    filename field pre-filled with the current basename, a New Folder button,
    and a primary button reading Save. `[superseded — see section G: there
    is no pre-existing "Scratch" pane any more; substitute an untitled
    buffer, prefilled from its Untitled tab label]`
53. Save the scratch to a new LOCAL path → the tab renames to the new
    basename, the tooltip becomes the new absolute path, the dirty dot
    clears, and the file exists on disk with the right bytes. Syntax
    highlighting switches to the new extension (save a scratch as `x.py` and
    watch Python highlighting appear).
54. ⌘S afterwards → writes the NEW file. The original scratch is still on
    disk, unchanged since its last save. `[superseded — see section G: an
    untitled buffer has nothing on disk before this first save, so "the
    original scratch is still on disk" no longer applies]`
55. Save As onto an EXISTING file → the "Overwrite File?" prompt appears
    stacked over the chooser. Cancel → nothing is written, nothing rebinds,
    and the chooser is still open at the same directory with the same typed
    name. Overwrite → it writes.
56. New Folder (local) → prompts for a name, creates it, and the listing
    refreshes with the folder in it. Cancel at the prompt creates nothing.
    An invalid name (e.g. one in a read-only directory) shows the error
    INLINE in the dialog, not as a toast.
57. [SSH] ⌘⇧S from a scratch → a host scope → type a filename in a directory
    path that does NOT exist yet, creating it with New Folder first → Save.
    Expect: the file appears on the host, the tab becomes
    `name — user@host`, and the tooltip is the remote path.
    `[superseded — see section G: substitute an untitled buffer for
    "a scratch"]`
58. [SSH] After that rebind, ⌘S (and `:w` with vim on) uploads to the NEW
    host and path — verify on the host, and verify the OLD location is not
    written again.
59. [SSH] **The one that must not half-rebind:** open a remote file, edit it,
    kill the SSH connection, then ⌘⇧S to that same (now dead) host. Expect a
    "Save As Failed" toast AND: the tab still shows the OLD name and host,
    the tooltip is still the OLD remote path, the dirty dot is still there,
    and ⌘W still PROMPTS. Reconnect and ⌘S → the edit uploads to the OLD
    location.
60. [SSH] New Folder on a host scope → `sftp_mkdir` creates it and the
    listing refreshes; the same folder is visible from the files panel.
61. [SSH] Save As from a remote file to a LOCAL path → the tab loses the host
    suffix entirely, and the remote temp file for the old binding is gone
    (`find "$(node -e 'console.log(require("os").tmpdir())')/termlab-sftp-edits" -type f`).
62. Save As onto a path that is ALREADY OPEN in another editor tab → refused
    with a message naming the file; nothing is written over it.
63. Settings → Keymap → Editor lists New Scratch, Open File, Save File and
    Save File As. Rebind Save File As, Apply, and confirm the new combo works
    and the old one does not. `[superseded — see section G: the list now
    reads "New File", not "New Scratch"]`

## G. Untitled files (replaces scratches)

Added by the untitled-files plan: New File opens an in-memory `Untitled`
buffer instead of a real file under `<config_dir>/scratches/`; every first
save — whatever triggers it — routes through the Save As chooser. Nothing
below was run in a GUI by the implementer; every step is a human pass.
**Steps marked [SSH] need a real SSH host**; the rest need only a local
build. Run in order; stop and report at the first mismatch.

64. **New File from all three entry points** — ⌘N, File → New File, and the
    command palette — each opens a tab labelled `Untitled`. `pane.filePath`
    is null, nothing exists on disk, and the tab is not dirty.
65. Type into it, then ⌘S → the Save As chooser opens (not a plain write).
    Save to a LOCAL path → the tab renames to the new basename, the tooltip
    becomes the absolute path, the dirty dot clears, and the bytes are on
    disk.
66. [SSH] Repeat 65 but save straight to a CONNECTED host from the chooser's
    scope bar → the tab becomes `name — user@host`, the tooltip is the
    remote path, and the host has the bytes.
67. `:w` (vim keybindings on) on an untitled pane → the same Save As chooser
    opens; it is not a silent write. Completing it rebinds the pane exactly
    as ⌘S does.
68. `:wq` on an untitled pane, CANCELLED at the dialog → the tab stays open
    and still dirty; no error toast; nothing closes.
69. Type into a new untitled pane, then close its tab (or the window, or
    ⌘Q) → the Save/Don't Save/Cancel prompt appears; choosing Save opens the
    Save As chooser; cancelling THAT chooser aborts the close — the tab
    stays open, still untitled, still dirty, and no error toast appears.
70. New File, type nothing, close the tab immediately → it closes silently,
    no prompt at all (an untouched untitled behaves like an untouched titled
    file).
71. Open New File three times in the same window → tabs read `Untitled`,
    `Untitled-2`, `Untitled-3` in creation order.
72. Add `new_scratch = "cmd+shift+u"` to the keymap config (the pre-rename
    field name) → Apply/restart and confirm the combo still binds New File
    (the `#[serde(alias = "new_scratch")]` back-compat path), and Settings →
    Keymap → Editor shows it against "New File", not "New Scratch".
73. Open an untitled pane, type, press ⌘S, and WHILE the chooser is still on
    screen press ⌘S again → only one chooser is visible (the second
    keystroke joins the first in flight rather than opening a second
    dialog), and completing it produces exactly one write and one rebind.
74. Open TWO untitled panes (A and B). Trigger A's Save As chooser (⌘S) and,
    while it is still open, switch to B and press ⌘S → B's save is silently
    refused (no dialog, no toast, no write) rather than sharing A's chooser
    or queuing behind it. Answer A's chooser → A saves normally. Then press
    ⌘S on B by itself → B's own chooser opens and B saves fine. **Note:**
    this refusal is silent by design (see Task 2's fix-round notes); a
    follow-up to surface some feedback for pane B is logged separately, not
    part of this plan.

## H. File chooser redesign (sidebar + detail columns)

Added by the file-dialog-redesign plan: the scope pill row became a Places /
Hosts sidebar, the listing grew Name / Size / Modified columns with
click-to-sort headers and file/folder icons, and the Hidden toggle plus the
save-mode controls moved into the dialog footer. Task 3 rewrote the
stylesheet; **no part of the redesign was ever rendered in a GUI by the
implementer** — every step below is a human pass, and the row-height,
column-width and sticky-header judgments are explicitly yours to make.
**Steps marked [SSH] need a real SSH host**; the rest need only a local
build. Run in order; stop and report at the first mismatch.

75. ⌘O with no SSH session connected → the sidebar shows a `PLACES` caption
    over a single `This Mac` row, and **no `HOSTS` caption at all** (an empty
    heading would read as a broken feature). The row is marked active, the
    listing to its right is your home directory, and the sidebar reads as a
    recessed panel against the lighter listing box beside it.
76. [SSH] Connect one host, then ⌘O → a `HOSTS` caption appears under Places
    with that session's row, a server glyph, and a small round dot at the
    trailing edge. Disconnect it, reopen the chooser → the row and the whole
    `HOSTS` section are gone again.
77. [SSH] Open two panes onto the SAME host, then ⌘O → both rows read
    `user@host:port (pane N)` with different N, and neither is truncated
    into unreadability — a label too long for 150px must ellipsize, not push
    the connected dot out of the row.
78. [SSH] Click a host row → the listing switches to that host's home
    directory and the active marking moves; click it again → nothing
    happens. Now make a host's start FAIL (connect, then drop the connection
    at the far end before clicking) → the inline error appears, the row
    stays active, and **clicking that same row again retries** rather than
    doing nothing.
79. Click the `Size` header → the arrow moves to that column, it brightens
    against the other two, and the listing reorders by size. Click it again
    → the arrow flips and the order reverses. Click `Modified`, then `Name`
    → same behaviour each time. Directories stay above files in every one of
    the six states.
80. With a non-default sort showing (say Size descending), Cancel the dialog
    and reopen it → the sort is back to **Name ascending**. Sort choice is
    per-open by design; a chooser that remembered it would be a bug here.
81. Select a file, then click a header to re-sort → the SAME file is still
    selected (the highlight follows the entry, not the row number). In save
    mode, re-sorting must not overwrite a filename you typed after clicking
    that row.
82. Read a directory row: size reads `—`, never a byte count. Read a file
    row saved today → `Today`; yesterday → `Yesterday`; earlier this year →
    `Jan 5` style, **with no leading zero on the day**; last year →
    `YYYY-MM-DD`. Scan the Modified column down the listing: the digits
    should line up in a straight edge (tabular numerals), and the Size
    column's numbers should be flush right.
83. Navigate to a directory holding a file with a very long name (e.g.
    `touch /tmp/h/$(printf 'a%.0s' {1..120}).txt`) → the name truncates with
    an ellipsis inside its column; the Size and Modified columns do not move
    and the row does not grow a second line or a horizontal scrollbar.
84. Toggle **Hidden** in the footer → dotfiles appear and disappear, the
    selection clears, and the toggle sits on the LEFT of the footer opposite
    Cancel / Open.
85. Navigate deep enough that the breadcrumb strip overflows (10+ segments)
    → the strip scrolls horizontally on its own; the path bar stays ONE row
    at the `lg` dialog width, the path field is still wide enough to read a
    real path in, and the filter field still shows its magnifier glyph
    inside the field. Clicking any crumb navigates to it; the last crumb is
    plain text, not a link.
86. Type a directory into the path field and press Enter → it jumps there
    and the crumbs follow. Type a full FILE path and press Enter → the file
    opens (in save mode, its name lands in the filename field). Type
    nonsense → the parent listing plus an inline `No such file or directory`
    message in the body, not a toast.
87. Click into the listing and drive it from the keyboard only: ↓ ↑ Home End
    move the selection and scroll it into view, Enter descends into a
    directory, Enter on a file opens it, Escape closes. While the listing
    has focus, the box around it should show a focus ring — and Tab should
    reach the three column headers and fire a sort with Space/Enter.
88. ⌘⇧S from an untitled buffer → the footer's left side carries
    `Save As:`, the filename field (pre-filled and SELECTED), and
    `New Folder`, with Cancel / Save on the right. The field is wide enough
    to read a real filename in without the footer wrapping.
89. `New Folder` in save mode → the nested prompt stacks over the chooser,
    its single field fills the small dialog, and creating the folder
    refreshes the listing underneath with the new directory in it. Cancel
    leaves you exactly where you were.
90. **Both themes.** Repeat 75, 79, 82, 84 and 88 with the appearance set to
    light and again set to dark. In each: the sidebar is distinguishable
    from both the dialog panel and the listing box; the active sidebar row
    and the selected listing row are legible (foreground against the
    selection fill, including the connected dot and the dimmed Size/Modified
    text); the sticky column header stays opaque with no seam or bleed-
    through as rows scroll under it; and focus rings are visible on the
    sidebar rows, the column headers, the up button, the crumbs and both
    fields. Also check the row glyphs and the filter icon — `file.svg` and
    `search.svg` — for legibility against the row/selection fill in each
    appearance: neither has a `_dark` variant in the vendored set, so the
    same asset is served to both, and these are the first time either glyph
    has shipped in this app. **Known:** the light theme inherits several
    dark token values app-wide (see the spec's Known limitations) — report
    anything that looks
    wrong, but a light-theme border or muted text that reads dark is a
    token-pipeline gap, not a regression from this branch.
91. **The judgment call:** with a listing of 30+ mixed files and folders on
    screen, decide whether the 24px row height, the 150px sidebar, the 80px
    Size column and the 110px Modified column read as comfortable or as
    cramped, in both themes. Say which, with a number, if any should change.

## I. The chooser as an independent window

Added by the chooser-window plan: the unified Open/Save As dialog moved out
of the in-app `tl-dialog` overlay into its own OS window (`chooser.html`),
window-modal over the requesting TermLab window, IntelliJ-save-dialog-style.
Nothing below was run in a GUI by the implementer; every step is a human
pass. **Steps marked [SSH] need a real SSH host**; the rest need only a
local build. Run in order; stop and report at the first mismatch.

92. ⌘O (or ⌘⇧S) from a terminal pane → a separate OS window appears, titled
    "Open" or "Save As" respectively, centered on the PARENT window (not the
    screen), with its own titlebar and traffic lights — not an overlay
    inside the parent.
93. With the chooser open, click into the parent window's body and try to
    type → the parent grays out under a scrim; neither the click nor the
    keystrokes reach it (no tab switch, no menu action, no terminal input).
    The chooser window itself is unaffected and keeps working normally.
94. Click the parent window (its titlebar or its grayed-out body) → the
    chooser comes back to the front instead of the parent taking focus.
95. Press ⌘O again while a chooser is already open for that window → no
    second window opens; the existing chooser is re-fronted instead. Press
    ⌘O while a SAVE-mode chooser is open → nothing happens at all — no
    re-front, no toast, no second window. The cross-mode refusal is silent
    by design; confirm it stays silent.
96. **Escape while the PARENT window has focus** (focus it via its titlebar,
    not its inert body, so the click doesn't route through the scrim) → the
    chooser stays open. Escape only resolves the chooser when the chooser
    window itself has focus — window-modal semantics, previously untested
    behavior; don't assume it from the code.
97. Drag the chooser window's edge smaller → it stops shrinking at a
    content-fit floor (720×420 logical px). At that floor, nothing but the
    file listing has a scrollbar — the sidebar, path bar, column header and
    footer stay fully visible and unclipped all the way down to it.
98. Resize the chooser larger, then close it (Cancel or complete a pick) and
    reopen (⌘O again) → it reopens at the SAME size, not back at the floor.
    Quit and relaunch the app, reopen once more → the size is still
    remembered (it persists to disk, not just in-memory for the session).
99. Open two separate TermLab windows, then open a chooser from EACH → two
    independent chooser windows, each graying out only its own parent (the
    other app window stays fully usable). Browse, sort, or resize one →
    the other's scrim, listing, selection and size are untouched.
100. **Displacement — no automated coverage for this GUI path.** With a
     chooser open, open Developer Console (F12) on the PARENT window and run
     `location.reload()` there — this resets the parent's JS state (so its
     own same-mode/cross-mode guards forget the open chooser) while the
     Rust-side registry entry for that parent survives. Trigger ⌘O again
     from the reloaded page. Expect: the OLD chooser window closes, a NEW
     one opens with the correct mode and title, and the parent's scrim
     stays continuous across the swap (no flash of the parent's real
     content in between). **If this path isn't reachable this way, say so
     and note it as race-only** — the actual IPC race the spec describes
     isn't reproducible on demand from the UI, and the Rust registry's own
     unit tests are the only coverage that exists for it; do not force a
     result you didn't actually see.
101. [SSH] Open a chooser, switch to a host scope, then kill that session's
     connection mid-browse → an inline error replaces the listing (not a
     toast), the other scope buttons keep working, and clicking the failed
     scope again retries rather than doing nothing.
102. ⌘⇧S onto a filename that already exists → the "Overwrite File?" confirm
     appears STACKED INSIDE the chooser window, not as a separate window and
     not hidden behind it. Cancel returns to the chooser at the same
     directory with the same typed name; confirming writes.
103. **Save-mode focus timing.** Open a Save As chooser with a placeholder
     filename (e.g. ⌘⇧S from an untitled buffer) → within a beat of the
     window appearing, the filename field has focus AND its text is fully
     selected, so the first keystroke replaces it rather than appending.
     This window host adds one more deferred frame on top of the view's own
     (at most one extra frame total beyond what the in-app chooser in
     section H needed). **Flag it if the delay reads as laggy rather than
     instant — that judgment is yours.**
104. With that same save-mode chooser, type a name and press Escape → it
     closes with no save, same as the overlay's save-mode Escape behavior in
     F5/H.
105. Close the PARENT window (its native close button) while its chooser is
     still open → the chooser window closes too, with no leftover orphan
     and no crash. Check the OS window list/dock afterward to confirm
     nothing lingers.
106. **Both themes.** Repeat 92, 93, 97 and 102 with the appearance set to
     light and again to dark: the scrim reads as a translucent scrim, not a
     flat block, in both; the chooser window's own chrome (sidebar, listing,
     footer) stays legible in both; and the overwrite confirm stacked over
     the chooser is legible in both. **Known:** light theme is still the
     app-wide approximation documented in the file-dialog-redesign spec's
     Known limitations (token-pipeline gap) — this branch does not attempt
     to fix it. Report anything chooser-specific that looks wrong, but a
     light-theme border or muted text reading dark is that known gap, not a
     regression from this branch.
107. **The judgment call:** at the resize floor (720×420), decide whether
     the chooser window reads as comfortably small or cramped relative to
     the TermLab window it sits modal over — the same kind of call as step
     91, now about the WINDOW's own proportions rather than the row/column
     metrics inside it. Say which, with a number, if anything should
     change.

## J. TermLab Light — the appearance switch actually works

Added by the TermLab Light plan: `app/core/appearance.js` now owns
`data-tl-appearance`, so Settings → Appearance Mode's Light and System
options genuinely restyle the app for the first time (previously nothing
ever set the attribute, so `tokens-light.css` never activated and every
icon only ever rendered its `_dark` variant, dark or light setting
notwithstanding). Nothing below was run in a GUI by the implementer; every
step is a human pass. Run in order; stop and report at the first mismatch.

108. Settings → Appearance → Appearance Mode: switch to **Light** → the
     titlebar, the tool-strip rail and its tool-window tabs restyle to
     light colors immediately, with no dark panel, border, or icon left
     anywhere in the window.
109. With Light active, open each tool window in turn — **Hosts**,
     **Tunnels**, **Notifications**, and the **SFTP pane** — and confirm
     each restyles fully: panel background, list rows, hover/selected
     states, and any icons inside them read as light-themed, not a dark
     island inside an otherwise light app.
110. Open **Settings** (the in-app modal, ⌘,) with Light active → it
     restyles fully. Separately open the **standalone settings window**
     (if reachable independently of the modal) with Light active → same.
     Scroll through every settings section, not just Appearance, checking
     labels, descriptions, toggles and dividers all read as light.
111. ⌘O and ⌘⇧S with Light active → the **chooser window**, in both Open
     and Save As modes, is fully light: sidebar, listing, column headers,
     footer, and — onto an existing filename — the overwrite-confirm
     stacked over it.
112. Open a real source file with syntax highlighting (e.g. a `.rs` or
     `.ts` file) in the **editor** with Light active → the editor
     background, gutter, and syntax token colors are all light-appropriate;
     no token renders a dark background/foreground pair left over from
     dark mode. **Note (fix round, F2):** the editor deliberately does NOT
     match the terminal pane beside it under Light — the terminal palette
     stays dark on this branch (step 121), and the ruling is that the
     editor is a document surface that follows APP appearance. An editor
     and a terminal side by side will show a light pane next to a dark
     one; that is the intended outcome until the terminal-themes plan's
     `auto` lands, not a defect. Also expect the light syntax palette to
     be flatter than dark's — under Light the accents come from the app's
     own tokens, so keywords, strings, numbers and types share
     `--tl-accent`. Judgment call: say whether that flatness is
     acceptable for now or wants a real light syntax palette as a
     follow-up.
113. Trigger a **toast** (e.g. a save confirmation, an induced error) with
     Light active → it reads legibly light-themed, including any bold
     call-out toast variant.
114. Open a **context menu** (right-click a tab, a file row, a terminal
     pane) with Light active → the menu, its hover state, and any icons in
     it are light-themed.
115. Open the **command palette** with Light active → the overlay, input
     field, and result rows are light-themed, including the
     selected-result highlight.
116. **FG_MUTED contrast — judgment call.** Task 2 flagged the muted-text
     anchor `FG_MUTED` (`#8A94A3`, e.g. `--tl-base-infoForeground` and
     everything aliased to it) at **2.49:1** against the base background —
     below general text-contrast guidance — and carried it forward as
     given rather than fixing it, since it's the brief's own designated
     "muted" anchor and mirrors dark's own low-contrast muted tone. With
     Light active, read three places that use it: a **Settings row
     description** (the small gray text under a setting label), the
     **file-dialog listing's Modified column** (the dimmed secondary
     date text), and a **toast's secondary/detail line**. Decide, at
     normal viewing distance, whether that text is comfortably legible or
     actually hard to read — say which. If it reads as hard to read, flag
     it for a follow-up design pass through the source theme JSON
     (`TermLabLight.theme.json`); do not hand-fix it here.
117. **Icon legibility in Light — the vendored variant swap firing for the
     first time.** `tl-icon.js` resolves a `_dark`-suffixed variant only
     when the dark appearance is active — confirmed non-inverted in Task 1
     by checking the actual SVG fills (`add.svg` fills `#6E6E6E`, dark
     grey, legible on light; `add_dark.svg` fills `#AFB1B3`, light grey,
     legible on dark). Before this branch, `data-tl-appearance` was never
     set at runtime, so every icon in the app has only ever rendered its
     `_dark` variant, in either theme setting. With Light active, walk the
     titlebar, the tool-window tabs, the chooser sidebar/listing icons, and
     the `file.svg`/`search.svg` glyphs (which have no `_dark` variant at
     all, per step 90) — confirm the plain, non-`_dark` variants that now
     render for real are actually legible against light backgrounds, not
     merely present.
118. **System-mode live OS flip, both directions, app open.** Settings →
     Appearance Mode → **System**, with the app open and visible. Flip the
     OS-level appearance (System Settings → Appearance, or the test
     machine's equivalent) from light to dark → the app restyles live, no
     relaunch. Flip back dark → light → same, live, no relaunch.
119. **System-mode flip with a chooser window open.** With Appearance Mode
     still **System** and a chooser window open (⌘O) alongside its parent
     TermLab window, flip the OS appearance → **both windows restyle
     together**, live, in the same flip. The parent and the chooser must
     not visibly disagree on appearance even momentarily longer than a
     repaint.
119a. **The standalone settings window flips ITSELF (fix round, F1/F3).**
     Open the **standalone settings window** with Dark active. Change
     Appearance Mode to **Light** and click **Apply** — the button that
     saves without closing. → The settings window you are looking at goes
     light immediately: background, rows, dividers, **the icons in its
     sidebar**, and (macOS, full decorations) its native title bar. It
     must not stay dark until closed and reopened. Switch back to Dark
     with Apply → same, in reverse.
119b. **A chooser open across an explicit save (fix round, F3).** With a
     chooser window open (⌘O) and Appearance Mode currently Dark, save an
     appearance change to **Light** from somewhere else — the standalone
     settings window, or another main window's settings modal. → The
     already-open chooser restyles too; it does not keep its boot-time
     appearance for the rest of its life.
119c. **Persistent icons across an explicit flip (fix round, F1).** This is
     the one step 108 could not catch by eye alone. With several tool
     windows open and their rails visible, and **without touching any of
     them**, flip Dark → Light from Settings. → Every icon in the
     always-visible tool-strip rail, the tool-window title bars (gear,
     hide), the tab bar, and any open Hosts/Tunnels/Notifications panel
     re-resolves in place. The tell for a failure is a light-grey glyph
     (`#AFB1B3`) on the new light surface — near-invisible rather than
     merely wrong. Flip back → the dark variants return. Then open a menu
     and the command palette to confirm the rebuild-per-open surfaces
     agree with the persistent ones.
119d. **An open editor survives an OS flip (fix round, F2).** Appearance
     Mode → **System**, with a source file open in an editor pane. Flip
     the OS appearance. → The open editor pane itself re-themes in the
     same flip — background, gutter, syntax — without reopening the file.
     Before the fix round only the chrome moved and the editor kept its
     old colours entirely.
120. **Dark spot-check — confirm zero net change.** Switch Appearance Mode
     back to **Dark** → re-check the titlebar, a tool window, the editor,
     and the chooser: everything should look and measure exactly as it did
     before this branch. This is the visual counterpart to Task 4's grep
     sweep, which confirmed `data-tl-appearance` is asserted only by
     `tokens-light.css` (CSS) and `appearance.js`/`tl-icon.js` (JS) — with
     the attribute absent under Dark, nothing else in the app can react to
     it.
121. **Terminal appearance — expected UNCHANGED.** With Appearance Mode set
     to Light (and again to System, with the OS in light mode), open a new
     terminal pane or tab → its colors follow `colors.theme` exactly as
     before this branch — whatever palette that setting already names
     (e.g. still "TermLab Dark," or whatever was configured).
     It does **NOT** switch to "TermLab Light," even though that palette
     now exists as a built-in (Task 3): this branch adds the palette but
     wires nothing to select it — the next plan (`auto` terminal-theme
     wiring) does that. Confirm a terminal pane's background, foreground
     and ANSI colors look identical to a same-session terminal pane opened
     under Dark appearance. This step guards the spec's Goal 4
     (byte-identical dark experience) on the one surface that is
     deliberately NOT wired to appearance yet.
122. **Judgment call-outs — parity-passes-but-taste-fails tokens.** For any
     surface walked above where the light color is technically present
     (parity test passes, the token resolves to a real value) but reads
     wrong to the eye — too washed out, insufficient contrast, a jarring
     hue — note it here with the specific `--tl-*` token name if
     identifiable (inspect the computed style in DevTools) or a precise
     description of where it appears. Fix loops for anything found go
     through the **SOURCE theme JSON**
     (`../TermLab/core/resources/themes/TermLabLight.theme.json`) and a
     regeneration via the extractor, per Task 2's process — never hand-edit
     `tokens-light.css`.

## K. Terminal-only themes — drop-in files, Auto, and the packaged build

Added by the terminal-themes plan: `colors.theme` gains a reserved `"auto"`
default that tracks app appearance via the two built-in TermLab palettes,
plus a `~/.config/termlab/themes/*.toml` drop-in directory and a Settings →
Appearance → "Terminal Theme" row — one combo listing them alongside the two
built-ins, with a fake-terminal preview box below it showing the selected
palette. That row sits directly under Appearance Mode; the built-ins are
TermLab Dark (the app's own default palette) and TermLab Light. None of this
touches the editor or app chrome — that's the whole point of
"terminal-only" — and none of it was run against a real GUI or a real
packaged build by the implementer. Run in order; stop and report at the
first mismatch.

123. Download a real theme file from
     [alacritty-theme](https://github.com/alacritty/alacritty-theme) (any
     one not already in this repo's fixture set is fine — Nord and Gruvbox
     are already vendored, so pick something else, e.g. Tokyo Night or
     Everforest) and drop it into `~/.config/termlab/themes/` unmodified.
     Open Settings → Appearance → Terminal Theme → the new theme appears in
     the combo's dropdown, after the built-ins, by its file stem. Select it →
     the preview box below the row immediately repaints in that file's actual
     colors (not a placeholder, and no visible fetch/flicker), AND the open
     terminal pane re-themes live, no restart, matching the preview.
124. Break that same file (delete a required table, e.g. `[colors.normal]`,
     or introduce a syntax error like an unterminated string) and reopen
     Settings → Appearance → Terminal Theme → the entry stays listed in the
     dropdown (not silently dropped) but is greyed/disabled, its label
     reading the theme name then "— Parse error: " followed by the actual
     parse failure — not a generic message. Clicking it is a no-op: the
     current selection, and the preview box, do not change.
125. Rename a (working) theme file to `auto.toml` in the same directory →
     it enumerates in the dropdown like any other user theme, but greyed and
     unselectable, its label reading `auto — Reserved name, ignored (would
     resolve to Auto). Rename this file to use it.` **Judgment call-out:**
     this is
     deliberate, not a bug — `auto` is a reserved `colors.theme` value
     intercepted before any file lookup runs, so a file by that name can
     never be reached by its own name no matter what. Also break this
     file's TOML while it's still named `auto.toml` → the label's note
     changes to "Reserved name, ignored — also fails to parse.", not the
     ordinary parse-error label from step 124 — the reserved-name
     classification wins over the broken classification.
126. **Bold judgment call-out — a user theme can shadow a built-in, including
     for Auto.** Create `~/.config/termlab/themes/TermLab Dark.toml` with
     colors clearly different from the real built-in (e.g. a bright red
     background) → in the dropdown, that entry reads `TermLab Dark
     (overrides built-in)`, and the built-in "TermLab Dark" is no longer
     offered as a separate option. Select it explicitly → the preview box and
     the terminal both show YOUR red background, not the built-in.
     Then separately set Terminal Theme to **Auto** with Appearance Mode
     **Dark** → the terminal *also* shows your red background, not the
     built-in TermLab Dark palette. This is the existing later-dirs-win
     collision rule applying to Auto's own resolution, not a new special
     case — confirm it's actually true and not merely documented as true.
     Delete the file when done, or later steps will keep seeing red instead
     of the real built-in.
127. **Auto tracks a live OS appearance flip on the terminal itself**, not
     just app chrome (section J only proved chrome). With Terminal Theme
     set to **Auto** and Appearance Mode set to **System**, open a terminal
     pane, note its background, then flip the OS-level appearance (System
     Settings → Appearance) light → dark or back → in the *same* flip and
     without touching the pane: the terminal's background/foreground/ANSI
     colors change together with the titlebar and other chrome, live, no
     reconnect or relaunch. Also try Appearance Mode **Light**/**Dark**
     directly from Settings (not System) → same live re-theme on Apply.
128. **A concrete theme survives appearance flips unchanged.** Set Terminal
     Theme to a named palette (e.g. `gruvbox_dark.toml` — vendored at
     `crates/termlab_core/tests/fixtures/alacritty-themes/gruvbox_dark.toml`,
     copy it into `~/.config/termlab/themes/`, or just pick TermLab Light,
     which is a concrete name even though `auto` also uses it) and
     Appearance Mode to **System**. Flip the OS appearance both directions.
     → app chrome restyles as expected (per section J); the terminal pane's
     colors do not change at all — same background, same ANSI colors,
     pixel-for-pixel, in both OS appearances. This is the decoupling
     product rule (terminal theme sticks regardless of app appearance) and
     it's easy to accidentally break by wiring appearance and terminal
     theme through the same code path.
129. **The terminal-only promise, checked directly.** With a source file
     open in an editor pane and the Settings window open, switch Terminal
     Theme through two or three visibly different palettes (e.g. TermLab
     Dark → Gruvbox → Solarized Light). Confirm nothing outside the terminal
     surface changes: the editor's background/gutter/syntax colors, the
     titlebar, the tool-window rail, Settings' own colors, and the file
     chooser (open one, ⌘O) all stay exactly as app appearance already has
     them. The only things allowed to shift are the terminal canvas itself
     and the seven sanctioned accent variables (`--tl-terminal-bg`, `--red`,
     `--green`, `--yellow`, `--blue`, `--cyan`, `--magenta` — visible as the
     flavor of status dots/indicators elsewhere in the chrome). If anything
     else moves, that's the boundary this whole plan exists to protect.
130. **256-color content renders correctly under an `indexed_colors`
     theme.** Copy
     `crates/termlab_core/tests/fixtures/alacritty-themes/github_dark.toml`
     into `~/.config/termlab/themes/`, select it, and run a 256-color test
     in the pane (e.g.
     `for i in {0..255}; do printf '\e[48;5;%sm  \e[0m' "$i"; [ $((($i+1) % 16)) -eq 0 ] && echo; done`
     or any 256-color test script you have on hand). → the swatches in the
     indices this theme's `colors.indexed_colors[]` overrides show that
     theme's specific colors; every other index falls back to the standard
     256-color table (not black, not the theme's normal/bright 16, and not
     a crash) — the sparse-array/xterm-defaults behavior from the spec's
     Known limitations, seen live rather than through a snapshot test.
131. [SSH] Open a remote pane against a real SSH host with a non-default
     Terminal Theme selected → the remote pane's colors match the local
     pane's (same palette, no per-host override exists or is expected —
     confirms the Non-Goals' "no per-pane or per-host terminal themes" line
     holds in practice, not just on paper).
132. **The packaged-build step — bold as the un-automated ship gate.** None
     of the above proves the built app can find its bundled themes; Task 3b
     verified the resource-bundling config and injection logic without ever
     producing a real bundle. Run `cargo tauri build` for a real packaged
     build (not `cargo tauri dev`), then:
     - Inspect the built `.app`'s `Contents/Resources/themes/` (macOS; the
       platform-equivalent resource dir elsewhere) and confirm it contains
       **both** `TermLab Dark.toml` and `TermLab Light.toml` — not zero, not
       one.
     - Launch the packaged `.app` itself (not from `cargo tauri dev`, not
       from a `target/debug` binary) with a fresh config (no prior
       `colors.theme` entry, or `theme = "auto"` explicitly) and Appearance
       Mode **Dark** → the terminal pane shows the **TermLab Dark** palette.
     - Switch Appearance Mode to **Light** (or set the OS to light with
       Appearance Mode **System**) → the terminal shows **TermLab Light**.
     - **The failure mode to watch for:** the terminal staying on the DARK
       palette after switching to Light. TermLab Dark is byte-identical to
       the hardcoded `ColorScheme::default()` fallback, so a dark terminal
       under a light appearance is exactly what you'd see if the packaged
       binary's resource-dir injection didn't run or found nothing and
       `bundled_themes_dir()` fell through to its dev-only fallback (which
       does not exist inside a real app bundle) — the dark half would look
       fine and only the light half would give it away. Check the Light case
       specifically; do not infer success from the Dark one.
