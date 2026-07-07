#!/usr/bin/env node
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync, chmodSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, '..');
const version = process.env.DAIRO_CLI_VERSION || readCargoVersion(join(repoRoot, 'Cargo.toml'));
const binaryRoot = resolve(process.env.DAIRO_BINARY_ROOT || join(repoRoot, 'dist', 'binaries'));
const packageRoot = resolve(process.env.DAIRO_NPM_PACKAGE_ROOT || join(repoRoot, 'dist', 'npm'));
const npmScope = process.env.DAIRO_NPM_SCOPE || '@dairo-app';

// The gnu packages declare libc so npm skips them on musl systems (Alpine).
// The musl packages are static binaries that run on any Linux, so they stay
// libc-unconstrained: glibc systems install both flavors and the launcher
// picks gnu when the runtime glibc is new enough, musl otherwise.
const platforms = [
  { target: 'aarch64-apple-darwin', npm: 'cli-darwin-arm64', os: 'darwin', cpu: 'arm64', exe: 'dairo' },
  { target: 'x86_64-apple-darwin', npm: 'cli-darwin-x64', os: 'darwin', cpu: 'x64', exe: 'dairo' },
  { target: 'x86_64-unknown-linux-gnu', npm: 'cli-linux-x64', os: 'linux', cpu: 'x64', libc: ['glibc'], exe: 'dairo' },
  { target: 'aarch64-unknown-linux-gnu', npm: 'cli-linux-arm64', os: 'linux', cpu: 'arm64', libc: ['glibc'], exe: 'dairo' },
  { target: 'x86_64-unknown-linux-musl', npm: 'cli-linux-x64-musl', os: 'linux', cpu: 'x64', exe: 'dairo' },
  { target: 'aarch64-unknown-linux-musl', npm: 'cli-linux-arm64-musl', os: 'linux', cpu: 'arm64', exe: 'dairo' },
  { target: 'x86_64-pc-windows-msvc', npm: 'cli-win32-x64', os: 'win32', cpu: 'x64', exe: 'dairo.exe' },
  { target: 'aarch64-pc-windows-msvc', npm: 'cli-win32-arm64', os: 'win32', cpu: 'arm64', exe: 'dairo.exe' },
];

rmSync(packageRoot, { recursive: true, force: true });
mkdirSync(packageRoot, { recursive: true });

const optionalDependencies = {};
for (const platform of platforms) {
  const packageName = `${npmScope}/${platform.npm}`;
  optionalDependencies[packageName] = version;
  const dir = join(packageRoot, platform.npm);
  const binDir = join(dir, 'bin');
  mkdirSync(binDir, { recursive: true });
  const source = findBinary(platform);
  const dest = join(binDir, platform.exe);
  copyFileSync(source, dest);
  if (platform.os !== 'win32') chmodSync(dest, 0o755);
  writeFileSync(join(dir, 'package.json'), JSON.stringify({
    name: packageName,
    version,
    description: `Dairo CLI native binary for ${platform.target}`,
    license: 'MIT',
    repository: { type: 'git', url: 'git+https://github.com/dairo-app/dairo-cli.git' },
    os: [platform.os],
    cpu: [platform.cpu],
    ...(platform.libc ? { libc: platform.libc } : {}),
    files: ['bin'],
  }, null, 2) + '\n');
  writeFileSync(join(dir, 'README.md'), `# ${packageName}\n\nNative Dairo CLI binary for \`${platform.target}\`. Install \`${npmScope}/cli\` instead of this package directly.\n`);
}

