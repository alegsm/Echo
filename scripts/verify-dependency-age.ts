const MINIMUM_RELEASE_AGE_MS = 72 * 60 * 60 * 1000;
const RELEASE_EVIDENCE_TIMEOUT_MS = 10_000;

export type DependencyCoordinate = {
  ecosystem: 'cargo' | 'npm';
  name: string;
  version: string;
};

type ReleaseEvidence = { publishedAt?: string; isYanked?: boolean };
type VerificationMode = { kind: 'changed'; base: string } | { kind: 'full' };

export function parseArguments(arguments_: string[]): VerificationMode {
  if (arguments_.length === 1 && arguments_[0] === '--full') return { kind: 'full' };
  if (arguments_.length === 2 && arguments_[0] === '--base' && arguments_[1].trim()) {
    return { kind: 'changed', base: arguments_[1] };
  }
  throw new Error('Use --base <git-commit> or --full.');
}

export function parseBunLock(lockText: string): DependencyCoordinate[] {
  return parseBunLockCoordinates(lockText, 'current');
}

export function parseBunComparisonBaseline(lockText: string): DependencyCoordinate[] {
  return parseBunLockCoordinates(lockText, 'comparison-baseline');
}

function parseBunLockCoordinates(
  lockText: string,
  source: 'current' | 'comparison-baseline'
): DependencyCoordinate[] {
  if (!/"lockfileVersion"\s*:\s*1/.test(lockText)) {
    throw new Error('Unsupported or malformed Bun lockfile.');
  }
  const dependencies: DependencyCoordinate[] = [];
  let resolutionCount = 0;
  for (const line of lockText.split('\n')) {
    const resolution = /^\s{4}"[^"]+": \["([^"]+)"/.exec(line)?.[1];
    if (!resolution) continue;
    resolutionCount += 1;
    const separator = resolution.lastIndexOf('@');
    if (separator < 1) throw new Error(`Unreadable Bun lock resolution: ${resolution}`);
    const name = resolution.slice(0, separator);
    const version = resolution.slice(separator + 1);
    if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
      if (source === 'comparison-baseline') continue;
      throw new Error(`Non-registry Bun dependency lacks release evidence: ${resolution}`);
    }
    if (!line.includes('sha512-')) throw new Error(`Bun lock entry lacks integrity: ${resolution}`);
    dependencies.push({ ecosystem: 'npm', name, version });
  }
  if (resolutionCount === 0) throw new Error('Bun lockfile contains no readable package entries.');
  return uniqueCoordinates(dependencies);
}

export function parseCargoLock(lockText: string): DependencyCoordinate[] {
  const dependencies: DependencyCoordinate[] = [];
  for (const block of lockText.split('[[package]]').slice(1)) {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
    const source = /^source = "([^"]+)"$/m.exec(block)?.[1];
    if (!source) continue;
    if (!name || !version) throw new Error('Cargo lock contains an unreadable package entry.');
    if (source !== 'registry+https://github.com/rust-lang/crates.io-index') {
      throw new Error(`Non-crates.io dependency lacks release evidence: ${name}@${version}`);
    }
    if (!/^checksum = "[a-f0-9]{64}"$/m.test(block)) {
      throw new Error(`Cargo lock entry lacks a checksum: ${name}@${version}`);
    }
    dependencies.push({ ecosystem: 'cargo', name, version });
  }
  return uniqueCoordinates(dependencies);
}

export function selectChangedCoordinates(
  current: DependencyCoordinate[],
  baseline: DependencyCoordinate[]
): DependencyCoordinate[] {
  const previous = new Set(baseline.map(coordinateKey));
  return current.filter((coordinate) => !previous.has(coordinateKey(coordinate)));
}

export function assessReleaseEvidence(
  coordinate: DependencyCoordinate,
  evidence: ReleaseEvidence,
  now: Date
): string | undefined {
  if (evidence.isYanked) return `${coordinateKey(coordinate)} is yanked.`;
  if (!evidence.publishedAt) return `${coordinateKey(coordinate)} has no publication timestamp.`;
  const publishedAt = Date.parse(evidence.publishedAt);
  if (!Number.isFinite(publishedAt)) return `${coordinateKey(coordinate)} has an invalid publication timestamp.`;
  const age = now.getTime() - publishedAt;
  if (age < MINIMUM_RELEASE_AGE_MS) {
    return `${coordinateKey(coordinate)} is ${Math.floor(age / 3_600_000)}h old; 72h is required.`;
  }
  return undefined;
}

