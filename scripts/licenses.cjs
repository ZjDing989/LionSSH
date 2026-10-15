'use strict';

// Build-time only. The installed application never invokes Node.js or Cargo.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { buildEnvironment } = require('./tauri.cjs');

const root = path.resolve(__dirname, '..');
const outputDirectory = path.join(root, 'src-tauri', 'resources', 'licenses');
const NOTICE_FILE = 'THIRD-PARTY-NOTICES.txt';
const TEXT_FILE = 'LICENSE-TEXTS.txt';
const MANIFEST_FILE = 'manifest.json';
const sourceDirectory = path.join(__dirname, 'license-sources');
const licenseName = /^(?:licen[cs]e(?:[-_. ].*)?|copying(?:[-_. ].*)?|notice(?:[-_. ].*)?|copyright(?:[-_. ].*)?|unlicense(?:[-_. ].*)?)$/i;
const licenseDirectory = /^(?:licen[cs]es?|notices?|copyrights?)$/i;

function run(command, args, environment) {
  const result = spawnSync(command, args, {
    cwd: root,
    env: environment,
    encoding: 'utf8',
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw new Error(`${command} failed: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed:\n${result.stderr || result.stdout}`);
  return result.stdout;
}

function digest(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

function readText(file) {
  const bytes = fs.readFileSync(file);
  if (bytes.includes(0)) throw new Error(`A license file contains binary data: ${file}`);
  // Normalize line endings and a possible BOM for reproducible output across
  // Windows and Unix registries. All other license text is retained verbatim.
  return bytes.toString('utf8').replace(/^\uFEFF/, '').replace(/\r\n?/g, '\n');
}

function licenseFiles(packageRoot, declaredFile) {
  const selected = new Set();
  const explicit = declaredFile ? path.resolve(packageRoot, declaredFile) : null;
  if (explicit) {
    if (!fs.statSync(explicit).isFile()) throw new Error(`Declared license file is missing: ${explicit}`);
    selected.add(explicit);
  }
  function walk(directory, insideLicenses = false) {
    const entries = fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name, 'en'));
    for (const entry of entries) {
      if (entry.isSymbolicLink()) continue;
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        if (['.git', 'target', 'node_modules'].includes(entry.name)) continue;
        walk(file, insideLicenses || licenseDirectory.test(entry.name));
      } else if (entry.isFile()) {
        // Rust source files named license.rs are code rather than license text.
        const sourceFile = /\.(?:rs|c|cc|cpp|h|hpp|py|js|ts|png|jpe?g|gif|ico|woff2?|ttf|pdf|bin|a|lib|dll|exe)$/i.test(entry.name);
        if (!sourceFile && (insideLicenses || licenseName.test(entry.name))) selected.add(file);
      }
    }
  }
  walk(packageRoot);
  return [...selected].sort((a, b) => path.relative(packageRoot, a).localeCompare(path.relative(packageRoot, b), 'en'));
}

