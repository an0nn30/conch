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
  than doing nothing.

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
28. ⌘S in a plain terminal pane → nothing is consumed; the shell receives it.
