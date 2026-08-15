# UI probe — pixel-parity measurement loop

Tools for driving TermLab's UI to pixel parity with the reference
(IntelliJ-based) TermLab. Requires macOS Screen Recording permission for the
terminal running these commands.

Build once:

```bash
cd scripts/ui-probe
swiftc -O winlist.swift -o /tmp/winlist
swiftc -O sample.swift  -o /tmp/sample
swiftc -O scan.swift    -o /tmp/scan
```

## The loop

1. **Find both windows.** Run the reference app (`cd ~/projects/TermLab && make termlab`)
   and ours (`cargo run -p termlab_tauri`), then:

   ```bash
   /tmp/winlist
   ```

   Columns: `windowId  WxH  owner  title`. Ours is owner `termlab`; the
   reference is owner `Main`, title `<user> – Terminal`.

2. **Capture each window** without raising or disturbing it:

   ```bash
   screencapture -x -o -l <windowId> /tmp/shot.png
   ```

3. **Sample exact colors** at points (device pixels; retina = 2x logical):

   ```bash
   /tmp/sample /tmp/shot.png 2240,87 2240,420
   ```

4. **Measure metrics** — scan a column or row and get runs of identical color,
   which gives element heights/widths and border thicknesses:

   ```bash
   /tmp/scan /tmp/shot.png col 2600 40 320   # x=2600, y from 40 to 320
   /tmp/scan /tmp/shot.png row 500 0 2800    # y=500, x from 0 to 2800
   ```

## Reading results

Screen capture drifts colors by ±1 per channel (display profile), so
`#22252A` measured == `#21252b` token. Compare run *lengths* for metrics:
a 54-device-px run is 27 logical px.

Reference captures live in `docs/superpowers/specs/assets/reference/`.
