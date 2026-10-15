'use strict';

// Exercise the packaged, unmarked layout without installing or opening SSH.
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');

const project = path.resolve(__dirname, '..');
const output = path.join(project, 'test-output');
const packageInfo = require('../package.json');
const packaged = path.join(project, 'release', `Lion-SSH-${packageInfo.version}-windows-${process.arch}-portable`);
const reportPath = path.join(output, 'data-directory-smoke.json');
const dataFiles = ['profiles.json', 'credentials.json', 'known-hosts.json', 'settings.json'];

function inspectExistingApps() {
  const command = `
    @(Get-CimInstance Win32_Process -Filter "Name='lion-ssh.exe' OR Name='Lion SSH.exe'" | ForEach-Object {
      [PSCustomObject]@{ pid=$_.ProcessId; parent=$_.ParentProcessId; path=$_.ExecutablePath; startTicks=$_.CreationDate.ToUniversalTime().Ticks.ToString() }
    }) | ConvertTo-Json -Compress
  `;
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', command], { encoding: 'utf8', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error('无法确认现有 Lion SSH 实例，目录验证已停止');
  const parsed = result.stdout.trim() ? JSON.parse(result.stdout) : [];
  const running = Array.isArray(parsed) ? parsed : [parsed];
  return { identities: running, count: running.filter(item => !running.some(parent => parent.pid === item.parent)).length };
}

async function exists(file) {
  try { await fs.access(file); return true; }
  catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}

async function nativeSmoke(executable, cwd, resultFile, verificationRoot, override) {
  const env = { ...process.env, LION_SMOKE_RESULT: resultFile, LION_VERIFY_ROOT: verificationRoot, LION_VERIFY_INSTANCE_ID: randomUUID() };
  delete env.LION_DATA_DIR;
  delete env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS;
  if (override !== undefined) env.LION_DATA_DIR = override;
  const child = spawn(executable, ['--smoke', '--verification-instance'], { cwd, env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', chunk => { stderr = (stderr + chunk.toString()).slice(-4000); });
  child.stdout.on('data', () => {});
  const code = await new Promise((resolve, reject) => {
    let expired = false;
    const timer = setTimeout(() => { expired = true; child.kill(); }, 30_000);
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', code => {
      clearTimeout(timer);
      if (expired) reject(new Error('测试实例没有及时自动退出'));
      else resolve(code);
    });
  });
  assert.equal(code, 0, `原生目录验证启动失败：${stderr}`);
  const result = JSON.parse(await fs.readFile(resultFile, 'utf8'));
  assert.equal(result.ok, true, result.error);
  return result.initial;
}

async function main() {
  if (process.platform !== 'win32') throw new Error('此打包目录验证需要 Windows');
  const before = inspectExistingApps();
  await fs.mkdir(output, { recursive: true });
  const fixture = await fs.mkdtemp(path.join(output, 'data-directory-smoke-'));
  const installed = path.join(fixture, 'installed-layout');
  const foreignCwd = path.join(fixture, 'unrelated-working-directory');
  const results = path.join(fixture, 'native-results');
  const adjacentData = path.join(installed, 'data');
  const overrideData = path.join(fixture, 'explicit-override');
  const report = { ok: false, version: packageInfo.version, existingInstances: before.count, isolatedVerificationNamespace: true, temporaryDataRemoved: false };
  try {
    await fs.mkdir(installed);
    await fs.mkdir(foreignCwd);
    await fs.mkdir(results);
    await fs.copyFile(path.join(packaged, 'Lion SSH.exe'), path.join(installed, 'lion-ssh.exe'));
    assert.equal(await exists(path.join(packaged, 'backend')), false);
    assert.equal(await exists(path.join(packaged, 'node.exe')), false);
    assert.equal(await exists(path.join(installed, 'portable.json')), false);
    await fs.mkdir(adjacentData);
    const profile = { id: 'preserved-installed-profile', name: '已有设备配置', host: '192.0.2.10', port: 22, username: 'fixture', authType: 'key', keyPath: 'C:\\fixture-only\\id_test_key', autoReconnect: false, tunnels: [] };
    const seeded = {
      'profiles.json': JSON.stringify([profile], null, 2),
      'credentials.json': JSON.stringify({ [profile.id]: 'old-format-fixture-ciphertext' }, null, 2),
      'known-hosts.json': JSON.stringify({ '192.0.2.10:22': ['SHA256:fixture-only-host-key'] }, null, 2),
      'settings.json': JSON.stringify({ theme: 'dark' }, null, 2),
    };
    await Promise.all(Object.entries(seeded).map(([name, value]) => fs.writeFile(path.join(adjacentData, name), value)));
    const initial = await nativeSmoke(path.join(installed, 'lion-ssh.exe'), foreignCwd, path.join(results, 'default.json'), fixture);
    assert.equal(initial.profiles.length, 1);
    assert.equal(initial.profiles[0].id, profile.id);
    assert.equal(initial.profiles[0].host, profile.host);
    assert.equal(initial.profiles[0].hasSavedSecret, true);
    assert.equal(initial.theme.preference, 'dark');
    assert.ok((await fs.stat(path.join(adjacentData, 'webview'))).isDirectory());
    assert.equal(await exists(path.join(foreignCwd, 'data')), false);
    for (const [name, value] of Object.entries(seeded)) assert.equal(await fs.readFile(path.join(adjacentData, name), 'utf8'), value, `${name} should stay byte-identical`);
    report.unmarkedInstalledLayout = { adjacentProfileLoaded: true, adjacentThemeLoaded: true, oldFormatFilesPreserved: true, adjacentWebviewCreated: true, workingDirectoryDataCreated: false };

    const overridden = await nativeSmoke(path.join(installed, 'lion-ssh.exe'), foreignCwd, path.join(results, 'override.json'), fixture, overrideData);
    assert.deepEqual(overridden.profiles, []);
    assert.equal(overridden.theme.preference, 'system');
    for (const name of dataFiles) JSON.parse(await fs.readFile(path.join(overrideData, name), 'utf8'));
    assert.deepEqual(JSON.parse(await fs.readFile(path.join(overrideData, 'profiles.json'), 'utf8')), []);
    assert.ok((await fs.stat(path.join(overrideData, 'webview'))).isDirectory());
    assert.equal(await exists(path.join(foreignCwd, 'data')), false);
    for (const [name, value] of Object.entries(seeded)) assert.equal(await fs.readFile(path.join(adjacentData, name), 'utf8'), value, `override must not modify adjacent ${name}`);
    report.explicitOverride = { higherPriority: true, freshProfilesEmpty: true, fourJsonFilesInitialized: true, webviewCreated: true, adjacentFilesPreserved: true, workingDirectoryDataCreated: false };

    const after = inspectExistingApps();
    assert.deepEqual(after.identities.sort((a, b) => a.pid - b.pid), before.identities.sort((a, b) => a.pid - b.pid));
    report.existingInstancesPreserved = true;
    const resolved = path.resolve(fixture);
    assert.equal(path.dirname(resolved).toLowerCase(), output.toLowerCase());
    assert.ok(path.basename(resolved).startsWith('data-directory-smoke-'));
    // WebView2 may release its cache locks shortly after the app has exited.
    await fs.rm(resolved, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    report.temporaryDataRemoved = true;
    report.ok = true;
  } catch (error) {
    report.fixturePreservedForInspection = fixture;
    report.error = error.stack || String(error);
    throw error;
  } finally {
    await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
  }
  console.log(`数据目录验证通过：${reportPath}`);
}

if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });
module.exports = { main };
