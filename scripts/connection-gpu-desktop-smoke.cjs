'use strict';

// Verify the distributed Windows executable with isolated loopback SSH fixtures.
// The pipe harness exercises graceful shutdown in the same packaged Rust
// service; the native WebView2 launch then restores and renders those sessions.
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const { chromium } = require('playwright');
const { ServiceHarness } = require('../tests/helpers/service-harness.cjs');
const { localSshServer, echoServer, freePort, roundTrip } = require('../tests/helpers/local-ssh.cjs');
const { profile, tunnel, delay } = require('../tests/helpers/fixtures.cjs');

const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');
const version = require('../package.json').version;
const executable = path.join(root, 'release', `Lion-SSH-${version}-windows-${process.arch}-portable`, 'Lion SSH.exe');
const reportPath = path.join(output, 'connection-gpu-desktop-smoke.json');

function existingApps() {
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', `
    @(Get-CimInstance Win32_Process -Filter "Name='lion-ssh.exe' OR Name='Lion SSH.exe'" | ForEach-Object {
      [PSCustomObject]@{ pid=$_.ProcessId; parent=$_.ParentProcessId; path=$_.ExecutablePath; startTicks=$_.CreationDate.ToUniversalTime().Ticks.ToString() }
    }) | ConvertTo-Json -Compress
  `], { encoding: 'utf8', windowsHide: true });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, 'existing application inventory must be available');
  const parsed = result.stdout.trim() ? JSON.parse(result.stdout) : [];
  return (Array.isArray(parsed) ? parsed : [parsed]).sort((a, b) => a.pid - b.pid);
}

async function launch(dataDir, verification, errors) {
  const port = await freePort();
  let exited = false;
  let log = '';
  const child = spawn(executable, ['--verification-instance'], {
    cwd: path.dirname(executable), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, LION_DATA_DIR: dataDir, LION_VERIFY_ROOT: verification.root,
      LION_VERIFY_INSTANCE_ID: verification.id,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` },
  });
  const closed = new Promise(resolve => child.once('close', code => { exited = true; resolve(code); }));
  child.on('error', error => { log += error.message; });
  child.stdout.on('data', data => { log += data.toString(); });
  child.stderr.on('data', data => { log += data.toString(); });
  let browser;
  const stop = async () => {
    // This is only our spawned verification instance. All intentional session
    // shutdown assertions use the pipe harness; GUI cleanup is explicit below.
    if (!exited) child.kill();
    await closed;
    await browser?.close().catch(() => {});
  };
  try {
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline) {
      if (exited) throw new Error(`Native fixture exited during startup: ${log}`);
      try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
      catch { await delay(150); }
    }
    assert.ok(browser, `Native WebView2 debugging endpoint did not start: ${log}`);
    let page;
    while (Date.now() < deadline) {
      page = browser.contexts().flatMap(context => context.pages()).find(candidate => candidate.url() !== 'about:blank');
      if (page) break;
      await delay(100);
    }
    assert.ok(page, 'native application page must exist');
    const consoleErrors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
    await page.waitForFunction(() => Boolean(window.__TAURI_INTERNALS__?.invoke), null, { timeout: 15_000 });
    await page.locator('.app-shell').waitFor({ state: 'visible', timeout: 20_000 });
    const invoke = (method, ...args) => page.evaluate(({ method, args }) => window.__TAURI_INTERNALS__.invoke('lion_request', { method, args }), { method, args });
    return { page, invoke, stop, consoleErrors };
  } catch (error) { await stop(); throw error; }
}

