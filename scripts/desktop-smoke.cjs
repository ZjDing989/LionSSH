'use strict';

// Tests the packaged native shell against simulated loopback SSH services only.
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const { chromium } = require('playwright');
const { localSshServer, echoServer, freePort, roundTrip } = require('../tests/helpers/local-ssh.cjs');
const { profile, tunnel, delay } = require('../tests/helpers/fixtures.cjs');
const { mineQueue, allQueue } = require('../tests/helpers/slurm-output.cjs');

const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');
const reportPath = path.join(output, 'desktop-smoke.json');
const packageInfo = require('../package.json');
const executable = path.join(root, 'release', `Lion-SSH-${packageInfo.version}-windows-${process.arch}-portable`, 'Lion SSH.exe');

function inspectExistingApps() {
  const command = `
    @(Get-CimInstance Win32_Process -Filter "Name='lion-ssh.exe' OR Name='Lion SSH.exe'" | ForEach-Object {
      [PSCustomObject]@{ pid=$_.ProcessId; parent=$_.ParentProcessId; path=$_.ExecutablePath; startTicks=$_.CreationDate.ToUniversalTime().Ticks.ToString() }
    }) | ConvertTo-Json -Compress
  `;
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', command], { encoding: 'utf8', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error('无法确认现有 Lion SSH 实例，桌面验证已停止');
  const parsed = result.stdout.trim() ? JSON.parse(result.stdout) : [];
  const running = Array.isArray(parsed) ? parsed : [parsed];
  return { identities: running, existingInstances: running.filter(item => !running.some(parent => parent.pid === item.parent)).length };
}

