#!/usr/bin/env node

/**
 * Copies release binaries into the per-platform npm packages.
 *
 * Usage: node scripts/stage-native-packages.js <artifacts-dir>
 *
 * The artifacts directory holds the files built by the release workflow
 * (frob-linux-x64, frob-win32-x64.exe, ...), in any layout.
 * Each one is copied to packages/@frob/cli-<key>/bin/. Exits non-zero
 * if any binary is missing or implausibly small.
 */

import { chmodSync, copyFileSync, mkdirSync, readdirSync, statSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const rootDir = join(dirname(fileURLToPath(import.meta.url)), '..');
const MIN_SIZE = 100000;

// Release artifact name -> platform package directory and executable name
const NATIVE_PACKAGES = [
  ['frob-darwin-arm64', 'cli-darwin-arm64', 'frob'],
  ['frob-darwin-x64', 'cli-darwin-x64', 'frob'],
  ['frob-linux-x64', 'cli-linux-x64-gnu', 'frob'],
  ['frob-linux-arm64', 'cli-linux-arm64-gnu', 'frob'],
  ['frob-linux-musl-x64', 'cli-linux-x64-musl', 'frob'],
  ['frob-linux-musl-arm64', 'cli-linux-arm64-musl', 'frob'],
  ['frob-win32-x64.exe', 'cli-win32-x64-msvc', 'frob.exe'],
];

function findFiles(directory) {
  const files = new Map();
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      for (const [name, nested] of findFiles(path)) files.set(name, nested);
    } else {
      files.set(entry.name, path);
    }
  }
  return files;
}

const artifactsDir = process.argv[2];
if (!artifactsDir) {
  console.error('Usage: node scripts/stage-native-packages.js <artifacts-dir>');
  process.exit(1);
}

const artifacts = findFiles(artifactsDir);
let errors = 0;
for (const [artifact, packageDir, file] of NATIVE_PACKAGES) {
  const source = artifacts.get(artifact);
  if (!source) {
    console.error(`ERROR: Missing ${artifact}`);
    errors++;
    continue;
  }
  const size = statSync(source).size;
  if (size < MIN_SIZE) {
    console.error(`ERROR: ${artifact} is too small (${size} bytes, expected >= ${MIN_SIZE})`);
    errors++;
    continue;
  }
  const destination = join(rootDir, 'packages', '@frob', packageDir, 'bin', file);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  console.log(`OK: ${artifact} -> packages/@frob/${packageDir}/bin/${file} (${size} bytes)`);
}

if (errors > 0) {
  console.error(`Error: ${errors} binary issues found`);
  process.exit(1);
}
console.log(`All ${NATIVE_PACKAGES.length} platform packages staged`);
