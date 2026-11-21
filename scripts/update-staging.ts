// Stages an in-app update on this machine: builds Echo vN and vN+1 from this checkout,
// signed with a throwaway key, and serves vN+1's manifest and artifact on loopback.
// Install the printed vN build by hand, start it, and its update button installs vN+1.
//
//   bun scripts/update-staging.ts                       build both versions and serve
//   bun scripts/update-staging.ts --serve <dir> [--tamper]  serve an earlier run again
//
// vN is package.json's version and vN+1 its next patch. The key pair, the config overlays
// and the copied builds live in one new directory under the OS temp dir; no tracked file
// changes. The builds use src-tauri/tauri.release.conf.json plus an overlay that sets the
// version, the staging pubkey and a loopback http:// endpoint, which only staging may use.
// --tamper serves the same manifest with one artifact byte flipped, so the signature check
// must fail. macOS builds are signed with APPLE_SIGNING_IDENTITY when it is set. This
// script never installs or starts Echo. Ctrl+C stops the server and keeps the directory.

import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { createManifest, RELEASE_ASSETS, requireSignedBy, type UpdatePlatform } from './update-manifest.ts';

type Staging = { port: number; from: string; to: string; platform: UpdatePlatform };

const repositoryRoot = resolve(import.meta.dir, '..');
/** Printed when the script stops, so a failed or interrupted run can be inspected. */
let stagingDirectory: string | undefined;
const bundleRoot = join(repositoryRoot, 'src-tauri/target/release/bundle');

function hostPlatform(): UpdatePlatform {
  if (process.platform === 'darwin' && process.arch === 'arm64') return 'darwin-aarch64';
  if (process.platform === 'win32' && process.arch === 'x64') return 'windows-x86_64';
  throw new Error(`Echo publishes updates for Apple Silicon macOS and x64 Windows, not ${process.platform}-${process.arch}.`);
}

function nextPatch(version: string) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`package.json version ${version} is not x.y.z.`);
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
}

/** Where tauri build leaves the installable bundle and the signed updater artifact. */
function buildOutputs(platform: UpdatePlatform, version: string) {
  if (platform === 'darwin-aarch64') {
    const app = join(bundleRoot, 'macos/Echo.app');
    return { bundles: 'app', installable: app, artifact: `${app}.tar.gz` };
  }
  const setup = join(bundleRoot, `nsis/Echo_${version}_x64-setup.exe`);
  return { bundles: 'nsis', installable: setup, artifact: setup };
}

async function run(command: string[], env: Record<string, string | undefined> = process.env, quiet = false) {
  const child = Bun.spawn(command, { cwd: repositoryRoot, env, stdin: 'ignore', stdout: quiet ? 'ignore' : 'inherit', stderr: 'inherit' });
  if ((await child.exited) !== 0) throw new Error(`${command.slice(0, 4).join(' ')} failed.`);
}