async function launch(dataDir, errors, verification) {
  const port = await freePort();
  let exited = false;
  let startupLog = '';
  const child = spawn(executable, ['--verification-instance'], {
    cwd: path.dirname(executable), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, LION_DATA_DIR: dataDir, LION_VERIFY_ROOT: verification.root, LION_VERIFY_INSTANCE_ID: verification.id, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` },
  });
  const closed = new Promise(resolve => child.once('close', code => { exited = true; resolve(code); }));
  child.on('error', error => { startupLog += error.message; });
  child.stdout.on('data', data => { startupLog += data.toString(); });
  child.stderr.on('data', data => { startupLog += data.toString(); });
  let browser;
  let page;
  const consoleErrors = [];
  try {
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline) {
      if (exited) throw new Error(`桌面应用提前退出：${startupLog}`);
      try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break; }
      catch { await delay(200); }
    }
    if (!browser) throw new Error(`无法连接 WebView2 调试端口：${startupLog}`);
    while (Date.now() < deadline) {
      page = browser.contexts().flatMap(context => context.pages()).find(candidate => candidate.url() !== 'about:blank');
      if (page) break;
      await delay(100);
    }
    if (!page) throw new Error('WebView2 未创建应用页面');
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
    await page.waitForFunction(() => Boolean(window.__TAURI_INTERNALS__?.invoke), null, { timeout: 15_000 });
    await page.locator('.app-shell').waitFor({ state: 'visible', timeout: 20_000 });
    assert.equal(await page.locator('#startup-shell').count(), 0);
    const invoke = (method, ...args) => page.evaluate(({ method, args }) => window.__TAURI_INTERNALS__.invoke('lion_request', { method, args }), { method, args });
    return {
      page, invoke, child,
      async stop() {
        // Only this spawned test process is terminated. Rust owns its sockets
        // and tasks; live SSH is disconnected explicitly before this call.
        if (!exited) child.kill();
        await closed;
        await browser.close().catch(() => {});
      },
    };
  } catch (error) {
    if (page && !page.isClosed()) {
      const state = await page.evaluate(async () => {
        let backend;
        try {
          backend = await Promise.race([
            window.__TAURI_INTERNALS__.invoke('lion_request', { method: 'initial', args: [] }),
            new Promise((_, reject) => setTimeout(() => reject(new Error('initial diagnostic timed out')), 2500)),
          ]);
        } catch (error) { backend = { error: String(error) }; }
        return { url: location.href, body: document.body.innerText, title: document.title, backend };
      }).catch(() => ({}));
      await fs.writeFile(path.join(output, 'desktop-smoke-launch-failure.json'), JSON.stringify({ ...state, consoleErrors }, null, 2));
      await page.screenshot({ path: path.join(output, 'desktop-smoke-launch-failure.png') }).catch(() => {});
    }
    if (!exited) child.kill();
    await closed;
    await browser?.close().catch(() => {});
    throw error;
  }
}

async function pollState(app, id, predicate, description, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  let state;
  while (Date.now() < deadline) {
    state = (await app.invoke('initial')).states.find(candidate => candidate.profileId === id);
    if (state && predicate(state)) return state;
    await delay(100);
  }
  throw new Error(`等待${description}超时：${JSON.stringify(state)}`);
}

async function nativeRestartSmoke(dataDir, verification) {
  const file = path.join(dataDir, 'native-restart.json');
  const child = spawn(executable, ['--smoke', '--verification-instance'], {
    cwd: path.dirname(executable), windowsHide: true, stdio: 'ignore',
    env: { ...process.env, LION_DATA_DIR: dataDir, LION_SMOKE_RESULT: file, LION_VERIFY_ROOT: verification.root, LION_VERIFY_INSTANCE_ID: verification.id },
  });
  const code = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(new Error('原生重启验证没有自动退出')); }, 30_000);
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', code => { clearTimeout(timer); resolve(code); });
  });
  assert.equal(code, 0);
  const result = JSON.parse(await fs.readFile(file, 'utf8'));
  assert.equal(result.ok, true);
  assert.equal(result.initial.profiles.length, 1);
  assert.equal(result.initial.profiles[0].hasSavedSecret, true);
  assert.equal(result.initial.theme.preference, 'dark');
  return { autoExit: true, profilesRestored: 1, savedCredentialRestored: true, themeRestored: true };
}

function ownedWindowHandle(pid) {
  assert.ok(Number.isInteger(pid) && pid > 0);
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', `(Get-Process -Id ${pid} -ErrorAction Stop).MainWindowHandle.ToInt64()`], { encoding: 'utf8', windowsHide: true, timeout: 3000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`测试实例窗口状态不可用：${result.stderr}`);
  const handle = Number(result.stdout.trim());
  assert.ok(Number.isSafeInteger(handle) && handle > 0);
  return handle;
}

function ownedWindowState(pid, handle) {
  assert.ok(Number.isInteger(pid) && pid > 0 && Number.isSafeInteger(handle) && handle > 0);
  const command = `Get-Process -Id ${pid} -ErrorAction Stop | Out-Null; Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class LionSmokeWindow { [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr handle); }'; if ([LionSmokeWindow]::IsWindowVisible([IntPtr]${handle})) { 'visible' } else { 'hidden' }`;
  const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', command], { encoding: 'utf8', windowsHide: true, timeout: 3000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`测试实例窗口可见性不可用：${result.stderr}`);
  return result.stdout.trim();
}

async function waitOwnedWindow(pid, handle, expected) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (ownedWindowState(pid, handle) === expected) return;
    await delay(100);
  }
  throw new Error(`测试实例窗口未进入 ${expected} 状态`);
}