function generate(options = {}) {
  const environment = buildEnvironment();
  const host = run('rustc', ['-vV'], environment).match(/^host:\s*(\S+)/m)?.[1];
  if (!host) throw new Error('Cannot determine the Rust host target');
  const target = options.target || environment.CARGO_BUILD_TARGET || host;
  const metadata = JSON.parse(run('cargo', [
    'metadata', '--manifest-path', 'src-tauri/Cargo.toml', '--locked',
    '--format-version', '1', '--filter-platform', target,
  ], environment));
  const included = new Set(metadata.resolve.nodes.map(node => node.id));
  const workspace = new Set(metadata.workspace_members);
  const packages = metadata.packages
    .filter(pkg => included.has(pkg.id) && !workspace.has(pkg.id))
    .sort((a, b) => a.name.localeCompare(b.name, 'en') || a.version.localeCompare(b.version, 'en') || a.id.localeCompare(b.id, 'en'));
  const texts = new Map();
  const supplements = JSON.parse(readText(path.join(sourceDirectory, 'manifest.json'))).supplements;
  const cargoEntries = packages.map(pkg => {
    const packageRoot = path.dirname(pkg.manifest_path);
    const supplemental = supplements.filter(item => item.packages.includes(`${pkg.name}@${pkg.version}`));
    const sourceFiles = licenseFiles(packageRoot, pkg.license_file).map(file => ({
      file, relativePath: path.relative(packageRoot, file).split(path.sep).join('/'),
    }));
    for (const supplement of supplemental) {
      const vcs = JSON.parse(readText(path.join(packageRoot, '.cargo_vcs_info.json')));
      if (vcs.git.sha1 !== supplement.commit) throw new Error(`Supplemental license commit mismatch: ${pkg.name} ${pkg.version}`);
      for (const source of supplement.files) {
        const file = path.resolve(sourceDirectory, source.path);
        if (!file.startsWith(`${sourceDirectory}${path.sep}`)) throw new Error(`Invalid supplemental license path: ${source.path}`);
        if (digest(fs.readFileSync(file)) !== source.sha256) throw new Error(`Supplemental license checksum mismatch: ${source.path}`);
        sourceFiles.push({ file, relativePath: `upstream/${source.upstreamPath}`, source: source.url, commit: supplement.commit });
      }
    }
    const files = sourceFiles.map(({ file, relativePath, source, commit }) => {
      const content = readText(file);
      const sha256 = digest(content);
      const owner = `Cargo / ${pkg.name} ${pkg.version} / ${relativePath}`;
      if (!texts.has(sha256)) texts.set(sha256, { content, owners: [] });
      texts.get(sha256).owners.push(owner);
      return { path: relativePath, sha256, ...(source ? { source, commit } : {}) };
    });
    return {
      ecosystem: 'cargo',
      name: pkg.name,
      version: pkg.version,
      license: pkg.license || null,
      authors: pkg.authors || [],
      repository: pkg.repository || null,
      source: `https://crates.io/crates/${encodeURIComponent(pkg.name)}/${encodeURIComponent(pkg.version)}`,
      licenseNotes: supplemental.map(item => item.note).filter(Boolean),
      files,
    };
  });
  const npmLock = JSON.parse(readText(path.join(root, 'package-lock.json')));
  if (!npmLock.packages) throw new Error('package-lock.json must include a packages inventory; run npm install');
  const npmEntries = Object.entries(npmLock.packages)
    .filter(([location, pkg]) => location.startsWith('node_modules/') && pkg.dev !== true)
    .map(([location, locked]) => {
      const packageRoot = path.resolve(root, location);
      if (!packageRoot.startsWith(`${root}${path.sep}node_modules${path.sep}`)) throw new Error(`Invalid npm package location: ${location}`);
      const packageJson = path.join(packageRoot, 'package.json');
      if (!fs.existsSync(packageJson)) throw new Error(`Production npm package is missing; run npm ci: ${location}`);
      const pkg = JSON.parse(readText(packageJson));
      if (pkg.version !== locked.version) throw new Error(`Installed npm package does not match package-lock.json: ${location}`);
      const files = licenseFiles(packageRoot).map(file => {
        const content = readText(file);
        const sha256 = digest(content);
        const relativePath = path.relative(packageRoot, file).split(path.sep).join('/');
        const owner = `npm / ${pkg.name} ${pkg.version} / ${relativePath}`;
        if (!texts.has(sha256)) texts.set(sha256, { content, owners: [] });
        texts.get(sha256).owners.push(owner);
        return { path: relativePath, sha256 };
      });
      const person = value => typeof value === 'string' ? value : value && [value.name, value.email && `<${value.email}>`, value.url].filter(Boolean).join(' ');
      const authors = [...new Set([pkg.author, ...(pkg.contributors || [])].map(person).filter(Boolean))];
      const license = pkg.license || locked.license;
      return {
        ecosystem: 'npm',
        name: pkg.name,
        version: pkg.version,
        license: typeof license === 'string' ? license : license?.type || null,
        authors,
        repository: typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url || null,
        source: `https://www.npmjs.com/package/${pkg.name}/v/${pkg.version}`,
        archive: locked.resolved || null,
        integrity: locked.integrity || null,
        files,
      };
    })
    .sort((a, b) => a.name.localeCompare(b.name, 'en') || a.version.localeCompare(b.version, 'en'));
  const entries = [...cargoEntries, ...npmEntries];
  const missing = entries.filter(entry => entry.files.length === 0);
  if (missing.length) {
    throw new Error(`Dependency archives lack license/notice files: ${missing.map(entry => `${entry.name} ${entry.version} (${entry.license || 'no license expression'})`).join(', ')}`);
  }
  const lines = [
    'Lion SSH - Third-Party Notices',
    '==============================',
    '',
    `Target platform: ${target}`,
    `Cargo dependency packages: ${cargoEntries.length}`,
    `Frontend production npm packages: ${npmEntries.length}`,
    `Total dependency packages: ${entries.length}`,
    '',
    'This inventory is generated from the locked Cargo dependency graph for the',
    'target platform and the production npm packages in package-lock.json.',
    'Cargo build-time and development dependencies are also included; inclusion',
    'does not imply that every package is linked into the distributed executable.',
    'Development-only npm packages are excluded.',
    '',
    'The original license expressions and package authors below are package',
    'metadata. Copyright holders and additional attribution requirements appear',
    'in the complete LICENSE, COPYING, NOTICE, and COPYRIGHT texts reproduced in',
    `${TEXT_FILE}. Each referenced text is identified by its SHA-256 digest.`,
    'Identical texts are stored once with all package/file origins listed.',
    'Where a published archive omits its license file, a checked-in upstream',
    'copy is used after verifying the dependency source commit and text checksum.',
    '',
    'Dependency source archives are available from the package links below.',
    'The Lion SSH application license is distributed separately as LICENSE.txt.',
    'The ChatGPT icon source and license are in ChatGPT-logo-LICENSE.txt.',
    '',
  ];
  for (const entry of entries) {
    lines.push(`[${entry.ecosystem}] ${entry.name} ${entry.version}`, '-'.repeat(72));
    lines.push(`License expression: ${entry.license || '(see license file)'}`);
    if (entry.authors.length) lines.push(`Package authors: ${entry.authors.join('; ')}`);
    lines.push(`Source package: ${entry.source}`);
    if (entry.repository) lines.push(`Repository: ${entry.repository}`);
    for (const note of entry.licenseNotes || []) lines.push(`License note: ${note}`);
    for (const file of entry.files) {
      lines.push(`Text: ${file.path}\nSHA-256: ${file.sha256}`);
      if (file.source) lines.push(`Upstream license: ${file.source}\nDependency commit: ${file.commit}`);
    }
    lines.push('');
  }
  const fullTexts = [
    'Lion SSH - Complete Third-Party License and Notice Texts',
    '======================================================',
    '',
    `See ${NOTICE_FILE} for package versions, license expressions, and source links.`,
    '',
  ];
  for (const [sha256, item] of [...texts].sort(([a], [b]) => a.localeCompare(b, 'en'))) {
    fullTexts.push('='.repeat(80), `SHA-256: ${sha256}`, 'Origins:');
    fullTexts.push(...item.owners.sort((a, b) => a.localeCompare(b, 'en')).map(owner => `  ${owner}`));
    fullTexts.push('-'.repeat(80), item.content, '');
  }
  const manifest = {
    format: 1,
    target,
    cargoLockSha256: digest(fs.readFileSync(path.join(root, 'src-tauri', 'Cargo.lock'))),
    npmLockSha256: digest(fs.readFileSync(path.join(root, 'package-lock.json'))),
    packageCount: entries.length,
    cargoPackageCount: cargoEntries.length,
    npmProductionPackageCount: npmEntries.length,
    uniqueTextCount: texts.size,
    packages: entries,
  };
  const outputs = new Map([
    [NOTICE_FILE, `${lines.join('\n')}\n`],
    [TEXT_FILE, `${fullTexts.join('\n')}\n`],
    [MANIFEST_FILE, `${JSON.stringify(manifest, null, 2)}\n`],
    ['ChatGPT-logo-LICENSE.txt', readText(path.join(root, 'assets', 'chatgpt-logo-LICENSE.txt'))],
  ]);
  if (options.check) {
    for (const [name, content] of outputs) {
      const file = path.join(outputDirectory, name);
      if (!fs.existsSync(file) || fs.readFileSync(file, 'utf8') !== content) {
        throw new Error(`Generated dependency notices are missing or stale: ${file}`);
      }
    }
  } else {
    fs.mkdirSync(outputDirectory, { recursive: true });
    for (const [name, content] of outputs) fs.writeFileSync(path.join(outputDirectory, name), content, 'utf8');
  }
  return { target, packageCount: entries.length, cargoPackageCount: cargoEntries.length, npmProductionPackageCount: npmEntries.length, uniqueTextCount: texts.size, files: [...outputs.keys()] };
}

if (require.main === module) {
  try {
    const args = process.argv.slice(2);
    const targetAt = args.indexOf('--target');
    if (targetAt !== -1 && !args[targetAt + 1]) throw new Error('--target requires a Rust target triple');
    const result = generate({ check: args.includes('--check'), target: targetAt === -1 ? undefined : args[targetAt + 1] });
    console.log(`${args.includes('--check') ? 'Verified' : 'Generated'} license notices for ${result.cargoPackageCount} Cargo crates and ${result.npmProductionPackageCount} production npm packages (${result.uniqueTextCount} unique texts), ${result.target}`);
    for (const name of result.files) console.log(path.join(outputDirectory, name));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

module.exports = { generate };