async function readGitFile(repositoryRoot: string, base: string, path: string): Promise<string> {
  const result = Bun.spawnSync(['git', '-C', repositoryRoot, 'show', `${base}:${path}`], {
    stderr: 'pipe', stdout: 'pipe'
  });
  if (result.exitCode !== 0) {
    throw new Error(`Cannot read ${path} at base ${base}: ${result.stderr.toString().trim()}`);
  }
  return result.stdout.toString();
}

async function fetchJson(url: string): Promise<Record<string, unknown>> {
  const response = await fetch(url, {
    headers: { 'User-Agent': 'Echo dependency age verifier' },
    signal: AbortSignal.timeout(RELEASE_EVIDENCE_TIMEOUT_MS)
  });
  if (!response.ok) throw new Error(`Release evidence request failed (${response.status}): ${url}`);
  return response.json() as Promise<Record<string, unknown>>;
}

async function loadEvidence(coordinate: DependencyCoordinate): Promise<ReleaseEvidence> {
  if (coordinate.ecosystem === 'npm') {
    const metadata = await fetchJson(`https://registry.npmjs.org/${encodeURIComponent(coordinate.name)}`);
    const times = metadata.time;
    const timestamp = isRecord(times) ? times[coordinate.version] : undefined;
    const publishedAt = typeof timestamp === 'string' ? timestamp : undefined;
    return { publishedAt };
  }
  const metadata = await fetchJson(
    `https://crates.io/api/v1/crates/${encodeURIComponent(coordinate.name)}/${encodeURIComponent(coordinate.version)}`
  );
  const version = metadata.version;
  return isRecord(version)
    ? {
        publishedAt: typeof version.created_at === 'string' ? version.created_at : undefined,
        isYanked: typeof version.yanked === 'boolean' ? version.yanked : undefined
      }
    : {};
}

async function verifyReleaseAges(coordinates: DependencyCoordinate[]): Promise<void> {
  const errors: string[] = [];
  const queue = [...coordinates];
  const workers = Array.from({ length: Math.min(8, queue.length) }, async () => {
    for (let coordinate = queue.shift(); coordinate; coordinate = queue.shift()) {
      try {
        const error = assessReleaseEvidence(coordinate, await loadEvidence(coordinate), new Date());
        if (error) errors.push(error);
      } catch (error) {
        errors.push(`${coordinateKey(coordinate)}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  });
  await Promise.all(workers);
  if (errors.length) throw new Error(errors.sort().join('\n'));
}

async function run(mode: VerificationMode): Promise<number> {
  const repositoryRoot = `${import.meta.dir}/..`;
  const bunLock = await Bun.file(`${repositoryRoot}/bun.lock`).text();
  const cargoLock = await Bun.file(`${repositoryRoot}/src-tauri/Cargo.lock`).text();
  const current = [...parseBunLock(bunLock), ...parseCargoLock(cargoLock)];
  const selected = mode.kind === 'full' ? current : selectChangedCoordinates(current, [
    ...parseBunComparisonBaseline(await readGitFile(repositoryRoot, mode.base, 'bun.lock')),
    ...parseCargoLock(await readGitFile(repositoryRoot, mode.base, 'src-tauri/Cargo.lock'))
  ]);
  await verifyReleaseAges(selected);
  return selected.length;
}

function coordinateKey(coordinate: DependencyCoordinate): string {
  return `${coordinate.ecosystem}:${coordinate.name}@${coordinate.version}`;
}

function uniqueCoordinates(coordinates: DependencyCoordinate[]): DependencyCoordinate[] {
  return [...new Map(coordinates.map((coordinate) => [coordinateKey(coordinate), coordinate])).values()];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

if (import.meta.main) {
  try {
    const checked = await run(parseArguments(process.argv.slice(2)));
    console.log(`Dependency policy verified for ${checked} registry entries.`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
