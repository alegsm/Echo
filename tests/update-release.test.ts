import { afterAll, describe, expect, test } from 'bun:test';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PUBKEY_PLACEHOLDER, publicKeyId } from '../scripts/update-manifest.ts';

// Release pins and the update-manifest.ts command line. The no-`updater:`-grant and
// overlay-capability pins live once, in src-tauri/src/command_registry.rs.

const root = fileURLToPath(new URL('../', import.meta.url));
const readJson = async (path: string) => JSON.parse(await readFile(join(root, path), 'utf8'));
const shippedConfigs = ['src-tauri/tauri.conf.json', 'src-tauri/tauri.release.conf.json'];
const version: string = (await readJson('package.json')).version;

// The CSP from before the updater existed. Rust performs every update request, so the
// webview needs no new source.
const CSP = "default-src 'none'; connect-src ipc: http://ipc.localhost; font-src 'self'; img-src 'self' data:; media-src 'none'; object-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; worker-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'";

describe('release configuration', () => {
  test('the CSP is byte-identical to the pre-updater string', async () => {
    expect((await readJson('src-tauri/tauri.conf.json')).app.security.csp).toBe(CSP);
  });

  test('updater artifacts are created only through the release overlay', async () => {
    expect(await readJson('src-tauri/tauri.release.conf.json')).toEqual({ bundle: { createUpdaterArtifacts: true } });
    expect((await readJson('src-tauri/tauri.conf.json')).bundle).not.toHaveProperty('createUpdaterArtifacts');
  });

  test('no shipped config allows insecure update transport', async () => {
    for (const path of shippedConfigs) {
      expect(await readFile(join(root, path), 'utf8')).not.toContain('dangerousInsecureTransportProtocol');
    }
  });

  test('the configured pubkey is the placeholder or a well-formed minisign public key', async () => {
    const { pubkey } = (await readJson('src-tauri/tauri.conf.json')).plugins.updater;
    if (pubkey !== PUBKEY_PLACEHOLDER) expect(publicKeyId(pubkey)).toMatch(/^[0-9A-F]{16}$/);
  });

  test('the frontend version comes from ECHO_APP_VERSION when set, else package.json', () => {
    const read = (env: Record<string, string>) => Bun.spawnSync(
      ['bun', '-e', "const { default: config } = await import('./vite.config.ts'); console.log(config.define.__APP_VERSION__)"],
      { cwd: root, env: { ...process.env, ECHO_APP_VERSION: '', ...env }, stdout: 'pipe', stderr: 'pipe' }
    ).stdout.toString().trim();
    expect(read({ ECHO_APP_VERSION: '9.8.7' })).toBe('"9.8.7"');
    expect(read({})).toBe(JSON.stringify(version));
  });
});

// Throwaway keys and a signature from src-tauri/src/services/updater_fixtures; the
// artifact is signed by trusted.pub, not by other.pub.
const fixtures = join(root, 'src-tauri/src/services/updater_fixtures');
const trustedPub = (await readFile(join(fixtures, 'trusted.pub'), 'utf8')).trim();
const otherPub = (await readFile(join(fixtures, 'other.pub'), 'utf8')).trim();
const signature = (await readFile(join(fixtures, 'artifact.bin.sig'), 'utf8')).trim();
const macSig = 'Echo-macos.app.tar.gz.sig';
const windowsSig = 'Echo-windows-setup.exe.sig';
const tag = `v${version}`;

const scratch = await mkdtemp(join(tmpdir(), 'echo-update-release-test-'));
afterAll(() => rm(scratch, { recursive: true, force: true }));
let caseNumber = 0;

/** A fresh assets directory holding the named .sig files, and a --config overlay with `pubkey`. */
async function release(sigs: string[], pubkey: string) {
  const directory = join(scratch, `case-${++caseNumber}`);
  await mkdir(directory);
  for (const name of sigs) await writeFile(join(directory, name), `${signature}\n`);
  const config = join(directory, 'pubkey.json');
  await writeFile(config, JSON.stringify({ plugins: { updater: { pubkey } } }));
  return { directory, config, manifest: join(directory, 'latest.json') };
}

