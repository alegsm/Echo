export function readBunVersion(packageJson: string): string {
  const parsed = JSON.parse(packageJson) as { packageManager?: unknown };
  if (typeof parsed.packageManager !== 'string') throw new Error('packageManager must pin Bun.');
  const match = /^bun@(\d+\.\d+\.\d+)$/.exec(parsed.packageManager);
  if (!match) throw new Error('packageManager must use an exact bun@x.y.z version.');
  return match[1];
}

export function readRustVersion(toolchainToml: string): string {
  const match = /^channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$/m.exec(toolchainToml);
  if (!match) throw new Error('rust-toolchain.toml must use an exact x.y.z channel.');
  return match[1];
}

export function requireVersion(tool: string, expected: string, actualOutput: string): void {
  const actual = actualOutput.match(/\d+\.\d+\.\d+/)?.[0];
  if (actual !== expected) {
    throw new Error(`${tool} ${expected} is required; found ${actual ?? 'an unreadable version'}.`);
  }
}

function runVersionCommand(command: string, arguments_: string[]): string {
  const result = Bun.spawnSync([command, ...arguments_], { stderr: 'pipe', stdout: 'pipe' });
  if (result.exitCode !== 0) {
    const message = result.stderr.toString().trim() || 'command unavailable';
    throw new Error(`${command} version check failed: ${message}`);
  }
  return result.stdout.toString().trim();
}

export async function verifyToolchain(repositoryRoot: string): Promise<{ bun: string; rust: string }> {
  const packageJson = await Bun.file(`${repositoryRoot}/package.json`).text();
  const toolchainToml = await Bun.file(`${repositoryRoot}/rust-toolchain.toml`).text();
  const bunVersion = readBunVersion(packageJson);
  const rustVersion = readRustVersion(toolchainToml);
  requireVersion('Bun', bunVersion, runVersionCommand('bun', ['--version']));
  requireVersion('rustc', rustVersion, runVersionCommand('rustc', ['--version']));
  requireVersion('Cargo', rustVersion, runVersionCommand('cargo', ['--version']));
  return { bun: bunVersion, rust: rustVersion };
}

if (import.meta.main) {
  const repositoryRoot = `${import.meta.dir}/..`;
  try {
    const versions = await verifyToolchain(repositoryRoot);
    console.log(`Toolchain verified: Bun ${versions.bun}, Rust ${versions.rust}.`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