async function pollState(app, id, predicate, description) {
  const deadline = Date.now() + 25_000;
  let state;
  while (Date.now() < deadline) {
    state = (await app.invoke('initial')).states.find(candidate => candidate.profileId === id);
    if (state && predicate(state)) return state;
    await delay(100);
  }
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(state)}`);
}

async function connect(backend, input, secrets) {
  await backend.invoke('save-profile', input, secrets);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id, secrets);
  const first = await backend.waitFor(messages => messages.slice(watermark).find(message =>
    message.payload?.profileId === input.id && (message.event === 'lion:host-key' ||
      (message.event === 'lion:state' && message.payload.status === 'connected'))), 'connection or host-key prompt');
  if (first.event === 'lion:host-key') await backend.invoke('trust-host', input.id, true, first.payload.fingerprint);
  await backend.state(input.id, state => state.status === 'connected', 'fixture authentication');
}

async function remembered(dataDir) {
  const settings = JSON.parse(await fs.readFile(path.join(dataDir, 'settings.json'), 'utf8'));
  return [...(settings.connectedProfileIds || [])].sort();
}

async function main() {
  assert.equal(process.platform, 'win32', 'this packaged UI verification requires Windows');
  await fs.access(executable);
  const inventory = existingApps();
  await fs.mkdir(output, { recursive: true });
  const fixture = await fs.mkdtemp(path.join(output, 'connection-gpu-desktop-fixture-'));
  const dataDir = path.join(fixture, 'data');
  await fs.mkdir(dataDir);
  const verification = { id: randomUUID(), root: fixture };
  const errors = [];
  const report = { ok: false, version, executable, dataDir, isolatedVerificationNamespace: true, rendererErrors: errors };
  const servers = [];
  let backend;
  let app;
  const activeIds = ['packaged-saved-password', 'packaged-key'];
  try {
    const password = `fixture-${randomUUID()}`;
    const unsavedSsh = await localSshServer({ password }); servers.push(unsavedSsh);
    const savedSsh = await localSshServer({ password,
      gpuRows: '0, GPU-busy, NVIDIA Test Card, 24576, 18119, 65\n1, GPU-idle, NVIDIA Test Card, 24576, 15, 0\n',
      computeRows: 'GPU-busy, 4242, training, 18119\n' }); servers.push(savedSsh);
    const keySsh = await localSshServer({ password }); servers.push(keySsh);
    const echo = await echoServer(); servers.push(echo);
    const savedPort = await freePort();
    const keyPort = await freePort();
    const unsaved = [0, 1].map(index => profile(unsavedSsh, { id: `packaged-unsaved-${index}`, name: `未保存密码 ${index + 1}`, autoReconnect: false }));
    const saved = profile(savedSsh, { id: activeIds[0], name: '已保存密码 GPU 服务器', autoReconnect: false,
      gpu: { enabled: true, intervalSec: 2, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 1 },
      tunnels: [tunnel('packaged-saved-tunnel', savedPort, echo.port)] });
    const key = profile(keySsh, { id: activeIds[1], name: '密钥服务器', authType: 'key', keyPath: keySsh.keyPath, autoReconnect: false,
      tunnels: [tunnel('packaged-key-tunnel', keyPort, echo.port)] });
    backend = new ServiceHarness(dataDir, { binary: executable });
    await backend.invoke('initial');
    await backend.invoke('set-language', 'zh');
    await connect(backend, unsaved[0], { password, remember: false });
    await connect(backend, saved, { password, remember: true });
    await connect(backend, unsaved[1], { password, remember: false });
    await connect(backend, key, {});
    const expectedRemembered = [...unsaved.map(input => input.id), ...activeIds].sort();
    assert.deepEqual(await remembered(dataDir), expectedRemembered);
    const credentials = await fs.readFile(path.join(dataDir, 'credentials.json'), 'utf8');
    assert.equal(credentials.includes(password), false);
    assert.deepEqual(Object.keys(JSON.parse(credentials)), [saved.id]);
    assert.match(JSON.parse(credentials)[saved.id], /^native:v1:/);
    for (const input of [saved, key]) await backend.state(input.id, state => state.tunnels.length === 1 && state.tunnels[0].status === 'running', 'original tunnel listener');
    assert.equal(await roundTrip(savedPort, 'packaged-before-exit'), 'packaged-before-exit');
    const counts = { unsaved: unsavedSsh.stats.connections, saved: savedSsh.stats.authenticated, key: keySsh.stats.authenticated };
    backend.child.stdin.end();
    const timer = setTimeout(() => backend.child.kill(), 10_000);
    try { assert.equal(await backend.closed, 0, 'EOF must exit through graceful packaged service shutdown'); }
    finally { clearTimeout(timer); }
    assert.ok(backend.messages.some(message => message.event === 'lion:shutdown'));
    backend = null;
    await assert.rejects(roundTrip(savedPort, 'closed-on-exit', 1000));
    await assert.rejects(roundTrip(keyPort, 'closed-on-exit', 1000));
    assert.deepEqual(await remembered(dataDir), expectedRemembered, 'shutdown must preserve connection intent');
    report.gracefulPackagedServiceShutdown = { cleanExit: true, tunnelsClosed: true, rememberedConnectionsPreserved: true, savedPasswordEncrypted: true };

    app = await launch(dataDir, verification, errors);
    assert.equal((await app.invoke('initial')).version, version);
    await pollState(app, saved.id, state => state.status === 'connected' && state.gpu.status === 'ready' && state.tunnels[0]?.status === 'running', 'automatic saved-password restoration and GPU samples');
    await pollState(app, key.id, state => state.status === 'connected' && state.tunnels[0]?.status === 'running', 'automatic public-key restoration');
    assert.equal(await roundTrip(savedPort, 'packaged-restored-password-中文'), 'packaged-restored-password-中文');
    assert.equal(await roundTrip(keyPort, 'packaged-restored-key-中文'), 'packaged-restored-key-中文');
    const restored = await app.invoke('initial');
    for (const input of unsaved) {
      const state = restored.states.find(candidate => candidate.profileId === input.id);
      assert.equal(state.status, 'disconnected');
      assert.equal(state.error ?? null, null);
      assert.deepEqual(state.events, []);
    }
    await delay(500);
    assert.equal(unsavedSsh.stats.connections, counts.unsaved, 'unsaved password profiles must open no new SSH transports');
    assert.equal(savedSsh.stats.authenticated, counts.saved + 1, 'repeated initial snapshots must not duplicate sessions');
    assert.equal(keySsh.stats.authenticated, counts.key + 1);
    assert.deepEqual(restored.pendingHostKeys, []);
    assert.equal(await app.page.getByTestId('host-accept').count(), 0);
    report.automaticNativeStartup = { savedPasswordConnected: true, keyConnected: true, restoredTunnelsPassBytes: true, unsavedPasswordProfilesSkipped: 2, noHostPrompt: true, repeatedInitialDoesNotReconnect: true };

    await app.page.locator('.server-item').filter({ hasText: saved.name }).click();
    await app.page.getByRole('button', { name: '查看显卡详情', exact: true }).click();
    await app.page.getByTestId('gpu-utilization-meter').first().waitFor();
    const metrics = await app.page.locator('.gpu-detail').evaluateAll(cards => cards.map(card => {
      const measure = selector => {
        const meter = card.querySelector(selector);
        const fill = meter.firstElementChild;
        return { value: Number(meter.getAttribute('aria-valuenow')), color: getComputedStyle(fill).backgroundColor,
          width: meter.getBoundingClientRect().width, fillWidth: fill.getBoundingClientRect().width,
          top: meter.getBoundingClientRect().top, bottom: meter.getBoundingClientRect().bottom };
      };
      return { utilization: measure('.utilization-meter'), memory: measure('.memory-meter') };
    }));
    assert.equal(metrics.length, 2);
    for (let index = 0; index < metrics.length; index++) {
      const metric = metrics[index];
      assert.notEqual(metric.utilization.color, metric.memory.color);
      assert.ok(metric.memory.top > metric.utilization.bottom);
      for (const [kind, expected] of [['utilization', index === 0 ? 65 : 0], ['memory', (index === 0 ? 18119 : 15) / 24576 * 100]]) {
        assert.ok(Math.abs(metric[kind].value - expected) < 0.001);
        assert.ok(Math.abs(metric[kind].fillWidth - metric[kind].width * expected / 100) < 1);
      }
    }
    await app.page.getByTestId('gpu-card').screenshot({ path: path.join(output, 'packaged-gpu-dual-meters.png'), animations: 'disabled' });
    report.nativeGpuMeters = { distinctColors: true, utilizationMatches65PercentAndZero: true, memoryMatchesActualMiB: true, metrics };
    assert.deepEqual(app.consoleErrors, []);
    for (const id of activeIds) await app.invoke('disconnect', id);
    for (const id of activeIds) await pollState(app, id, state => state.status === 'disconnected', 'manual disconnect');
    assert.deepEqual(await remembered(dataDir), unsaved.map(input => input.id).sort());
    await app.stop(); app = null;

    const afterDisconnect = [savedSsh.stats.authenticated, keySsh.stats.authenticated];
    app = await launch(dataDir, verification, errors);
    const disconnected = await app.invoke('initial');
    for (const id of activeIds) assert.equal(disconnected.states.find(state => state.profileId === id).status, 'disconnected');
    await delay(500);
    assert.deepEqual([savedSsh.stats.authenticated, keySsh.stats.authenticated], afterDisconnect);
    assert.equal(unsavedSsh.stats.connections, counts.unsaved);
    await assert.rejects(roundTrip(savedPort, 'manual-disconnect-stays-closed', 1000));
    await assert.rejects(roundTrip(keyPort, 'manual-disconnect-stays-closed', 1000));
    assert.deepEqual(app.consoleErrors, []);
    await app.stop(); app = null;
    report.manualDisconnectSurvivesNativeRestart = true;
    assert.deepEqual(errors, []);
    assert.deepEqual(existingApps(), inventory, 'pre-existing applications must retain their original process identities');
    report.existingInstancesPreserved = true;
    report.ok = true;
  } catch (error) {
    report.error = error.stack || String(error);
    throw error;
  } finally {
    if (app) {
      for (const id of activeIds) await app.invoke('disconnect', id).catch(() => {});
      await app.stop();
    }
    await backend?.stop();
    for (const server of servers.reverse()) await server.close();
    if (report.ok) {
      assert.equal(path.dirname(path.resolve(fixture)).toLowerCase(), output.toLowerCase());
      assert.ok(path.basename(fixture).startsWith('connection-gpu-desktop-fixture-'));
      await fs.rm(fixture, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      report.temporaryDataRemoved = true;
    }
    await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
  }
  console.log(`Packaged GPU and startup restoration verification passed: ${reportPath}`);
}

if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });
module.exports = { main };
