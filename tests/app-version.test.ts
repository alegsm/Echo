import { expect, test } from 'bun:test';

const read = (path: string) => Bun.file(new URL(`../${path}`, import.meta.url)).text();

// The dashboard shows package.json's version, so it is only truthful while the three manifests agree.
test('package.json, tauri.conf.json and Cargo.toml declare the same app version', async () => {
  const npm = JSON.parse(await read('package.json')).version;
  const tauri = JSON.parse(await read('src-tauri/tauri.conf.json')).version;
  const cargo = /^\[package\][^[]*?^version\s*=\s*"([^"]+)"/m.exec(await read('src-tauri/Cargo.toml'))?.[1];
  expect(npm).toMatch(/^\d+\.\d+\.\d+/);
  expect(tauri).toBe(npm);
  expect(cargo).toBe(npm);
});
