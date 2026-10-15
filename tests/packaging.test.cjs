const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { createHash } = require('node:crypto');
const { zipSync, unzipSync } = require('fflate');
const { distributionFiles, removeManifestFiles, parseArguments, windowsArchitecture, buildDirectory } = require('../scripts/package.cjs');
const { buildEnvironment } = require('../scripts/tauri.cjs');
const { releaseArtifacts, requiredAssets, signatureVersion, validate, makeUpdaterJson } = require('../scripts/ci-release.cjs');

test('portable archive excludes existing credentials, user-added files and stale runtime files', async () => {
  const output = path.join(__dirname, '..', 'test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'package-privacy-'));
  try {
    const portable = path.join(dir, 'portable');
    const resources = path.join(dir, 'resources');
    await fs.mkdir(path.join(portable, 'data'), { recursive: true });
    await fs.mkdir(path.join(portable, 'backend'), { recursive: true });
    await fs.mkdir(resources);
    await fs.writeFile(path.join(portable, 'data', 'credentials.json'), 'PRIVATE CREDENTIALS');
    await fs.writeFile(path.join(portable, 'my-private-key'), 'PRIVATE KEY');
    await fs.writeFile(path.join(portable, 'backend', 'stale-user-file'), 'PRIVATE NOTES');
    for (const file of ['Lion SSH.exe', 'LICENSE.txt', 'portable.json', '使用说明.txt']) await fs.writeFile(path.join(portable, file), file);
    await fs.writeFile(path.join(resources, 'THIRD-PARTY-NOTICES.txt'), 'LICENSES');
    const archive = unzipSync(zipSync(await distributionFiles(portable, resources)));
    assert.deepEqual(Object.keys(archive).sort(), ['LICENSE.txt', 'Lion SSH.exe', 'licenses/THIRD-PARTY-NOTICES.txt', 'portable.json', '使用说明.txt'].sort());
    assert.equal(await fs.readFile(path.join(portable, 'data', 'credentials.json'), 'utf8'), 'PRIVATE CREDENTIALS');
  } finally { await fs.rm(dir, { recursive: true, force: true }); }
});

test('portable upgrade removes old runtime files while retaining data and added files', async () => {
  const output = path.join(__dirname, '..', 'test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'package-upgrade-'));
  try {
    await fs.mkdir(path.join(dir, 'backend', 'dependencies'), { recursive: true });
    await fs.mkdir(path.join(dir, 'data'));
    await fs.writeFile(path.join(dir, 'backend', 'node.exe'), 'OLD NODE');
    await fs.writeFile(path.join(dir, 'backend', 'dependencies', 'ssh.cjs'), 'OLD BACKEND');
    await fs.writeFile(path.join(dir, 'keep.txt'), 'USER FILE');
    await fs.writeFile(path.join(dir, 'data', 'credentials.json'), 'PRIVATE');
    await removeManifestFiles(dir, { files: [{ path: 'backend/node.exe' }, { path: 'backend/dependencies/ssh.cjs' }] });
    await assert.rejects(fs.stat(path.join(dir, 'backend')), { code: 'ENOENT' });
    assert.equal(await fs.readFile(path.join(dir, 'data', 'credentials.json'), 'utf8'), 'PRIVATE');
    assert.equal(await fs.readFile(path.join(dir, 'keep.txt'), 'utf8'), 'USER FILE');
    for (const unsafe of ['../outside.txt', './data/credentials.json', 'backend/../data/credentials.json', '.']) {
      await assert.rejects(removeManifestFiles(dir, { files: [{ path: unsafe }] }), /不安全/);
    }
  } finally { await fs.rm(dir, { recursive: true, force: true }); }
});

test('Tauri child process receives one PATH key with Node, Cargo and inherited tools', () => {
  const env = buildEnvironment();
  assert.deepEqual(Object.keys(env).filter(key => key.toLowerCase() === 'path'), ['PATH']);
  assert.ok(env.PATH.split(path.delimiter).includes(path.dirname(process.execPath)));
  const original = Object.keys(process.env).filter(key => key.toLowerCase() === 'path').map(key => process.env[key]).find(Boolean);
  if (original) assert.ok(env.PATH.endsWith(original));
});

test('portable packaging uses the requested Rust target instead of the build machine architecture', () => {
  assert.equal(windowsArchitecture('x86_64-pc-windows-msvc', 'arm64'), 'x64');
  assert.equal(windowsArchitecture('aarch64-pc-windows-msvc', 'x64'), 'arm64');
  assert.equal(windowsArchitecture('i686-pc-windows-gnu', 'x64'), 'ia32');
  const parsed = parseArguments(['--skip-build', '--sign-portable', '--target', 'x86_64-pc-windows-msvc']);
  assert.equal(parsed.skipBuild, true);
  assert.equal(parsed.signPortable, true);
  assert.equal(parsed.target, 'x86_64-pc-windows-msvc');
  assert.ok(buildDirectory(path.resolve(__dirname, '..'), parsed.target).endsWith(path.join(parsed.target, 'release')));
  for (const args of [['--target'], ['--target', '../../private'], ['--target', 'x86_64-unknown-linux-gnu'], ['--unknown']]) assert.throws(() => parseArguments(args));
});

function encodedSignature(version, extra = '') {
  return Buffer.from(`untrusted comment: fixture signature\nRWFAKE\ntrusted comment: timestamp:123\tfile:fixture.zip\tversion:${version}${extra}\nGLOBAL\n`).toString('base64');
}

test('release metadata covers native platforms and the separately signed portable ZIP', async () => {
  const version = '1.7.0';
  const assets = requiredAssets(version).map((name, index) => ({ id: index + 1, name, size: name.endsWith('.sig') ? 200 : 4096 }));
  const metadata = await makeUpdaterJson({ version, tag: 'v1.7.0', repository: 'ZjDing989/LionSSH', assets,
    publishedAt: '2026-10-15T08:00:00Z', readSignature: async () => encodedSignature(version) });
  assert.equal(metadata.version, version);
  for (const target of ['windows-x86_64', 'windows-x86_64-portable', 'darwin-aarch64', 'darwin-x86_64', 'linux-x86_64']) {
    assert.ok(metadata.platforms[target].url.startsWith('https://github.com/ZjDing989/LionSSH/releases/download/v1.7.0/'));
    assert.equal(metadata.platforms[target].signature, encodedSignature(version));
  }
  assert.ok(metadata.platforms['windows-x86_64'].url.endsWith('-setup.exe'));
  assert.ok(metadata.platforms['windows-x86_64-portable'].url.endsWith('-portable.zip'));
  assert.ok(metadata.platforms['darwin-aarch64'].url.endsWith('-macos-aarch64.app.tar.gz'));
  assert.ok(metadata.platforms['darwin-x86_64'].url.endsWith('-macos-x86_64.app.tar.gz'));
  assert.ok(metadata.platforms['linux-x86_64'].url.endsWith('-linux-x86_64.AppImage'));
  assert.deepEqual(metadata.platforms['windows-x86_64-nsis'], metadata.platforms['windows-x86_64']);
});

test('release finalization rejects missing architectures, empty assets and mismatched signed versions', async () => {
  const version = '1.7.0';
  const assets = requiredAssets(version).map((name, index) => ({ id: index + 1, name, size: 200 }));
  const input = { version, tag: 'v1.7.0', repository: 'ZjDing989/LionSSH', assets, readSignature: async () => encodedSignature(version) };
  await assert.rejects(makeUpdaterJson({ ...input, assets: assets.filter(asset => !asset.name.includes('macos-aarch64')) }), /Missing/);
  await assert.rejects(makeUpdaterJson({ ...input, assets: assets.map((asset, index) => index ? asset : { ...asset, size: 0 }) }), /empty/);
  await assert.rejects(makeUpdaterJson({ ...input, assets: [...assets, assets[0]] }), /Duplicate/);
  await assert.rejects(makeUpdaterJson({ ...input, readSignature: async () => encodedSignature('1.6.2') }), /bound/);
  await assert.rejects(makeUpdaterJson({ ...input, repository: '../different/repository' }), /repository/);
  await assert.rejects(makeUpdaterJson({ ...input, serverUrl: 'http://github.com' }), /HTTPS/);
  await assert.rejects(makeUpdaterJson({ ...input, tag: 'v1.6.2' }), /mismatch/);
  assert.throws(() => signatureVersion(encodedSignature(version, '\tversion:1.6.2'), version), /bound/);
  assert.throws(() => signatureVersion(Buffer.from('untrusted comment: version:1.7.0\nFAKE').toString('base64'), version), /trusted comment/);
  assert.throws(() => signatureVersion('https://example.com/signature.sig', version), /encoding/);
  assert.equal(releaseArtifacts(version).length, 5);
});

test('release preflight requires one version/tag and an updater public key before publishing', async () => {
  const output = path.join(__dirname, '..', 'test-output');
  await fs.mkdir(output, { recursive: true });
  const fixture = await fs.mkdtemp(path.join(output, 'release-preflight-'));
  try {
    await fs.mkdir(path.join(fixture, 'src-tauri'));
    await fs.writeFile(path.join(fixture, 'package.json'), JSON.stringify({ version: '1.7.0' }));
    await fs.writeFile(path.join(fixture, 'src-tauri', 'Cargo.toml'), '[package]\nname = "lion-ssh"\nversion = "1.7.0"\n\n[dependencies]\n');
    const config = { version: '1.7.0', bundle: { createUpdaterArtifacts: true }, plugins: { updater: { pubkey: 'test-public-key', requireSignedVersion: true } } };
    await fs.writeFile(path.join(fixture, 'src-tauri', 'tauri.conf.json'), JSON.stringify(config));
    assert.equal((await validate(fixture, 'v1.7.0')).version, '1.7.0');
    await assert.rejects(validate(fixture, 'v1.6.2'), /tag/);
    config.version = '1.7.1';
    await fs.writeFile(path.join(fixture, 'src-tauri', 'tauri.conf.json'), JSON.stringify(config));
    await assert.rejects(validate(fixture, 'v1.7.0'), /versions must match/);
    config.version = '1.7.0'; config.plugins.updater.pubkey = 'YOUR_PUBLIC_KEY';
    await fs.writeFile(path.join(fixture, 'src-tauri', 'tauri.conf.json'), JSON.stringify(config));
    await assert.rejects(validate(fixture, 'v1.7.0'), /public key/);
    config.plugins.updater.pubkey = 'test-public-key'; config.plugins.updater.requireSignedVersion = false;
    await fs.writeFile(path.join(fixture, 'src-tauri', 'tauri.conf.json'), JSON.stringify(config));
    await assert.rejects(validate(fixture, 'v1.7.0'), /requireSignedVersion/);
  } finally { await fs.rm(fixture, { recursive: true, force: true }); }
});

test('terminal runtime styles are permitted without disabling script CSP protection', async () => {
  const config = JSON.parse(await fs.readFile(path.join(__dirname, '..', 'src-tauri', 'tauri.conf.json'), 'utf8'));
  const security = config.app.security;
  assert.deepEqual(security.dangerousDisableAssetCspModification, ['style-src'], 'Only styles may opt out of automatic CSP nonce/hash modification');
  for (const policy of [security.csp, security.devCsp]) {
    const directives = new Map(policy.split(';').filter(item => item.trim()).map(item => {
      const [name, ...sources] = item.trim().split(/\s+/);
      return [name, sources];
    }));
    assert.ok(directives.get('style-src').includes("'unsafe-inline'"), 'xterm generates its theme, dimensions and cursor CSS at runtime');
    assert.deepEqual(directives.get('script-src'), ["'self'"], 'Script execution stays restricted to bundled assets and Tauri-injected hashes');
    assert.deepEqual(directives.get('object-src'), ["'none'"]);
    assert.deepEqual(directives.get('frame-src'), ["'none'"]);
  }
});

test('custom NSIS default keeps the user-selectable directory page and existing install location', async () => {
  const template = await fs.readFile(path.join(__dirname, '..', 'src-tauri', 'windows', 'installer.nsi'), 'utf8');
  assert.ok(template.includes('tauri-cli-v2.12.3/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi'));
  assert.ok(template.includes('StrCpy $INSTDIR "$LOCALAPPDATA\\Programs\\${PRODUCTNAME}"'));
  assert.ok(template.includes('${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"'));
  assert.ok(template.includes('Call RestorePreviousInstallLocation'));
  assert.ok(template.includes('!insertmacro MUI_PAGE_DIRECTORY'));
  const restored = template.slice(template.indexOf('Unicode true')).replace(/\r\n?/g, '\n')
    .replace('StrCpy $INSTDIR "$LOCALAPPDATA\\Programs\\${PRODUCTNAME}"', 'StrCpy $INSTDIR "$LOCALAPPDATA\\${PRODUCTNAME}"');
  assert.equal(createHash('sha256').update(restored).digest('hex'), '8ae02c66f8adb4a0d15d97f984fbacd2b44262d8a4d03fefaf6177f67f7008e5', 'Only the default directory line and attribution comments may differ from the pinned official template');
});
