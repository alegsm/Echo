import { expect, test } from 'bun:test';
import { clientHarness } from './helpers/mount-svelte';
import { compileRenderer, sourcePath } from './helpers/render-svelte';

const root = new URL('../', import.meta.url);
const file = (path: string) => Bun.file(new URL(path, root));

function pngSize(bytes: Buffer) {
  expect(bytes.subarray(0, 8).toString('hex')).toBe('89504e470d0a1a0a');
  expect(bytes.toString('ascii', 12, 16)).toBe('IHDR');
  return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

test.each([
  ['src-tauri/icons/32x32.png', 32],
  ['src-tauri/icons/128x128.png', 128],
  ['src-tauri/icons/128x128@2x.png', 256],
  ['static/favicon.png', 64],
] as const)('%s is a valid %d px PNG', async (path, size) => {
  expect(pngSize(Buffer.from(await file(path).arrayBuffer()))).toEqual([size, size]);
});

test('the bundle keeps its icon list and the supplied ICNS with small representations', async () => {
  const config = await file('src-tauri/tauri.conf.json').json();
  expect(config.bundle.icon).toEqual([
    'icons/32x32.png', 'icons/128x128.png', 'icons/128x128@2x.png', 'icons/icon.icns', 'icons/icon.ico',
  ]);
  const bytes = Buffer.from(await file('src-tauri/icons/icon.icns').arrayBuffer());
  expect(bytes.toString('ascii', 0, 4)).toBe('icns');
  expect(bytes.readUInt32BE(4)).toBe(bytes.length);
  const types: string[] = [];
  let offset = 8;
  while (offset < bytes.length) {
    types.push(bytes.toString('ascii', offset, offset + 4));
    const length = bytes.readUInt32BE(offset + 4);
    expect(length).toBeGreaterThanOrEqual(8);
    offset += length;
  }
  expect(offset).toBe(bytes.length);
  expect(types.sort()).toEqual(['ic04', 'ic05', 'ic07', 'ic08', 'ic09', 'ic10', 'ic11', 'ic12', 'ic13', 'ic14', 'info']);
});

test('the supplied ICO contains six PNG representations from 16 through 256 px', async () => {
  const bytes = Buffer.from(await file('src-tauri/icons/icon.ico').arrayBuffer());
  expect([bytes.readUInt16LE(0), bytes.readUInt16LE(2), bytes.readUInt16LE(4)]).toEqual([0, 1, 6]);
  const sizes = [16, 24, 32, 48, 64, 256];
  sizes.forEach((size, index) => {
    const entry = 6 + index * 16;
    expect([bytes[entry] || 256, bytes[entry + 1] || 256]).toEqual([size, size]);
    const length = bytes.readUInt32LE(entry + 8);
    const offset = bytes.readUInt32LE(entry + 12);
    expect(offset + length).toBeLessThanOrEqual(bytes.length);
    expect(pngSize(bytes.subarray(offset, offset + length))).toEqual([size, size]);
  });
});

test('Windows uses its own full-bleed ICO with the 32 px frame first', async () => {
  const config = await file('src-tauri/tauri.windows.conf.json').json();
  expect(config.bundle.icon).toEqual(['icons/windows/icon.ico']);
  expect(config.bundle.windows.nsis).toMatchObject({
    installerIcon: 'icons/windows/icon.ico', uninstallerIcon: 'icons/windows/icon.ico',
  });
  const bytes = Buffer.from(await file('src-tauri/icons/windows/icon.ico').arrayBuffer());
  const sizes = [32, 16, 20, 24, 40, 48, 64, 256];
  expect([bytes.readUInt16LE(0), bytes.readUInt16LE(2), bytes.readUInt16LE(4)]).toEqual([0, 1, sizes.length]);
  sizes.forEach((size, index) => {
    const entry = 6 + index * 16;
    expect([bytes[entry] || 256, bytes[entry + 1] || 256]).toEqual([size, size]);
    expect([bytes.readUInt16LE(entry + 4), bytes.readUInt16LE(entry + 6)]).toEqual([1, 32]);
    const length = bytes.readUInt32LE(entry + 8);
    const offset = bytes.readUInt32LE(entry + 12);
    expect(offset + length).toBeLessThanOrEqual(bytes.length);
    const png = bytes.subarray(offset, offset + length);
    expect(pngSize(png)).toEqual([size, size]);
    expect([png[24], png[25]]).toEqual([8, 6]);
  });
});

test('the Windows installer wizard uses 24-bit uncompressed BMPs at the NSIS sizes', async () => {
  const config = await file('src-tauri/tauri.windows.conf.json').json();
  expect(config.bundle.windows.nsis).toMatchObject({
    sidebarImage: 'installer/windows/sidebar.bmp',
    headerImage: 'installer/windows/header.bmp',
    uninstallerHeaderImage: 'installer/windows/header.bmp',
  });
  for (const [path, width, height] of [['installer/windows/sidebar.bmp', 164, 314], ['installer/windows/header.bmp', 150, 57]] as const) {
    const bytes = Buffer.from(await file(`src-tauri/${path}`).arrayBuffer());
    expect(bytes.toString('ascii', 0, 2)).toBe('BM');
    expect(bytes.readUInt32LE(2)).toBe(bytes.length);
    const stride = (width * 3 + 3) & ~3;
    // BITMAPINFOHEADER, positive height (bottom-up), one plane, 24 bpp, BI_RGB.
    expect([bytes.readUInt32LE(14), bytes.readInt32LE(18), bytes.readInt32LE(22)]).toEqual([40, width, height]);
    expect([bytes.readUInt16LE(26), bytes.readUInt16LE(28), bytes.readUInt32LE(30)]).toEqual([1, 24, 0]);
    expect(bytes.length - bytes.readUInt32LE(10)).toBe(stride * height);
  }
});

test('two rendered EchoMarks resolve their own glow filters and stay decorative', async () => {
  const render = await compileRenderer(`<script>import Mark from '${sourcePath('src/lib/ui/brand/EchoMark.svelte')}';</script><Mark /><Mark shinePeriodMs={5000} />`);
  const marks = [...render().matchAll(/<svg\b[\s\S]*?<\/svg>/g)].map(([svg]) => svg);
  expect(marks).toHaveLength(2);
  const ids = marks.flatMap(mark => [...mark.matchAll(/<filter\s+id="([^"]+)"/g)].map(([, id]) => id));
  expect(ids.length).toBeGreaterThan(0);
  expect(new Set(ids).size).toBe(ids.length);
  for (const mark of marks) {
    const local = [...mark.matchAll(/<filter\s+id="([^"]+)"/g)].map(([, id]) => id);
    const references = [...mark.matchAll(/filter="url\(#([^)]*)\)"/g)].map(([, id]) => id);
    expect(references.length).toBeGreaterThan(0);
    expect(references.every(id => local.includes(id))).toBe(true);
    expect(mark).toContain('aria-hidden="true"');
  }
});

// Keep: intact component effects update SVG attributes and release timers/listeners.
test('the opt-in mark shines on schedule and releases its animation on unmount', async () => {
  const mark = await clientHarness(new URL('../src/lib/ui/brand/EchoMark.svelte', import.meta.url));
  try {
    mark.mount({ shinePeriodMs: 5000 });
    const read = () => Number(mark.target.querySelector('g[stroke-dashoffset]')!.getAttribute('stroke-dashoffset'));
    expect(read()).toBe(-523.293);
    expect([...mark.timers.values()].map(timer => timer.delay)).toEqual([5000]);
    mark.elapse(5000); mark.frame(5000);
    expect(read()).toBeCloseTo(324.185, 3);
    mark.frame(5700);
    expect(read()).toBeLessThan((324.185 - 523.293) / 2);
    expect(read()).toBeGreaterThan(-523.293);
    mark.frame(6400);
    expect(read()).toBe(-523.293);
    expect([...mark.timers.values()].map(timer => timer.delay)).toEqual([3600]);
    mark.elapse(10000); mark.frame(10000);
    expect(read()).toBeCloseTo(324.185, 3);
    mark.reduced(true);
    expect(read()).toBe(-523.293);
    expect(mark.frames.size).toBe(0); expect(mark.timers.size).toBe(0);
    mark.reduced(false);
    mark.elapse(15000); mark.frame(15000);
    Object.defineProperty(mark.window.document, 'visibilityState', { configurable: true, value: 'hidden' });
    mark.window.document.dispatchEvent(new mark.window.Event('visibilitychange')); mark.flush();
    expect(read()).toBe(-523.293);
    expect(mark.frames.size).toBe(0); expect(mark.timers.size).toBe(0);
    Object.defineProperty(mark.window.document, 'visibilityState', { configurable: true, value: 'visible' });
    mark.window.document.dispatchEvent(new mark.window.Event('visibilitychange')); mark.flush();
    expect([...mark.timers.values()].map(timer => timer.delay)).toEqual([5000]);
    await mark.unmount();
    expect(mark.frames.size).toBe(0); expect(mark.timers.size).toBe(0); expect(mark.listeners.size).toBe(0);
  } finally { await mark.cleanup(); }
});

test.each(['reduced', 'hidden', 'default'])('the mark stays still for %s', async mode => {
  const mark = await clientHarness(new URL('../src/lib/ui/brand/EchoMark.svelte', import.meta.url));
  try {
    if (mode === 'reduced') mark.reduced(true);
    if (mode === 'hidden') Object.defineProperty(mark.window.document, 'visibilityState', { value: 'hidden' });
    mark.mount(mode === 'default' ? {} : { shinePeriodMs: 5000 });
    expect(mark.frames.size).toBe(0); expect(mark.timers.size).toBe(0);
  } finally { await mark.cleanup(); }
});