async function checkTrayRestore(app, dataDir, localPort, remotePort, verification) {
  const pid = app.child.pid;
  const handle = ownedWindowHandle(pid);
  assert.equal(ownedWindowState(pid, handle), 'visible');
  const close = spawnSync('powershell.exe', ['-NoProfile', '-Command', `(Get-Process -Id ${pid} -ErrorAction Stop).CloseMainWindow()`], { encoding: 'utf8', windowsHide: true, timeout: 3000 });
  if (close.error) throw close.error;
  assert.equal(close.status, 0);
  assert.match(close.stdout, /True/i);
  await waitOwnedWindow(pid, handle, 'hidden');
  assert.equal(await roundTrip(localPort, 'tray-local-alive'), 'tray-local-alive');
  assert.equal(await roundTrip(remotePort, 'tray-reverse-alive'), 'tray-reverse-alive');
  const second = spawn(executable, ['--verification-instance'], { cwd: path.dirname(executable), windowsHide: true, stdio: 'ignore', env: { ...process.env, LION_DATA_DIR: dataDir, LION_VERIFY_ROOT: verification.root, LION_VERIFY_INSTANCE_ID: verification.id } });
  const code = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { second.kill(); reject(new Error('第二个测试实例没有及时退出')); }, 10_000);
    second.once('error', error => { clearTimeout(timer); reject(error); });
    second.once('exit', code => { clearTimeout(timer); resolve(code); });
  });
  assert.equal(code, 0);
  await waitOwnedWindow(pid, handle, 'visible');
  assert.equal(app.child.exitCode, null);
  assert.equal(await roundTrip(localPort, 'restored-local-alive'), 'restored-local-alive');
  return { nativeCloseHidesToTray: true, tunnelsRemainActiveWhileHidden: true, secondInstanceRestoresWindow: true };
}

