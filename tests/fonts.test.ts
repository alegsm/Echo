import { expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { readdirSync } from 'node:fs';

const root = new URL('../', import.meta.url);
const file = (path: string) => Bun.file(new URL(path, root));
const fonts = [
  ['Inter', 'Inter-Variable.ttf', '100 900', '0b58fb370093f9a9f4ff785d94405710b79de67c', '29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031'],
  ['JetBrains Mono', 'JetBrainsMono-Variable.ttf', '100 800', '6e4b84c976cadb3c49a40fd9a1c203e4f7fcf2da', '48715a42ec242c21e9f02692891e147d022299a52e48d5e413e1a942193ffeda'],
] as const;

// Keep: the shipped font inventory, provenance and licenses are a bundle contract.
test('only the approved font files ship, with matching source hashes and licenses', async () => {
  expect(readdirSync(new URL('static/fonts/', root)).filter(name => /\.(ttf|otf|woff2?)$/i.test(name)).sort())
    .toEqual(fonts.map(([, name]) => name).sort());
  const sources = await file('static/fonts/SOURCES.md').text();
  for (const [family, name, , commit, hash] of fonts) {
    const row = sources.split('\n').find(line => line.includes(`\`${name}\``));
    expect(row).toContain(`https://github.com/google/fonts/commit/${commit}`);
    expect(row).toContain(hash);
    expect(createHash('sha256').update(await file(`static/fonts/${name}`).bytes()).digest('hex')).toBe(hash);
    const license = await file(`static/fonts/licenses/${family.replaceAll(' ', '')}-OFL.txt`).text();
    expect(license).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(license).toContain(`The ${family} Project Authors`);
  }
});

// Keep: Echo never fetches a font from the network.
test('font faces load only bundled files', async () => {
  const css = await file('src/lib/styles/fonts.css').text();
  expect(css).not.toMatch(/https?:|@import/);
});
