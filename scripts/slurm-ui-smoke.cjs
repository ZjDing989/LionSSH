'use strict';
// Renderer checks use an injected API; no native service or remote host is contacted.
const { chromium } = require('playwright');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');

(async () => {
  await fs.mkdir(output, { recursive: true });
  const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--allow-file-access-from-files'] });
  const errors = [];
  try {
    const page = await browser.newPage({ viewport: { width: 1366, height: 1000 }, colorScheme: 'light' });
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      const state = {
        profileId: 'slurm-ui', status: 'connected',
        gpu: { status: 'ready', availability: 'absent', cards: [], idleCount: 0, total: 0, intervalSec: 5 },
        slurm: { availability: 'available', status: 'ready', scope: 'mine', intervalSec: 30, lastUpdated: Date.now(), jobs: [] },
        tunnels: [], events: [],
      };
      const ownJobs = [
        { id: '167981', partition: 'gpu', name: 'train_model', user: 'user', stateCode: 'R', stateLabel: '运行中', stateTone: 'positive', elapsed: '2-03:11:05', nodes: 1, nodeList: 'gpu01', reasonCode: '', reasonLabel: '' },
        { id: '167986_[1-40%4]', partition: 'gpu-long', name: 'very_long_training_experiment_name_for_array_jobs', user: 'user', stateCode: 'PD', stateLabel: '排队中', stateTone: 'pending', elapsed: '0:00', nodes: 1, nodeList: '', reasonCode: 'Resources', reasonLabel: '等待可用资源' },
        { id: '167987', partition: 'gpu', name: 'compare', user: 'user', stateCode: 'PD', stateLabel: '排队中', stateTone: 'pending', elapsed: '0:00', nodes: 2, nodeList: '', reasonCode: 'QOSMaxGRESPerUser', reasonLabel: '达到服务质量规定的用户通用资源上限' },
      ];
      const allJobs = [...ownJobs, { id: '167988', partition: 'compute', name: 'simulation', user: 'colleague', stateCode: 'CF', stateLabel: '配置中', stateTone: 'pending', elapsed: '0:01', nodes: 2, nodeList: 'node[01-02]', reasonCode: '', reasonLabel: '' }];
      const profile = { id: state.profileId, name: '集群服务器', host: 'cluster.example', port: 22, username: 'user', authType: 'key', keyPath: 'C:\\Users\\user\\.ssh\\id_ed25519', autoReconnect: true, gpu: { enabled: true, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 }, tunnels: [] };
      let stateCallback;
      const send = () => stateCallback?.(JSON.parse(JSON.stringify(state)));
      state.slurm.jobs = ownJobs;
      window.__slurmUi = { state, send, ownJobs, allJobs };
      window.lion = {
        initialTheme: { preference: 'light', resolved: 'light' },
        getInitial: async () => ({ profiles: [profile], states: [JSON.parse(JSON.stringify(state))], version: '1.5.0' }),
        setTheme: async preference => ({ preference, resolved: preference === 'system' ? 'light' : preference }),
        onState: callback => { stateCallback = callback; return () => {}; },
        onHostKey: () => () => {}, onTheme: () => () => {}, onShutdown: () => () => {},
        setSlurmScope: async (_id, scope) => {
          state.slurm.scope = scope; state.slurm.status = 'loading'; state.slurm.jobs = []; send();
          setTimeout(() => { state.slurm.jobs = scope === 'mine' ? ownJobs : allJobs; state.slurm.status = 'ready'; state.slurm.lastUpdated = Date.now(); send(); }, 650);
        },
        refreshSlurm: async () => { state.slurm.status = 'loading'; send(); setTimeout(() => { state.slurm.status = 'ready'; state.slurm.error = ''; send(); }, 200); },
      };
    });
    await page.goto(pathToFileURL(path.join(root, 'dist/index.html')).href);
    const card = page.getByTestId('slurm-card');
    await card.waitFor();
    assert.deepEqual(await page.getByTestId('slurm-table').locator('th').allTextContents(), ['作业编号', '分区', '作业名称', '用户', '状态', '已运行时间', '节点数', '节点列表', '原因', '权重', '操作']);
    assert.equal(await card.locator('tbody tr').count(), 3);
    assert.equal(await card.getByText('运行中', { exact: true }).count(), 1);
    assert.equal(await card.getByText('等待可用资源', { exact: true }).count(), 1);
    assert.equal(await card.getByText('达到服务质量规定的用户通用资源上限', { exact: true }).count(), 1);
    assert.equal(await card.locator('tbody tr').nth(1).locator('td').nth(7).textContent(), '—');
    assert.equal(await card.getByText('等待可用资源', { exact: true }).getAttribute('title'), 'Slurm 原因：Resources');
    await page.getByTestId('slurm-scope-all').click();
    await page.getByTestId('slurm-loading').waitFor();
    assert.equal(await page.getByTestId('slurm-table').count(), 0, 'old-scope rows must disappear while switching');
    assert.equal(await page.getByTestId('slurm-scope-all').getAttribute('aria-pressed'), 'true');
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 4);
    assert.equal(await card.getByText('colleague', { exact: true }).count(), 1);
    for (const theme of ['light', 'dark']) {
      await page.getByTestId(`theme-${theme}`).click();
      await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
      await page.waitForTimeout(220);
      await card.screenshot({ path: path.join(output, `slurm-card-${theme}.png`) });
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({ path: path.join(output, `slurm-ui-${theme}.png`), fullPage: true });
    }
    await page.setViewportSize({ width: 880, height: 1000 });
    const layout = await page.evaluate(() => ({ viewport: innerWidth, document: document.documentElement.scrollWidth, tableWidth: document.querySelector('.slurm-table-wrap').clientWidth, tableScrollWidth: document.querySelector('.slurm-table-wrap').scrollWidth }));
    assert(layout.document <= layout.viewport + 1, `page overflow: ${JSON.stringify(layout)}`);
    assert(layout.tableScrollWidth > layout.tableWidth, 'narrow table should scroll within its card');
    await page.screenshot({ path: path.join(output, 'slurm-ui-narrow.png'), fullPage: true });
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.status = 'disconnected'; send(); });
    await card.getByText('连接已断开，当前展示上次结果。', { exact: true }).waitFor();
    assert.equal(await page.getByTestId('slurm-refresh').isDisabled(), true);
    assert.equal(await page.getByTestId('slurm-scope-mine').isDisabled(), true);
    assert.equal(await card.locator('tbody tr').count(), 4);
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.status = 'connected'; state.slurm.status = 'error'; state.slurm.error = '读取作业超时'; send(); });
    await card.getByText('读取作业超时 · 当前展示上次结果', { exact: true }).waitFor();
    assert.equal(await card.locator('tbody tr').count(), 4);
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.availability = 'unavailable'; state.slurm.jobs = []; send(); });
    await card.waitFor({ state: 'hidden' });
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.availability = 'unknown'; state.slurm.status = 'error'; state.slurm.error = 'Slurm 控制器暂时无法连接'; send(); });
    await page.getByTestId('slurm-detect-note').waitFor();
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.availability = 'available'; state.slurm.status = 'ready'; state.slurm.error = ''; state.slurm.jobs = []; send(); });
    await card.waitFor();
    assert.equal(await page.getByTestId('slurm-empty').textContent(), '当前暂无可见作业');
    assert.equal(await page.getByTestId('slurm-detect-note').count(), 0);
    await page.evaluate(() => {
      const { state, send, ownJobs } = window.__slurmUi;
      const jobs = Array.from({ length: 203 }, (_, index) => ({ ...ownJobs[0], id: String(300000 + index), name: `job_${index}` }));
      window.__slurmUi.largeJobs = jobs;
      state.slurm.jobs = jobs; send();
    });
    await page.getByTestId('slurm-pagination').waitFor();
    assert.equal(await card.locator('.count-tag').textContent(), '203', 'card count includes every job, not only the visible page');
    assert.equal(await card.locator('tbody tr').count(), 100);
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '1 / 3 页');
    assert.equal(await page.getByTestId('slurm-prev-page').isDisabled(), true);
    await page.getByTestId('slurm-next-page').click();
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '2 / 3 页');
    assert.equal(await card.locator('tbody tr').first().locator('td').first().textContent(), '300100');
    await page.getByTestId('slurm-next-page').click();
    assert.equal(await card.locator('tbody tr').count(), 3);
    assert.equal(await page.getByTestId('slurm-next-page').isDisabled(), true);
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; state.slurm.jobs = []; send(); });
    await page.getByTestId('slurm-loading').waitFor();
    await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.status = 'ready'; state.slurm.jobs = largeJobs; send(); });
    await page.getByTestId('slurm-table').waitFor();
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '3 / 3 页', 'automatic refresh preserves the selected page');
    assert.equal(await card.locator('tbody tr').count(), 3);
    await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.jobs = largeJobs.slice(0, 105); send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-page-number"]').textContent === '2 / 2 页');
    assert.equal(await card.locator('tbody tr').count(), 5, 'shrinking queues clamp the page to the remaining last page');
    await page.getByTestId('slurm-scope-mine').click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 3);
    assert.equal(await page.getByTestId('slurm-pagination').count(), 0, 'small queues omit pagination');
    await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.jobs = largeJobs; send(); });
    await page.getByTestId('slurm-pagination').waitFor();
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '1 / 3 页', 'changing scope resets the page');
    assert.deepEqual(errors, []);
    await fs.writeFile(path.join(output, 'slurm-ui-smoke.json'), JSON.stringify({ passed: true, headers: 9, scopes: ['mine', 'all'], themes: ['light', 'dark'], pagination: { total: 203, pageSize: 100, refreshStable: true, shrinkClamped: true }, layout, errors }, null, 2));
    console.log('Slurm UI smoke passed: translated columns, scope loading, themes, contained scrolling, stale results, capability hiding and large-queue pagination.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