async function main() {
  if (process.platform !== 'win32') throw new Error('此桌面验证脚本需要 Windows 和 WebView2');
  const inventory = inspectExistingApps();
  await fs.access(executable);
  await fs.mkdir(output, { recursive: true });
  const fixture = await fs.mkdtemp(path.join(output, 'desktop-smoke-fixture-'));
  const dataDir = path.join(fixture, 'data');
  await fs.mkdir(dataDir);
  const verification = { id: randomUUID(), root: fixture };
  const errors = [];
  const report = { ok: false, version: packageInfo.version, runtime: 'Tauri + Rust + WebView2', dataDir, existingInstances: inventory.existingInstances, isolatedVerificationNamespace: true, rendererErrors: errors };
  let ssh;
  let echo;
  let app;
  const id = 'desktop-smoke-device';
  try {
    const password = `fixture-${randomUUID()}`;
    ssh = await localSshServer({ password, squeueResponse: command => command === 'squeue' ? allQueue : mineQueue, storageResponse: 'LION_STORAGE_V1\t1000\nHOME\t4096\t3\t2\t0\nTMP\t8192\t4\t3\t0\n' });
    echo = await echoServer();
    const localPort = await freePort();
    const remotePort = await freePort();
    const input = profile(ssh, {
      id, name: '本机模拟服务器', autoReconnect: false,
      gpu: { enabled: true, intervalSec: 2, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 1 },
      tunnels: [tunnel('desktop-local', localPort, echo.port, 'local', { healthIntervalSec: 5 }), tunnel('desktop-reverse', remotePort, echo.port, 'remote', { healthIntervalSec: 5 })],
    });
    app = await launch(dataDir, errors, verification);
    const initial = await app.invoke('initial');
    assert.deepEqual(initial.profiles, []);
    assert.equal(initial.version, packageInfo.version);
    assert.equal(initial.update.currentVersion, packageInfo.version);
    assert.equal(initial.update.configured, true);
    assert.equal(initial.update.portable, true);
    assert.equal(initial.update.status, 'idle');
    assert.deepEqual(await app.invoke('get-update-state'), initial.update);
    await assert.rejects(app.invoke('install-update'), /没有可安装/);
    assert.equal((await app.invoke('initial')).shutdown, null);
    report.nativeUpdaterBridge = true;
    report.duplicateTitlebarRemoved = await app.page.locator('.app-titlebar').count() === 0;
    assert.equal(report.duplicateTitlebarRemoved, true);
    report.firstLaunchEmpty = true;
    for (const method of ['eval', 'shutdown', 'native-theme']) await assert.rejects(app.invoke(method), /不支持|允许|禁止|unknown|not allowed/i);
    report.nativeMethodAllowlist = true;
    await app.invoke('save-profile', input, { password, remember: true });
    const ciphertext = await fs.readFile(path.join(dataDir, 'credentials.json'), 'utf8');
    assert.equal(ciphertext.includes(password), false);
    assert.match(JSON.parse(ciphertext)[id], /^native:v1:/);
    report.nativeCredentialEncryption = true;
    await app.page.getByTestId('theme-dark').click();
    // The OS may already resolve system preference to dark; wait for the
    // selected preference after its persistence RPC, rather than its color.
    await app.page.waitForFunction(() => document.querySelector('[data-testid="theme-dark"]')?.getAttribute('aria-pressed') === 'true');
    assert.equal((await app.invoke('initial')).theme.preference, 'dark');
    await app.stop();
    app = await launch(dataDir, errors, verification);
    const restored = await app.invoke('initial');
    assert.equal(restored.profiles[0].hasSavedSecret, true);
    assert.equal(restored.theme.preference, 'dark');
    await app.page.getByTestId('connect-button').click();
    await app.page.getByTestId('host-accept').waitFor({ state: 'visible', timeout: 15_000 });
    assert.match(await app.page.locator('.fingerprint-box').innerText(), /SHA256:/);
    await app.page.getByTestId('host-accept').click();
    const state = await pollState(app, id, state => state.status === 'connected' && state.tunnels.length === 2 && state.tunnels.every(tunnel => tunnel.status === 'running' && tunnel.healthVerified) && state.gpu.status === 'ready' && state.slurm.status === 'ready' && state.storage.status === 'ready', 'SSH、双向隧道和监测');
    assert.equal(await roundTrip(localPort, 'native-local-forward'), 'native-local-forward');
    assert.equal(await roundTrip(remotePort, 'native-reverse-forward'), 'native-reverse-forward');
    assert.equal(state.gpu.total, 2);
    assert.equal(state.gpu.idleCount, 1);
    assert.equal(state.slurm.jobs.length, 2);
    assert.equal(state.storage.totalBytes, 12288);
    report.savedCredentialDecryptsAfterRestart = true;
    report.hostTrustUi = true;
    report.forwardTunnel = true;
    report.reverseTunnel = true;
    report.gpu = { cards: state.gpu.total, idle: state.gpu.idleCount };
    report.slurm = { jobs: state.slurm.jobs.length };
    report.storage = { totalBytes: state.storage.totalBytes };
    await app.invoke('set-slurm-scope', id, 'all');
    await pollState(app, id, state => state.slurm.scope === 'all' && state.slurm.jobs.length === 3, '全部 Slurm 作业');
    report.tray = await checkTrayRestore(app, dataDir, localPort, remotePort, verification);
    const screenshot = path.join(output, 'desktop-smoke-connected.png');
    await app.page.screenshot({ path: screenshot, animations: 'disabled', fullPage: true });
    report.screenshot = screenshot;
    await app.invoke('disconnect', id);
    await pollState(app, id, state => state.status === 'disconnected', '断开连接');
    await assert.rejects(roundTrip(localPort, 'after-disconnect', 1000));
    await assert.rejects(roundTrip(remotePort, 'after-disconnect', 1000));
    report.disconnectReleasesPorts = true;
    assert.deepEqual(ssh.stats.rejectedCommands, []);
    await app.stop();
    app = null;
    report.nativeRestart = await nativeRestartSmoke(dataDir, verification);
    const after = inspectExistingApps();
    assert.deepEqual(after.identities.sort((a, b) => a.pid - b.pid), inventory.identities.sort((a, b) => a.pid - b.pid));
    report.existingInstancesPreserved = true;
    assert.deepEqual(errors, []);
    report.ok = true;
  } catch (error) {
    report.error = error.stack || String(error);
    throw error;
  } finally {
    if (app) {
      try { await app.invoke('disconnect', id); } catch {}
      await app.stop();
    }
    await echo?.close();
    await ssh?.close();
    if (report.ok) {
      assert.equal(path.dirname(path.resolve(fixture)).toLowerCase(), output.toLowerCase());
      assert.ok(path.basename(fixture).startsWith('desktop-smoke-fixture-'));
      await fs.rm(fixture, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      report.temporaryDataRemoved = true;
    }
    await fs.writeFile(reportPath, JSON.stringify(report, null, 2));
  }
  console.log(`桌面验证通过：${reportPath}`);
}

if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });
module.exports = { main };
