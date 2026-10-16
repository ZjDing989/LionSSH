'use strict';
// Renderer-only server fixtures. No Codex installation or SSH connection runs.
const { chromium } = require('playwright');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');

(async () => {
  await fs.mkdir(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--allow-file-access-from-files'] });
  const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 1380, height: 950 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.addInitScript(() => {
      const clone = value => JSON.parse(JSON.stringify(value));
      const profiles = ['alpha', 'beta'].map(id => ({ id, name: id === 'alpha' ? 'Alpha 服务器' : 'Beta 服务器', host: '127.0.0.1', port: 22, username: 'tester', authType: 'key', keyPath: '/fixture/key', autoReconnect: false, gpu: { enabled: false, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 }, tunnels: [] }));
      const codex = (installed, method = 'npm') => ({ status: 'ready', installed, currentVersion: installed ? '0.1.0' : null, latestVersion: '0.2.0', updateAvailable: installed, installMethod: method, executablePath: installed ? '/home/tester/.local/bin/codex' : null, canInstall: true, unsupportedReason: null, lastChecked: Date.now(), lastResult: null, error: null, restartStatus: null, restartMessage: null, log: [] });
      const states = Object.fromEntries(profiles.map(profile => [profile.id, { profileId: profile.id, status: 'connected', gpu: { status: 'unknown', availability: 'unavailable', cards: [], idleCount: 0, total: 0, intervalSec: 5 }, slurm: { availability: 'unavailable', status: 'unknown', scope: 'mine', jobs: [], intervalSec: 30 }, storage: { status: 'unknown', homePath: '', homeBytes: null, tmpBytes: null, totalBytes: null, intervalSec: 300 }, tunnels: [], events: [], codex: codex(profile.id === 'alpha', profile.id === 'alpha' ? 'npm' : 'native') }]));
      let onState = () => {}, onTheme = () => {};
      const mock = window.codexMock = { profiles, states, calls: [], deferAlpha: false, failCheck: false, emit(id) { onState(clone(states[id])); }, set(id, patch) { Object.assign(states[id].codex, patch); this.emit(id); } };
      window.lion = {
        getInitial: async () => ({ profiles: clone(profiles), states: clone(Object.values(states)), version: '1.8.0', theme: { preference: 'dark', resolved: 'dark' } }),
        onState: callback => { onState = callback; return () => {}; },
        onTheme: callback => { onTheme = callback; return () => {}; },
        onHostKey: () => () => {}, onShutdown: () => () => {},
        setTheme: async preference => { const value = { preference, resolved: preference }; onTheme(value); return value; },
        refreshCodex: async id => {
          mock.calls.push(['refresh', id]);
          mock.set(id, { status: 'checking' });
          if (mock.deferAlpha && id === 'alpha') return new Promise(resolve => { mock.resolveAlpha = resolve; });
          await new Promise(resolve => setTimeout(resolve, 30));
          if (mock.failCheck) {
            mock.set(id, { status: 'error', installed: null, currentVersion: null, latestVersion: null, canInstall: false, error: '模拟版本查询失败' });
            throw Error('模拟版本查询失败');
          }
          mock.set(id, { status: 'ready', error: null });
          return clone(states[id].codex);
        },
        installCodex: async id => {
          mock.calls.push(['install', id]);
          mock.set(id, { status: 'installing', log: ['正在沿用检测到的安装方式更新 Codex'] });
          return new Promise((resolve, reject) => { mock.failInstall = () => {
            mock.set(id, { status: 'error', error: '模拟安装失败', log: ['模拟安装失败'] });
            reject(Error('模拟安装失败'));
          }; mock.completeInstall = () => {
            mock.set(id, { status: 'restarting', log: ['Codex 安装完成', '正在重新启动 Codex 服务'] });
            setTimeout(() => {
              mock.set(id, { status: 'ready', installed: true, currentVersion: '0.2.0', updateAvailable: false, restartStatus: 'restarted', restartMessage: 'Codex 服务已重新启动', lastResult: 'Codex 已更新至 0.2.0', log: ['版本校验通过', 'Codex 服务已重新启动'] });
              resolve(clone(states[id].codex));
            }, 50);
          }; });
        },
      };
    });
    await page.goto(pathToFileURL(path.join(root, 'dist/index.html')).href);
    await page.getByTestId('tools-card').waitFor();
    assert.equal(await page.locator('.page-header [data-testid="add-device"]').count(), 0);
    assert.equal(await page.locator('.page-header').getByRole('button', { name: '添加设备', exact: true }).count(), 0);
    assert.equal(await page.getByTestId('add-device').count(), 1, 'sidebar add-device entry stays available');
    assert.equal(await page.evaluate(() => document.querySelector('[data-testid="tools-card"]').previousElementSibling?.classList.contains('tunnel-card')), true, 'tools follow port tunnels');
    await page.getByTestId('tool-codex').click();
    await page.getByTestId('codex-page').waitFor();
    await page.waitForFunction(() => document.querySelector('[data-testid="codex-install"]')?.disabled === false);
    assert.equal(await page.getByTestId('codex-current-version').innerText(), '0.1.0');
    assert.equal(await page.getByTestId('codex-latest-version').innerText(), '0.2.0');
    assert.match(await page.getByTestId('codex-install').innerText(), /更新 Codex/);
    await page.screenshot({ path: path.join(output, 'codex-tool-update.png'), animations: 'disabled' });
    await page.getByTestId('codex-install').click();
    await page.getByTestId('codex-log').waitFor();
    assert.equal(await page.getByTestId('codex-install').isDisabled(), true);
    assert.match(await page.getByTestId('codex-install').innerText(), /正在更新/);
    assert.equal(await page.evaluate(() => window.codexMock.calls.filter(x => x[0] === 'install').length), 1);
    await page.evaluate(() => window.codexMock.completeInstall());
    await page.getByText('Codex 服务已重新启动', { exact: true }).first().waitFor();
    await page.waitForFunction(() => document.querySelector('[data-testid="codex-current-version"]')?.textContent === '0.2.0');
    assert.equal(await page.getByTestId('codex-install').isDisabled(), true, 'current release cannot be reinstalled as an update');
    await page.evaluate(() => window.codexMock.set('alpha', { currentVersion: '0.1.0', latestVersion: '0.2.0', updateAvailable: true, lastResult: null, restartStatus: null, restartMessage: null }));
    await page.getByTestId('codex-install').click();
    await page.evaluate(() => window.codexMock.failInstall());
    await page.getByTestId('codex-error').waitFor();
    assert.equal(await page.getByTestId('codex-current-version').innerText(), '0.1.0', 'a failed install retains the known server version');
    await page.getByTestId('codex-refresh').click();
    await page.getByTestId('codex-error').waitFor({ state: 'hidden' });
    assert.equal(await page.getByTestId('codex-install').isEnabled(), true, 'checking again allows retry after an installation error');
    await page.getByTestId('codex-back').click();
    await page.getByTestId('tools-card').waitFor();
    assert.equal(await page.getByTestId('codex-page').count(), 0);
    assert.equal(await page.locator('.server-item.selected').count(), 1);
    await page.getByTestId('tool-codex').click();
    await page.waitForFunction(() => !document.querySelector('[data-testid="codex-refresh"]')?.disabled);
    const choose = name => page.locator('.server-item').filter({ hasText: name }).click();
    await choose('Beta 服务器');
    await page.waitForFunction(() => !document.querySelector('[data-testid="codex-install"]')?.disabled);
    assert.equal(await page.getByTestId('codex-current-version').innerText(), '未安装');
    assert.match(await page.getByTestId('codex-install').innerText(), /安装 Codex/);
    await page.getByTestId('codex-install').click();
    await page.evaluate(() => window.codexMock.completeInstall());
    await page.waitForFunction(() => document.querySelector('[data-testid="codex-current-version"]')?.textContent === '0.2.0');
    assert.equal(await page.evaluate(() => window.codexMock.calls.filter(x => x[0] === 'install' && x[1] === 'beta').length), 1);
    await page.evaluate(() => window.codexMock.set('beta', { currentVersion: '0.3.0', latestVersion: '0.2.0', updateAvailable: true }));
    assert.equal(await page.getByTestId('codex-install').isDisabled(), true, 'never offer a downgrade even if a bad snapshot says updateAvailable');
    await page.evaluate(() => { window.codexMock.failCheck = true; });
    await page.getByTestId('codex-refresh').click();
    await page.getByTestId('codex-error').waitFor();
    assert.equal(await page.getByTestId('codex-current-version').innerText(), '尚未检测');
    assert.equal(await page.getByTestId('codex-install').isDisabled(), true, 'failed detection must not become an install action');
    await page.evaluate(() => { window.codexMock.failCheck = false; window.codexMock.set('beta', { installed: true, currentVersion: '0.2.0', latestVersion: '0.2.0', canInstall: true, updateAvailable: false }); });
    await page.getByTestId('codex-refresh').click();
    await page.getByTestId('codex-error').waitFor({ state: 'hidden' });
    await page.evaluate(() => { window.codexMock.deferAlpha = true; });
    await choose('Alpha 服务器');
    await page.waitForFunction(() => Boolean(window.codexMock.resolveAlpha));
    await choose('Beta 服务器');
    await page.waitForFunction(() => !document.querySelector('[data-testid="codex-refresh"]')?.disabled);
    await page.evaluate(() => window.codexMock.resolveAlpha({ ...window.codexMock.states.alpha.codex, status: 'ready', currentVersion: '9.9.9' }));
    assert.equal(await page.getByTestId('codex-current-version').innerText(), '0.2.0', 'late server A reply cannot replace server B');
    await page.evaluate(() => { window.codexMock.states.beta.status = 'disconnected'; window.codexMock.emit('beta'); });
    await page.getByTestId('codex-connection-note').waitFor();
    assert.equal(await page.getByTestId('codex-refresh').isDisabled(), true);
    assert.equal(await page.getByTestId('codex-install').isDisabled(), true);
    for (const theme of ['light', 'dark']) {
      await page.getByTestId(`theme-${theme}`).click();
      await page.screenshot({ path: path.join(output, `codex-tool-${theme}.png`), animations: 'disabled' });
    }
    await page.setViewportSize({ width: 900, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false);
    assert.deepEqual(errors, []);
    const report = { ok: true, operations: 'renderer mock only', topbarAddRemoved: true, sidebarAddPreserved: true, toolsBelowTunnels: true, dedicatedPageAndBack: true, npmUpdate: true, nativeInstall: true, progressAndRestartResult: true, failedInstallationRetry: true, downgradeDisabled: true, failedDetectionNotMissing: true, disconnectedDisabled: true, staleOtherServerReplyIgnored: true, rendererErrors: errors };
    await fs.writeFile(path.join(output, 'codex-ui-smoke.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
