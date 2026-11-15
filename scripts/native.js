/**
 * Locates the native frob binary for this host.
 *
 * Shared by bin/frob.js and scripts/postinstall.js. Kept free of
 * subprocesses because the launcher imports it on every wrapped invocation.
 */

import { existsSync } from 'fs';
import { createRequire } from 'module';
import { dirname, join } from 'path';
import { platform, arch } from 'os';

// Uses the process report instead of spawning `ldd`, which costs a shell
// startup on every call.
export function hostLibc() {
  if (platform() !== 'linux') return null;
  process.report.excludeNetwork = true;
  const report = process.report.getReport();
  if (report.header.glibcVersionRuntime) return 'glibc';
  if (report.sharedObjects.some((path) => /(?:^|\/)ld-musl-|libc\.musl-/.test(path))) return 'musl';
  // A statically linked Node has no loaded libc in its process report.
  const loaderArch = arch() === 'arm64' ? 'aarch64' : 'x86_64';
  return existsSync(`/lib/ld-musl-${loaderArch}.so.1`) ? 'musl' : 'glibc';
}

/**
 * Describes where the binary for a host can live:
 * - packageName/file: the per-platform optional dependency and its executable
 * - binaryName: the file name used inside this package's bin/ directory
 *   (postinstall download, source builds and linked installs)
 * - candidates: bin/ file names to try, in order
 */
export function nativeTarget(os = platform(), cpuArch = arch(), libc = os === 'linux' ? hostLibc() : null) {
  let archKey;
  switch (cpuArch) {
    case 'x64':
    case 'x86_64':
      archKey = 'x64';
      break;
    case 'arm64':
    case 'aarch64':
      archKey = 'arm64';
      break;
    default:
      return null;
  }

  switch (os) {
    case 'darwin':
      return {
        packageName: `@frob/cli-darwin-${archKey}`,
        file: 'frob',
        binaryName: `frob-darwin-${archKey}`,
        candidates: [`frob-darwin-${archKey}`],
      };
    case 'linux': {
      const musl = libc === 'musl';
      const binaryName = `frob-linux-${musl ? 'musl-' : ''}${archKey}`;
      return {
        packageName: `@frob/cli-linux-${archKey}-${musl ? 'musl' : 'gnu'}`,
        file: 'frob',
        binaryName,
        candidates: [binaryName],
      };
    }
    case 'win32':
      // Prefer a source-built native ARM64 executable when present. Otherwise,
      // use the published x64 binary through Windows' built-in emulation.
      return {
        packageName: '@frob/cli-win32-x64-msvc',
        file: 'frob.exe',
        binaryName: 'frob-win32-x64.exe',
        candidates:
          archKey === 'arm64'
            ? ['frob-win32-arm64.exe', 'frob-win32-x64.exe']
            : ['frob-win32-x64.exe'],
      };
    default:
      return null;
  }
}

// Returns the binary inside the installed platform package, or null.
export function platformPackageBinary(packageRoot, target) {
  try {
    const require = createRequire(join(packageRoot, 'package.json'));
    const manifest = require.resolve(`${target.packageName}/package.json`);
    const binary = join(dirname(manifest), 'bin', target.file);
    return existsSync(binary) ? binary : null;
  } catch {
    return null;
  }
}

// Returns the binary to run: a file already in bin/, else the platform package.
export function findNativeBinary(packageRoot, target) {
  for (const name of target.candidates) {
    const binary = join(packageRoot, 'bin', name);
    if (existsSync(binary)) return binary;
  }
  return platformPackageBinary(packageRoot, target);
}
