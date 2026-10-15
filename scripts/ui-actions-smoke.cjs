'use strict';
// Renderer-only checks: every server operation below is an in-memory mock.
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
  const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.addInitScript(() => {
      const profile = { id: 'renderer-actions', name: '作业测试服务器', host: '127.0.0.1', port: 22, username: 'tester', authType: 'key', keyPath: 'test-key', autoReconnect: true, gpu: { enabled: true, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 }, tunnels: [] };
      const job = (id, user) => ({ id, partition: 'gpu', name: user === 'tester' ? '我的训练' : '他人的训练', user, stateCode: 'R', stateLabel: '运行中', stateTone: 'positive', elapsed: '1:20:30', nodes: 1, nodeList: 'gpu01', reasonCode: '', reasonLabel: '' });
      const state = { profileId: profile.id, status: 'connected', gpu: { status: 'ready', availability: 'available', cards: [{ index: 0, uuid: 'GPU-1', name: 'NVIDIA RTX 4090', memoryTotalMiB: 24576, memoryUsedMiB: 0, utilization: 0, computeProcesses: [], idle: true, stableCount: 2 }], idleCount: 1, total: 1, intervalSec: 5, lastUpdated: Date.now(), nextPollAt: Date.now() + 5000 }, slurm: { availability: 'available', status: 'ready', scope: 'all', jobs: [job('4101', 'tester'), job('4102', 'another')], lastUpdated: Date.now(), intervalSec: 30 }, storage: { status: 'ready', homePath: '/home/tester', homeBytes: 1024 ** 3 * 2, tmpBytes: 1024 ** 2 * 32, totalBytes: 1024 ** 3 * 2 + 1024 ** 2 * 32, intervalSec: 300, lastUpdated: Date.now() }, tunnels: [], events: [] };
      let onState = () => {};
      let onTheme = () => {};
      let onHostKey = () => {};
      let onBackendError = () => {};
      let onUpdate = () => {};
      const update = { status: 'available', currentVersion: '1.6.0', version: '1.7.1', notes: '更新测试', publishedAt: null, error: null, downloaded: 0, total: null, lastChecked: Date.now(), nextCheckAt: Date.now() + 3600000, portable: true, configured: true };
      const hostKey = { profileId: profile.id, fingerprint: 'SHA256:startup-native-host', keyType: 'ssh-ed25519', host: profile.host, port: profile.port, changed: false };
      state.status = 'awaiting-trust';
      const calls = [];
      const clone = value => JSON.parse(JSON.stringify(value));
      const record = name => calls.push(name);
      window.mock = { calls, state, hostTrust: [], expire: false, failPriority: false, update, setUpdate: patch => { Object.assign(update, patch); onUpdate(clone(update)); }, emit: () => onState(clone(state)), requestHostKey: () => { state.status = 'awaiting-trust'; onState(clone(state)); onHostKey(clone(hostKey)); }, failBackend: () => onBackendError({ message: '本地连接服务已退出，请重新启动应用。' }) };
      const backend = {
        getInitial: async () => {
          if (window.mock.eventRegistrations.length !== 6) throw Error('Initial snapshot requested before all native listeners were installed');
          const staleSnapshot = clone(state);
          staleSnapshot.storage.homeBytes = 1;
          // A live state received while initial is in flight must beat its snapshot.
          onState(clone(state));
          onHostKey(clone(hostKey));
          onUpdate(clone(update));
          const expiredState = { ...clone(state), profileId: 'expired-profile', status: 'error' };
          return { profiles: [profile], states: [staleSnapshot, expiredState], version: '1.6.0', update: { ...clone(update), status: 'idle', version: null }, theme: { preference: 'light', resolved: 'light' }, pendingHostKeys: [clone(hostKey), { ...clone(hostKey), profileId: 'expired-profile' }] };
        },
        setTheme: async preference => { const theme = { preference, resolved: preference === 'dark' ? 'dark' : 'light' }; onTheme(theme); return theme; },
        onState: callback => { onState = callback; return () => {}; }, onTheme: callback => { onTheme = callback; return () => {}; }, onHostKey: callback => { onHostKey = callback; return () => {}; }, onShutdown: () => () => {}, onBackendError: callback => { onBackendError = callback; return () => {}; },
        onUpdate: callback => { onUpdate = callback; return () => {}; },
        openUpdateDownload: async () => record('openUpdateDownload'),
        installUpdate: async () => {
          record('installUpdate');
          window.mock.setUpdate({ status: 'downloading', downloaded: 25, total: 100, error: null });
          await new Promise((_, reject) => { window.mock.failUpdate = () => { window.mock.setUpdate({ status: 'available', error: '模拟下载失败' }); reject(Error('模拟下载失败')); }; });
        },
        trustHost: async (...args) => { window.mock.hostTrust.push(args); state.status = 'connected'; onState(clone(state)); },
        pickKey: async () => 'C:\\mock\\id_ed25519',
        refreshStorage: async () => { record('refreshStorage'); onState(clone(state)); },
        setSlurmScope: async (_id, scope) => { record('setSlurmScope'); state.slurm.scope = scope; state.slurm.jobs = scope === 'mine' ? [job('4101', 'tester')] : [job('4101', 'tester'), job('4102', 'another')]; onState(clone(state)); },
        prepareJobCancel: async (_id, jobId) => { record('prepareJobCancel'); return { jobId, owner: 'tester', token: 'job-token', expiresAt: Date.now() + (window.mock.expire ? -1000 : 60000) }; },
        cancelJob: async (_id, jobId, token) => { if (token !== 'job-token') throw Error('Invalid token'); record('cancelJob'); state.slurm.jobs = state.slurm.jobs.filter(job => job.id !== jobId); onState(clone(state)); return { jobId, cancelled: true }; },
        getJobPriority: async (_id, jobId) => { record('getJobPriority'); if (window.mock.failPriority) throw Error('权重读取失败'); return { jobId, priority: jobId === '4101' ? 43000 : 0 }; },
        prepareTmpCleanup: async () => { record('prepareTmpCleanup'); return { username: 'tester', uid: 1001, root: '/tmp', fileCount: 3, bytes: 32 * 1024 ** 2, token: 'tmp-token', expiresAt: Date.now() + (window.mock.expire ? -1000 : 60000) }; },
        cleanupTmp: async (_id, token) => { if (token !== 'tmp-token') throw Error('Invalid token'); record('cleanupTmp'); state.storage.tmpBytes = 0; state.storage.totalBytes = state.storage.homeBytes; onState(clone(state)); return { deletedCount: 3, cleanupStatus: 'complete' }; },
      };
      // Exercise the production Tauri bridge without starting SSH or a native process.
      const callbacks = new Map();
      const unlisteners = new Map();
      const eventMethods = { 'lion:state': 'onState', 'lion:theme': 'onTheme', 'lion:host-key': 'onHostKey', 'lion:shutdown': 'onShutdown', 'lion:backend-error': 'onBackendError', 'lion:update': 'onUpdate' };
      let nextId = 1;
      window.mock.eventRegistrations = [];
      window.isTauri = true;
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, id) => { unlisteners.get(id)?.(); unlisteners.delete(id); } };
      window.__TAURI_INTERNALS__ = {
        transformCallback: callback => { const id = nextId++; callbacks.set(id, callback); return id; },
        unregisterCallback: id => callbacks.delete(id),
        invoke: async (command, args) => {
          if (command === 'plugin:event|listen') {
            await new Promise(resolve => setTimeout(resolve, args.event === 'lion:host-key' ? 80 : 20));
            const id = nextId++;
            const deliver = payload => callbacks.get(args.handler)?.({ event: args.event, id, payload });
            unlisteners.set(id, backend[eventMethods[args.event]]?.(deliver) || (() => {}));
            window.mock.eventRegistrations.push(args.event);
            return id;
          }
          if (command === 'plugin:event|unlisten') return;
          if (command !== 'lion_request') throw Error(`Unexpected native command: ${command}`);
          const name = args.method === 'initial' ? 'getInitial' : args.method.replace(/-([a-z])/g, (_, char) => char.toUpperCase());
          if (!backend[name]) throw Error(`Unexpected backend method: ${args.method}`);
          return backend[name](...args.args);
        },
      };
    });
    await page.goto(pathToFileURL(path.join(root, 'dist/index.html')).href);
    await page.getByTestId('storage-card').waitFor();
    assert.equal(await page.locator('.host-modal').count(), 1, 'live and recovered host-key requests are deduplicated');
    await page.getByTestId('host-accept').click();
    await page.locator('.host-modal').waitFor({ state: 'hidden' });
    assert.deepEqual(await page.evaluate(() => window.mock.hostTrust), [['renderer-actions', true, 'SHA256:startup-native-host']]);
    for (const status of ['error', 'disconnected']) {
      await page.evaluate(() => window.mock.requestHostKey());
      await page.locator('.host-modal').waitFor({ state: 'visible' });
      await page.evaluate(status => { window.mock.state.status = status; window.mock.emit(); }, status);
      await page.locator('.host-modal').waitFor({ state: 'hidden' });
    }
    assert.equal(await page.evaluate(() => window.mock.hostTrust.length), 1, 'expired prompts disappear without sending a trust decision');
    await page.evaluate(() => { window.mock.state.status = 'connected'; window.mock.emit(); });
    await page.getByTestId('add-device').click();
    await page.getByRole('button', { name: '选择私钥文件', exact: true }).click();
    assert.equal(await page.getByPlaceholder('选择本机私钥文件').inputValue(), 'C:\\mock\\id_ed25519');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    assert.equal(await page.getByText('连续采样确认空闲，避免瞬时误判', { exact: true }).count(), 0);
    assert.equal(await page.locator('.gpu-progress-center path').count(), 2, 'center contains the original clock outline and hands');
    assert.equal(await page.locator('.gpu-progress-center').evaluate(element => getComputedStyle(element).backgroundColor), 'rgba(0, 0, 0, 0)');
    assert.equal(await page.getByTestId('job-cancel-4101').count(), 1);
    assert.equal(await page.getByTestId('job-cancel-4102').count(), 0, 'another user has no delete button');
    assert.equal(await page.getByTestId('storage-home').innerText(), '2 GiB');
    assert.equal(await page.getByTestId('storage-tmp').innerText(), '32 MiB');
    assert.equal(await page.getByTestId('storage-total').innerText(), '2.03 GiB');
    const calls = () => page.evaluate(() => window.mock.calls);
    assert.equal((await calls()).length, 0, 'rows do not make an automatic per-job priority RPC');
    assert.equal(await page.locator('.app-titlebar').count(), 0, 'the duplicate titlebar is removed');
    assert.equal(await page.locator('.app-shell').evaluate(element => element.getBoundingClientRect().top), 0);
    const updateButton = page.getByTestId('install-update');
    await updateButton.waitFor();
    assert.match(await updateButton.getAttribute('aria-label'), /1\.7\.1/);
    await page.screenshot({ path: path.join(output, 'desktop-update-available.png'), animations: 'disabled' });
    await updateButton.click();
    await page.waitForFunction(() => document.querySelector('[data-testid="install-update"]')?.textContent.includes('25%'));
    assert.equal(await updateButton.isDisabled(), true);
    assert.equal((await calls()).filter(call => call === 'installUpdate').length, 1);
    await page.evaluate(() => window.mock.failUpdate());
    await page.getByText('更新未完成：模拟下载失败', { exact: true }).waitFor();
    assert.equal(await updateButton.isEnabled(), true, 'a failed update remains retryable');
    await page.evaluate(() => window.mock.setUpdate({ status: 'available', manualOnly: true, error: null }));
    await updateButton.click();
    assert.equal((await calls()).filter(call => call === 'installUpdate').length, 1);
    assert.equal((await calls()).filter(call => call === 'openUpdateDownload').length, 1, 'unsupported local builds open the fixed release page');
    await page.evaluate(() => window.mock.setUpdate({ status: 'idle', version: null, error: null }));
    await updateButton.waitFor({ state: 'hidden' });
    await page.getByTestId('job-priority-query-4101').click();
    await page.getByTestId('job-priority-4101').waitFor();
    assert.equal(await page.getByTestId('job-priority-4101').innerText(), '43,000');
    await page.getByTestId('job-priority-query-4102').click();
    await page.getByTestId('job-priority-4102').waitFor();
    assert.equal(await page.getByTestId('job-priority-4102').innerText(), '0', 'zero priority is a valid value');
    await page.getByTestId('job-cancel-4101').click();
    await page.getByTestId('job-cancel-confirmation').waitFor();
    assert((await page.getByTestId('job-cancel-confirmation').innerText()).includes('tester'));
    assert.equal((await calls()).filter(call => call === 'cancelJob').length, 0);
    await page.getByTestId('job-cancel-dismiss').click();
    assert.equal((await calls()).filter(call => call === 'cancelJob').length, 0, 'dismissal sends no scancel');
    await page.getByTestId('job-cancel-4101').click();
    await page.getByTestId('job-cancel-approve').click();
    await page.getByTestId('job-cancel-confirmation').waitFor({ state: 'hidden' });
    assert.equal((await calls()).filter(call => call === 'cancelJob').length, 1);
    assert.equal(await page.getByTestId('job-cancel-4101').count(), 0);
    await page.getByTestId('tmp-cleanup').click();
    await page.getByTestId('tmp-cleanup-confirmation').waitFor();
    const cleanupText = await page.getByTestId('tmp-cleanup-confirmation').innerText();
    for (const text of ['/tmp', 'tester', 'UID 1001', '3 项', '32 MiB', '此操作无法撤销']) assert(cleanupText.includes(text), text);
    assert.equal((await calls()).filter(call => call === 'cleanupTmp').length, 0);
    await page.getByTestId('tmp-cleanup-dismiss').click();
    assert.equal((await calls()).filter(call => call === 'cleanupTmp').length, 0, 'dismissal sends no deletion');
    await page.getByTestId('tmp-cleanup').click();
    await page.getByTestId('tmp-cleanup-approve').click();
    await page.getByTestId('tmp-cleanup-confirmation').waitFor({ state: 'hidden' });
    assert.equal((await calls()).filter(call => call === 'cleanupTmp').length, 1);
    assert.equal(await page.getByTestId('storage-tmp').innerText(), '0 B');
    await page.evaluate(() => { window.mock.expire = true; });
    await page.getByTestId('tmp-cleanup').click();
    await page.getByText('确认已过期，请取消后重新统计文件。', { exact: true }).waitFor();
    assert.equal(await page.getByTestId('tmp-cleanup-approve').isDisabled(), true);
    await page.getByTestId('tmp-cleanup-dismiss').click();
    await page.evaluate(() => { window.mock.state.storage.homeBytes = null; window.mock.state.storage.tmpBytes = null; window.mock.state.storage.totalBytes = null; window.mock.state.storage.partial = true; window.mock.emit(); });
    for (const part of ['home', 'tmp', 'total']) assert.equal(await page.getByTestId(`storage-${part}`).innerText(), '—');
    await page.evaluate(() => { window.mock.state.status = 'disconnected'; window.mock.emit(); });
    assert.equal(await page.getByTestId('tmp-cleanup').isDisabled(), true);
    await page.evaluate(() => { window.mock.state.status = 'connected'; window.mock.state.slurm.jobs.unshift({ id: '4101', partition: 'gpu', name: '我的训练', user: 'tester', stateCode: 'R', stateLabel: '运行中', stateTone: 'positive', elapsed: '1:20:30', nodes: 1, nodeList: 'gpu01', reasonCode: '', reasonLabel: '' }); window.mock.state.storage.homeBytes = 2 * 1024 ** 3; window.mock.state.storage.tmpBytes = 32 * 1024 ** 2; window.mock.state.storage.totalBytes = 2 * 1024 ** 3 + 32 * 1024 ** 2; window.mock.state.storage.partial = false; window.mock.emit(); });
    assert.equal(await page.getByTestId('job-priority-query-4101').count(), 1, 'reconnection resets old priorities');
    for (const theme of ['light', 'dark']) {
      await page.getByTestId(`theme-${theme}`).click();
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({ path: path.join(output, `desktop-actions-${theme}.png`), animations: 'disabled', fullPage: true });
    }
    await page.setViewportSize({ width: 900, height: 1100 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false, `only the jobs table scrolls horizontally: ${JSON.stringify(await page.evaluate(() => [...document.querySelectorAll('.app-shell, .workspace, .storage-card, .storage-metrics, .slurm-card, .slurm-table-wrap, .gpu-card, .device-card')].map(e => ({ cls: e.className, left: e.getBoundingClientRect().left, right: e.getBoundingClientRect().right, width: e.getBoundingClientRect().width }))))}`);
    await page.evaluate(() => window.mock.failBackend());
    await page.getByTestId('startup-screen').waitFor();
    assert((await page.getByTestId('startup-screen').innerText()).includes('本地连接服务已退出，请重新启动应用。'));
    assert.equal(errors.length, 0, errors.join('\n'));
    const report = { serverOperations: 'mock only', tauriBridge: true, asyncListenersBeforeInitial: true, liveStateBeatsInitialSnapshot: true, startupHostKeyRecoveredAndDeduplicated: true, staleHostKeySnapshotIgnored: true, expiredHostKeyDismissed: true, disconnectedHostKeyDismissed: true, nativeKeyPicker: true, backendFailureVisible: true, ownJobsOnly: true, priorityOnDemand: true, confirmsBeforeCancel: true, confirmsBeforeCleanup: true, expiredTokenDisabled: true, nullUsageIsUnknown: true, clockCenterRestored: true, reconnectClearsPriority: true, calls: await calls(), rendererErrors: errors };
    await fs.writeFile(path.join(output, 'ui-actions-smoke.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report, null, 2));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