// The launcher ships twice with identical contents: `${npmScope}/cli` and the
// unscoped `dairo-cli` name (`npx dairo-cli`). The bare `dairo` name is
// blocked by npm's typosquat guard (too similar to dagre/tjiro) and can only
// be granted by npm support.
const launcherScript = `#!/usr/bin/env node
const { spawnSync } = require('node:child_process');
const { existsSync } = require('node:fs');
const { join } = require('node:path');

const platformPackages = {
  'darwin-arm64': '${npmScope}/cli-darwin-arm64',
  'darwin-x64': '${npmScope}/cli-darwin-x64',
  'linux-x64-glibc': '${npmScope}/cli-linux-x64',
  'linux-arm64-glibc': '${npmScope}/cli-linux-arm64',
  'linux-x64-musl': '${npmScope}/cli-linux-x64-musl',
  'linux-arm64-musl': '${npmScope}/cli-linux-arm64-musl',
  'win32-x64': '${npmScope}/cli-win32-x64',
  'win32-arm64': '${npmScope}/cli-win32-arm64',
};

// The gnu binaries are built against glibc 2.35 (ubuntu-22.04 builders).
// On musl systems and on older-glibc distros (Amazon Linux, older Debian)
// the static musl binary is the one that runs.
const MIN_GLIBC = [2, 35];

function linuxFlavor() {
  let glibc = null;
  try {
    glibc = process.report.getReport().header.glibcVersionRuntime || null;
  } catch (_) {}
  if (!glibc) return 'musl';
  const [major = 0, minor = 0] = glibc.split('.').map((n) => parseInt(n, 10) || 0);
  if (major > MIN_GLIBC[0] || (major === MIN_GLIBC[0] && minor >= MIN_GLIBC[1])) return 'glibc';
  return 'musl';
}

const key = process.platform === 'linux'
  ? 'linux-' + process.arch + '-' + linuxFlavor()
  : process.platform + '-' + process.arch;
const pkg = platformPackages[key];
if (!pkg) {
  console.error('Dairo CLI is not available for ' + key + '. Download manually from https://dairo.app/downloads/cli/latest/');
  process.exit(1);
}

let packageJson;
try {
  packageJson = require.resolve(pkg + '/package.json');
} catch (_) {
  console.error('Dairo CLI native package ' + pkg + ' was not installed. Try: npm install -g @dairo-app/cli --include=optional');
  process.exit(1);
}

const exe = process.platform === 'win32' ? 'dairo.exe' : 'dairo';
const binary = join(packageJson, '..', 'bin', exe);
if (!existsSync(binary)) {
  console.error('Dairo CLI binary is missing at ' + binary + '. Reinstall the dairo package.');
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 0);
`;

for (const launcher of [
  { dir: 'cli', name: `${npmScope}/cli` },
  { dir: 'dairo-cli', name: 'dairo-cli' },
]) {
  const rootDir = join(packageRoot, launcher.dir);
  mkdirSync(join(rootDir, 'bin'), { recursive: true });
  writeFileSync(join(rootDir, 'package.json'), JSON.stringify({
    name: launcher.name,
    version,
    description: 'Official Dairo command-line interface',
    license: 'MIT',
    repository: { type: 'git', url: 'git+https://github.com/dairo-app/dairo-cli.git' },
    homepage: 'https://dairo.app',
    bin: { dairo: './bin/dairo.js' },
    files: ['bin', 'README.md'],
    optionalDependencies,
  }, null, 2) + '\n');
  writeFileSync(join(rootDir, 'bin', 'dairo.js'), launcherScript);
  chmodSync(join(rootDir, 'bin', 'dairo.js'), 0o755);
  writeFileSync(join(rootDir, 'README.md'), readFileSync(join(repoRoot, 'README.md'), 'utf8'));
}
console.log(`Prepared Dairo npm packages for ${version} in ${packageRoot}`);

function readCargoVersion(path) {
  const text = readFileSync(path, 'utf8');
  const match = text.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error('Could not read Cargo.toml version');
  return match[1];
}

function findBinary(platform) {
  const candidates = [
    join(binaryRoot, platform.target, platform.exe),
    join(binaryRoot, platform.target, 'dairo'),
    join(binaryRoot, platform.exe),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  throw new Error(`Missing binary for ${platform.target}; checked ${candidates.join(', ')}`);
}