// Asynchronous, so a Bun.serve in this process can answer the script's fetches.
async function runManifest(args: string[]) {
  const child = Bun.spawn(['bun', join(root, 'scripts/update-manifest.ts'), ...args], { cwd: root, stdout: 'pipe', stderr: 'pipe' });
  const [exitCode, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
  return { exitCode, stderr };
}

describe('update-manifest.ts', () => {
  test('writes latest.json for both platforms with the package version and release URLs', async () => {
    const { directory, config, manifest } = await release([macSig, windowsSig], trustedPub);
    expect((await runManifest(['--tag', tag, '--assets', directory, '--config', config])).exitCode).toBe(0);
    const written = JSON.parse(await readFile(manifest, 'utf8'));
    expect(written.version).toBe(version);
    expect(Object.keys(written.platforms).sort()).toEqual(['darwin-aarch64', 'windows-x86_64']);
    const base = `https://github.com/alegsm/Echo/releases/download/${tag}`;
    expect(written.platforms['darwin-aarch64']).toEqual({ url: `${base}/Echo-macos.app.tar.gz`, signature });
    expect(written.platforms['windows-x86_64']).toEqual({ url: `${base}/Echo-windows-setup.exe`, signature });
  });

  test('fetches the .sig files from the release and names a platform the release lacks', async () => {
    const requested: string[] = [];
    const server = Bun.serve({
      hostname: '127.0.0.1', port: 0,
      fetch(request) {
        const path = new URL(request.url).pathname;
        requested.push(path);
        return path === `/${tag}/${macSig}` ? new Response(signature) : new Response('Not Found', { status: 404 });
      }
    });
    try {
      const { config, manifest } = await release([], trustedPub);
      const result = await runManifest(['--tag', tag, '--base-url', `http://127.0.0.1:${server.port}`, '--config', config, '--out', manifest]);
      expect(requested.sort()).toEqual([`/${tag}/${macSig}`, `/${tag}/${windowsSig}`]);
      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain('windows-x86_64');
      expect(result.stderr).not.toContain('darwin-aarch64');
      expect(existsSync(manifest)).toBe(false);
    } finally {
      server.stop(true);
    }
  });

  const refusals: [string, string[], string, (tag: string) => string, RegExp][] = [
    ['only the macOS .sig', [macSig], trustedPub, (tag) => tag, /windows-x86_64/],
    ['only the Windows .sig', [windowsSig], trustedPub, (tag) => tag, /darwin-aarch64/],
    ['a .sig from another key', [macSig, windowsSig], otherPub, (tag) => tag, /key 256B1FC0B0A30D1E.*0B09975660912D1A/],
    ['the placeholder pubkey', [macSig, windowsSig], PUBKEY_PLACEHOLDER, (tag) => tag, /placeholder/],
    // A .sig pasted as the pubkey carries the same key id, so only the shape check refuses it.
    ['a signature in place of the pubkey', [macSig, windowsSig], signature, (tag) => tag, /not a minisign public key/],
    ['a tag with a digit before the version', [macSig, windowsSig], trustedPub, (tag) => tag.replace('v', 'v1'), /must be exactly v/],
    ['a tag without the v', [macSig, windowsSig], trustedPub, (tag) => tag.slice(1), /must be exactly v/],
    ['a tag with a suffix', [macSig, windowsSig], trustedPub, (tag) => `${tag}-beta.1`, /must be exactly v/]
  ];

  test.each(refusals)('refuses %s, exits non-zero and writes nothing', async (_name, sigs, pubkey, tagOf, reason) => {
    const { directory, config, manifest } = await release(sigs, pubkey);
    const result = await runManifest(['--tag', tagOf(tag), '--assets', directory, '--config', config]);
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toMatch(reason);
    expect(existsSync(manifest)).toBe(false);
  });
});