async function build(staging: Staging, directory: string, version: string, pubkey: string) {
  const overlay = join(directory, `overlay-${version}.json`);
  await writeFile(overlay, `${JSON.stringify({
    version,
    plugins: { updater: { pubkey, endpoints: [`http://127.0.0.1:${staging.port}/latest.json`], dangerousInsecureTransportProtocol: true } }
  }, null, 2)}\n`);
  const outputs = buildOutputs(staging.platform, version);
  console.log(`\n==> Building Echo v${version}`);
  await run(
    ['bun', 'run', 'tauri', 'build', '--ci', '--bundles', outputs.bundles, '--config', 'src-tauri/tauri.release.conf.json', '--config', overlay],
    // The key path, not the key, goes through the environment; the Tauri CLI reads the file.
    { ...process.env, ECHO_APP_VERSION: version, TAURI_SIGNING_PRIVATE_KEY: join(directory, 'staging.key') }
  );
  return outputs;
}

async function stage() {
  const platform = hostPlatform();
  const { version: from } = JSON.parse(await readFile(join(repositoryRoot, 'package.json'), 'utf8'));
  const directory = await mkdtemp(join(tmpdir(), 'echo-update-staging-'));
  stagingDirectory = directory;
  console.log(`Staging directory: ${directory}`);
  if (platform === 'darwin-aarch64' && !process.env.APPLE_SIGNING_IDENTITY) {
    console.log('APPLE_SIGNING_IDENTITY is not set: both builds stay unsigned, and macOS may ask again for the permissions an installed copy had.');
  }
  // Bind first so the vN build can embed the port it will check.
  const server = serve(directory, 0, platform, false);
  const staging: Staging = { port: server.port!, from, to: nextPatch(from), platform };
  await writeFile(join(directory, 'staging.json'), `${JSON.stringify(staging, null, 2)}\n`);

  await run(['bun', 'run', 'tauri', 'signer', 'generate', '--ci', '-w', join(directory, 'staging.key')], process.env, true);
  const pubkey = (await readFile(join(directory, 'staging.key.pub'), 'utf8')).trim();

  const older = await build(staging, directory, staging.from, pubkey);
  const installDirectory = join(directory, `v${staging.from}`);
  await mkdir(installDirectory);
  const installable = join(installDirectory, platform === 'darwin-aarch64' ? 'Echo.app' : `Echo_${staging.from}_x64-setup.exe`);
  // ditto keeps the bundle's code signature and symlinks intact.
  if (platform === 'darwin-aarch64') await run(['ditto', older.installable, installable]);
  else await copyFile(older.installable, installable);

  const newer = await build(staging, directory, staging.to, pubkey);
  const served = join(directory, 'serve');
  await mkdir(served);
  const asset = RELEASE_ASSETS[platform];
  await copyFile(newer.artifact, join(served, asset));
  const signature = (await readFile(`${newer.artifact}.sig`, 'utf8')).trim();
  requireSignedBy(signature, `${asset}.sig`, pubkey);
  const manifest = createManifest(staging.to, { [platform]: { url: `http://127.0.0.1:${staging.port}/${asset}`, signature } });
  await writeFile(join(served, 'latest.json'), `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(`\nInstall v${staging.from} by hand from: ${installable}`);
  report(directory, staging, false);
}

/** Serves latest.json and the artifact from `<directory>/serve`; every other path is 404. */
function serve(directory: string, port: number, platform: UpdatePlatform, tamper: boolean) {
  const asset = RELEASE_ASSETS[platform];
  let tampered: Uint8Array<ArrayBuffer> | undefined;
  const server = Bun.serve({
    hostname: '127.0.0.1',
    port,
    idleTimeout: 0,
    async fetch(request) {
      const path = new URL(request.url).pathname;
      const file = Bun.file(join(directory, 'serve', path.slice(1)));
      let response: Response;
      if (request.method !== 'GET' || (path !== '/latest.json' && path !== `/${asset}`) || !(await file.exists())) {
        response = new Response('Not Found', { status: 404 });
      } else if (tamper && path === `/${asset}`) {
        tampered ??= flipLastByte(new Uint8Array(await file.arrayBuffer()));
        response = new Response(tampered);
      } else {
        response = new Response(file);
      }
      console.log(`${new Date().toISOString()} ${request.method} ${path} -> ${response.status}${tamper && path === `/${asset}` ? ' (tampered)' : ''}`);
      return response;
    }
  });
  return server;
}

function flipLastByte(bytes: Uint8Array<ArrayBuffer>) {
  bytes[bytes.length - 1] ^= 0xff;
  return bytes;
}

function report(directory: string, staging: Staging, tamper: boolean) {
  console.log(`Manifest: http://127.0.0.1:${staging.port}/latest.json offers v${staging.to}${tamper ? ' with a tampered artifact' : ''}`);
  console.log(`Serve again: bun scripts/update-staging.ts --serve ${directory} [--tamper]`);
  console.log('Press Ctrl+C to stop serving. The staging directory stays for inspection.');
}

async function serveAgain(directory: string, tamper: boolean) {
  stagingDirectory = directory;
  const staging: Staging = JSON.parse(await readFile(join(directory, 'staging.json'), 'utf8'));
  serve(directory, staging.port, staging.platform, tamper);
  report(directory, staging, tamper);
}

if (import.meta.main) {
  const where = () => `Staging directory: ${stagingDirectory ?? 'not created'}`;
  process.on('SIGINT', () => {
    console.log(`\nStopped. ${where()}`);
    process.exit(0);
  });
  try {
    const { values } = parseArgs({ args: process.argv.slice(2), strict: true, options: { serve: { type: 'string' }, tamper: { type: 'boolean' } } });
    if (values.tamper && !values.serve) throw new Error('--tamper serves an earlier run again: pass --serve <dir> with it.');
    if (values.serve) await serveAgain(resolve(values.serve), values.tamper ?? false);
    else await stage();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(where());
    process.exit(1);
  }
}
