# Echo decay mark

The decay line is Echo's logo. Its tone is `#bdeeff` on the `#0a0d13`
tile, with a frozen `#ebfaff` light and white and tone glows.

- At 64 px and above, use the master: [icon.svg](icon.svg) is the 1024 px tile;
  [decay.svg](decay.svg) is the clean line. [icon-1024.png](icon-1024.png) is the supplied raster master.
- From 32 to 63 px (the ICO's 32 and 48), use small-a: remove the final tiny swing and thicken the line
  to 4.8 source units. [decay-small-a.svg](decay-small-a.svg) records the clean curve.
- At 16 px, use small-b: two wider swings and a 6.4-unit line.
  [decay-small-b.svg](decay-small-b.svg) records the clean curve. The ICO also uses this cut at 24 px.

The clean curve SVGs retain the master's 3.6-unit stroke. The approved raster
cuts in [32x32.png](../../src-tauri/icons/32x32.png),
[icon.icns](../../src-tauri/icons/icon.icns) and [icon.ico](../../src-tauri/icons/icon.ico) contain the
size-specific heavier strokes and omit the subpixel travelling light.

Windows has its own cut in [windows/icon.ico](../../src-tauri/icons/windows/icon.ico),
set by [tauri.windows.conf.json](../../src-tauri/tauri.windows.conf.json). Windows icons fill
their canvas, so the tile runs edge to edge with a corner radius of about 14% of the side,
rounded to whole pixels (2 px at 16, 4 px at 32, 36 px at 256). The line is scaled about 1.65×
about the centre, so with its caps it spans about 90% of the tile (86% at 16 px, to keep a
clear pixel at each end) and reads at the size of neighbouring taskbar icons. Its axis sits
about 3 units low so the decaying swings look centred. 64 and 256 px use the master with
the frozen light and glows. 48 px uses small-a with the light and glows. 32 and 40 px use small-a,
and 16, 20 and 24 px use small-b. These smaller cuts have the full-tone line and no glow or light,
and each is nudged by a fraction of a pixel to sit on the grid. Each frame is rendered from
vector at its own size and stored as a 32-bit PNG in the order 32, 16, 20, 24, 40, 48, 64,
256. The 32 px frame comes first because Tauri uses the ICO's first frame as the window icon.
The macOS-style [icon.ico](../../src-tauri/icons/icon.ico) stays as the shared bundle icon.

The Windows installer wizard has two images, set by the same config and kept out of `icons/`.
[sidebar.bmp](../../src-tauri/installer/windows/sidebar.bmp) is the 164×314 px Welcome and Finish
panel: the `#0a0d13` tile colour edge to edge, the master line with its frozen light and glows
in the upper middle, and the word "Echo" below in Inter Medium, `#bdeeff` at 70% opacity.
[header.bmp](../../src-tauri/installer/windows/header.bmp) is the 150×57 px header for installer
and uninstaller inner pages: white, with the 40 px Windows tile cut right-aligned 8 px from the
edge, so it sits beside the page title on the white header strip. Both are rendered from the
brand vectors at their final size and stored as uncompressed 24-bit bottom-up BMPs with no
alpha; NSIS's Modern UI misdraws 32-bit or compressed bitmaps.

[echo-mark-static.svg](echo-mark-static.svg) is the line without a tile and with the light frozen.
The sidebar's [EchoMark.svelte](../../src/lib/ui/brand/EchoMark.svelte) preserves
that art using SVG drop-shadow filters with instance-specific IDs. Its
`shinePeriodMs` defaults to zero for a still mark. The dashboard opts into a
5000 ms period with one 1400 ms ease-out shine pass, then holds the original
rest pose. Reduced motion and hidden documents stop the pass and restore
that pose; returning starts a fresh period. [echo-mark-animated.svg](echo-mark-animated.svg) is retained
for brand reference.

The source art and approved raster master live in this directory. Preserve
the supplied raster files in `src-tauri/icons/` when refreshing derivatives;
they contain the size-specific optical cuts described above.

To generate candidate platform derivatives, run from the repository root:

```sh
out="$(mktemp -d)"
bun run tauri icon design/brand/icon-1024.png --output "$out"
echo "$out"
```

Review the candidates in the temporary directory against the approved art.
The command produces generic derivatives, not the approved optical cuts.
Keep only the desktop icons listed in [tauri.conf.json](../../src-tauri/tauri.conf.json):
`32x32.png`, [128x128.png](../../src-tauri/icons/128x128.png),
[128x128@2x.png](../../src-tauri/icons/128x128@2x.png), `icon.icns` and `icon.ico`. Preserve their approved optical
cuts and the Windows `windows/icon.ico`; the icon command does not reproduce them. Keep
`src-tauri/icons/icon.svg` equal to `design/brand/icon.svg` and preserve the
supplied 64 px `static/favicon.png`.
