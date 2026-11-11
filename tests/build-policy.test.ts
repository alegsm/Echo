import { describe, expect, test } from 'bun:test';
import {
  assessReleaseEvidence,
  parseArguments,
  parseBunComparisonBaseline,
  parseBunLock,
  parseCargoLock,
  selectChangedCoordinates,
  type DependencyCoordinate
} from '../scripts/verify-dependency-age';
import { readBunVersion, readRustVersion, requireVersion } from '../scripts/verify-toolchain';

const dependency: DependencyCoordinate = { ecosystem: 'npm', name: 'echo-test', version: '1.2.3' };
const now = new Date('2026-09-09T12:00:00Z');
const bunLockHeader = '{\n  "lockfileVersion": 1,\n  "packages": {\n';

describe('dependency release policy', () => {
  test('rejects a release that is only 71 hours old', () => {
    const publishedAt = new Date(now.getTime() - 71 * 60 * 60 * 1000).toISOString();
    expect(assessReleaseEvidence(dependency, { publishedAt }, now)).toContain('71h old');
  });

  test('rejects missing release evidence and accepts the 72-hour boundary', () => {
    expect(assessReleaseEvidence(dependency, {}, now)).toContain('no publication timestamp');
    const publishedAt = new Date(now.getTime() - 72 * 60 * 60 * 1000).toISOString();
    expect(assessReleaseEvidence(dependency, { publishedAt }, now)).toBeUndefined();
  });

  test('detects lock version drift and rejects missing integrity', () => {
    const before = parseBunLock(`${bunLockHeader}    "echo-test": ["echo-test@1.2.2", "", {}, "sha512-old"],`);
    const after = parseBunLock(`${bunLockHeader}    "echo-test": ["echo-test@1.2.3", "", {}, "sha512-new"],`);
    expect(selectChangedCoordinates(after, before)).toEqual([dependency]);
    expect(() => parseBunLock(`${bunLockHeader}    "echo-test": ["echo-test@1.2.3", "", {}],`)).toThrow('lacks integrity');
  });

  test('rejects unsupported local dependencies and unreadable lock layouts', () => {
    const local = `${bunLockHeader}    "unreviewed": ["unreviewed@vendor/unreviewed.tgz", {}],`;
    expect(() => parseBunLock(local)).toThrow('lacks release evidence');
    const formerlyExempt = `${bunLockHeader}    "@example/local-ui": ["@example/local-ui@vendor/local-ui/local-ui-0.1.1.tgz", {}],`;
    expect(() => parseBunLock(formerlyExempt)).toThrow('lacks release evidence');
    expect(() => parseBunLock('{"lockfileVersion":1,"packages":{}}')).toThrow('no readable package entries');
  });

  test('ignores local dependencies only in the comparison baseline', () => {
    const local = `${bunLockHeader}    "unreviewed": ["unreviewed@file:../unreviewed", {}],
    "echo-test": ["echo-test@1.2.2", "", {}, "sha512-old"],`;
    const current = `${bunLockHeader}    "echo-test": ["echo-test@1.2.3", "", {}, "sha512-new"],`;

    const previous = parseBunComparisonBaseline(local);
    expect(selectChangedCoordinates(parseBunLock(current), previous)).toEqual([dependency]);
    expect(() => parseBunLock(local)).toThrow('lacks release evidence');
  });

  test('rejects a crates.io lock entry without its checksum', () => {
    const lock = '# generated\nversion = 4\n[[package]]\nname = "echo-test"\nversion = "1.2.3"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n';
    expect(() => parseCargoLock(lock)).toThrow('lacks a checksum');
  });

  test('requires one explicit audit mode', () => {
    expect(parseArguments(['--full'])).toEqual({ kind: 'full' });
    expect(parseArguments(['--base', 'abc123'])).toEqual({ kind: 'changed', base: 'abc123' });
    expect(() => parseArguments([])).toThrow('Use --base');
    expect(() => parseArguments(['--artifacts-only'])).toThrow('Use --base');
  });
});

describe('toolchain policy', () => {
  test('reads exact versions and rejects ranges', () => {
    expect(readBunVersion('{"packageManager":"bun@1.3.11"}')).toBe('1.3.11');
    expect(readRustVersion('[toolchain]\nchannel = "1.94.1"')).toBe('1.94.1');
    expect(() => readBunVersion('{"packageManager":"bun@^1.3.11"}')).toThrow('exact');
  });

  test('rejects the wrong installed toolchain', () => {
    expect(() => requireVersion('Bun', '1.3.11', '1.3.10')).toThrow('1.3.11 is required');
    expect(() => requireVersion('rustc', '1.94.1', 'rustc 1.93.0')).toThrow('1.94.1 is required');
  });
});
