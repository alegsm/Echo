// Writes latest.json, the manifest installed copies of Echo read to find an update.
// Run it once both platforms' .sig files are on the GitHub release, then upload its
// output last: a release without latest.json offers nothing, so a half-uploaded release
// is safe. docs/development.md#publish-a-release has the full release order.
//
//   bun scripts/update-manifest.ts --tag v0.5.0 [--assets <dir>] [--base-url <url>] [--config <json>] [--out <path>]
//
// --assets reads the .sig files from a local directory instead of fetching them from the
// release. --base-url replaces https://github.com/alegsm/Echo/releases/download for both
// the fetch and the manifest URLs. --config names a JSON file whose plugins.updater.pubkey
// replaces the one in src-tauri/tauri.conf.json. The output defaults to latest.json in the
// --assets directory, or in the current directory when fetching.

import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

/** The committed pubkey until the update key exists. A release must never ship with it. */
export const PUBKEY_PLACEHOLDER = 'ECHO_UPDATE_PUBLIC_KEY_NOT_SET';
const GITHUB_RELEASES = 'https://github.com/alegsm/Echo/releases/download';

/** The updater artifact each platform's installed copy downloads, by Tauri platform key. */
export const RELEASE_ASSETS = {
  'darwin-aarch64': 'Echo-macos.app.tar.gz',
  'windows-x86_64': 'Echo-windows-setup.exe'
} as const;
export type UpdatePlatform = keyof typeof RELEASE_ASSETS;

export type Manifest = {
  version: string;
  pub_date: string;
  platforms: Partial<Record<UpdatePlatform, { url: string; signature: string }>>;
};

const repositoryRoot = resolve(import.meta.dir, '..');

/** Decodes base64 and refuses text that does not round-trip, since Buffer.from skips bad characters. */
function strictBase64(text: string): Buffer | undefined {
  const bytes = Buffer.from(text, 'base64');
  return bytes.length > 0 && bytes.toString('base64') === text ? bytes : undefined;
}

/** Minisign prints a key id as its 8 stored bytes reversed, in uppercase hex. */
function keyIdHex(bytes: Buffer) {
  return Buffer.from(bytes).reverse().toString('hex').toUpperCase();
}

/**
 * The key id of a Tauri updater pubkey: base64 of a minisign public key file, whose
 * second line is base64 of "Ed", the 8-byte key id and the 32-byte Ed25519 key.
 */
export function publicKeyId(pubkey: string): string {
  if (pubkey === PUBKEY_PLACEHOLDER) {
    throw new Error(`The configured updater pubkey is the placeholder ${PUBKEY_PLACEHOLDER}. Commit the real public key before a release.`);
  }
  const keyLine = strictBase64(pubkey)?.toString('utf8').split('\n')[1];
  const key = keyLine === undefined ? undefined : strictBase64(keyLine.trim());
  if (key?.length !== 42 || key.toString('latin1', 0, 2) !== 'Ed') {
    throw new Error('The configured updater pubkey is not a minisign public key.');
  }
  return keyIdHex(key.subarray(2, 10));
}

/**
 * The key id of a Tauri .sig file: base64 of a minisign signature file, whose second
 * line is base64 of the algorithm ("Ed" or the prehashed "ED"), the key id and the signature.
 */
export function signatureKeyId(signature: string, name: string): string {
  const signatureLine = strictBase64(signature)?.toString('utf8').split('\n')[1];
  const bytes = signatureLine === undefined ? undefined : strictBase64(signatureLine.trim());
  if (bytes?.length !== 74 || !['Ed', 'ED'].includes(bytes.toString('latin1', 0, 2))) {
    throw new Error(`${name} is not a minisign signature.`);
  }
  return keyIdHex(bytes.subarray(2, 10));
}

/** Refuses a signature made by any key other than `pubkey`, which the installed copies would reject. */
export function requireSignedBy(signature: string, name: string, pubkey: string) {
  const expected = publicKeyId(pubkey);
  const actual = signatureKeyId(signature, name);
  if (actual !== expected) {
    throw new Error(`${name} is signed by key ${actual}, but the configured updater pubkey is key ${expected}.`);
  }
}

/** The Tauri static manifest. `version` has no leading v. */
export function createManifest(version: string, platforms: Manifest['platforms']): Manifest {
  return { version, pub_date: new Date().toISOString(), platforms };
}

async function readSignatures(tag: string, assets: string | undefined, baseUrl: string) {
  const found: Partial<Record<UpdatePlatform, string>> = {};
  for (const [platform, asset] of Object.entries(RELEASE_ASSETS) as [UpdatePlatform, string][]) {
    const name = `${asset}.sig`;
    const text = assets ? await readLocal(join(assets, name)) : await fetchRelease(`${baseUrl}/${tag}/${name}`);
    if (text !== undefined) found[platform] = text.trim();
  }
  const missing = (Object.keys(RELEASE_ASSETS) as UpdatePlatform[]).filter((platform) => !(platform in found));
  if (missing.length > 0) {
    const names = missing.map((platform) => `${platform} (${RELEASE_ASSETS[platform]}.sig)`).join(', ');
    throw new Error(`The release is missing the signature for ${names}. Upload it, then run this again.`);
  }
  return found as Record<UpdatePlatform, string>;
}

async function readLocal(path: string) {
  try {
    return await readFile(path, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

async function fetchRelease(url: string) {
  const response = await fetch(url);
  if (response.status === 404) return undefined;
  if (!response.ok) throw new Error(`${url} answered ${response.status}.`);
  return response.text();
}

async function configuredPubkey(overlay: string | undefined): Promise<string> {
  const base = JSON.parse(await readFile(join(repositoryRoot, 'src-tauri/tauri.conf.json'), 'utf8'));
  const override = overlay ? JSON.parse(await readFile(overlay, 'utf8'))?.plugins?.updater?.pubkey : undefined;
  return override ?? base.plugins.updater.pubkey;
}

export async function writeReleaseManifest(argv: string[]): Promise<string> {
  const { values } = parseArgs({
    args: argv,
    strict: true,
    options: {
      tag: { type: 'string' },
      assets: { type: 'string' },
      'base-url': { type: 'string' },
      config: { type: 'string' },
      out: { type: 'string' }
    }
  });
  const { version } = JSON.parse(await readFile(join(repositoryRoot, 'package.json'), 'utf8'));
  if (values.tag !== `v${version}`) {
    throw new Error(`--tag must be exactly v${version}, the package.json version with a leading v; got ${values.tag ?? 'nothing'}.`);
  }
  const pubkey = await configuredPubkey(values.config);
  publicKeyId(pubkey);
  const baseUrl = (values['base-url'] ?? GITHUB_RELEASES).replace(/\/+$/, '');
  const signatures = await readSignatures(values.tag, values.assets, baseUrl);
  const platforms: Manifest['platforms'] = {};
  for (const [platform, signature] of Object.entries(signatures) as [UpdatePlatform, string][]) {
    requireSignedBy(signature, `${RELEASE_ASSETS[platform]}.sig`, pubkey);
    platforms[platform] = { url: `${baseUrl}/${values.tag}/${RELEASE_ASSETS[platform]}`, signature };
  }
  const out = values.out ?? join(values.assets ?? process.cwd(), 'latest.json');
  await writeFile(out, `${JSON.stringify(createManifest(version, platforms), null, 2)}\n`);
  return out;
}

if (import.meta.main) {
  try {
    const out = await writeReleaseManifest(process.argv.slice(2));
    console.log(`Wrote ${out}. Upload it to the release last.`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
