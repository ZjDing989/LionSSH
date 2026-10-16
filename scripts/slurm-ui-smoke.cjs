'use strict';
// Renderer checks use an injected API; no native service or remote host is contacted.
const { chromium } = require('playwright');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');
const fixtureUrl = pathToFileURL(path.join(root, 'dist/index.html')).href;
const resizeStorageKey = 'lion-slurm-card-height';
const chromeOptions = { channel: 'chrome', headless: true, args: ['--allow-file-access-from-files'] };

async function installMockApi(page) {
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
}

async function cardMetrics(page) {
  return page.evaluate(() => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    const table = card.querySelector('.slurm-table-wrap');
    const bounds = card.getBoundingClientRect();
    return {
      height: bounds.height, width: bounds.width,
      tableHeight: table?.clientHeight, tableScrollHeight: table?.scrollHeight,
      viewport: innerWidth, document: document.documentElement.scrollWidth,
    };
  });
}

async function expectCardHeight(page, expected, description) {
  await page.waitForFunction(height => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    return card && Math.abs(card.getBoundingClientRect().height - height) <= 1;
  }, expected);
  const height = Number(await page.getByTestId('slurm-resize-handle').getAttribute('aria-valuenow'));
  assert(Math.abs(height - expected) <= 1, `${description}: accessible height ${height} should match ${expected}`);
}

async function dragCard(page, deltaY) {
  const handle = page.getByTestId('slurm-resize-handle');
  await handle.scrollIntoViewIfNeeded();
  const bounds = await handle.boundingBox();
  assert(bounds, 'resize handle has visible bounds');
  const x = bounds.x + bounds.width / 2;
  const y = bounds.y + bounds.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 55, y + deltaY, { steps: 12 });
  await page.mouse.up();
}

async function verifyResize(page) {
  await page.setViewportSize({ width: 1366, height: 1500 });
  const card = page.getByTestId('slurm-card');
  const handle = page.getByTestId('slurm-resize-handle');
  assert.equal(await handle.getAttribute('role'), 'separator');
  assert.equal(await handle.getAttribute('aria-orientation'), 'horizontal');
  assert.equal(await handle.getAttribute('aria-valuemin'), '280');
  assert.equal(await handle.getAttribute('aria-valuemax'), '2400');
  assert.equal(await page.evaluate(key => localStorage.getItem(key), resizeStorageKey), null, 'untouched cards keep automatic height');
  const initial = await cardMetrics(page);
  assert(initial.tableScrollHeight > initial.tableHeight, 'large queues start with vertical scrolling');
  await page.evaluate(() => window.scrollTo(0, 0));
  const initialGrip = await handle.boundingBox();
  await page.setViewportSize({ width: 1366, height: Math.ceil(initialGrip.y + initialGrip.height / 2 + 20) });
  const edgeGrip = await handle.boundingBox();
  const edgeX = edgeGrip.x + edgeGrip.width / 2;
  const edgeY = edgeGrip.y + edgeGrip.height / 2;
  assert(await page.evaluate(y => innerHeight - y <= 40, edgeY), 'stationary press starts near the lower viewport edge');
  await page.mouse.move(edgeX, edgeY);
  await page.mouse.down();
  await page.waitForTimeout(180);
  assert.equal((await cardMetrics(page)).height, initial.height, 'holding the grip still must not begin edge scrolling or resize');
  assert.equal(await page.evaluate(key => localStorage.getItem(key), resizeStorageKey), null, 'holding the grip still keeps automatic height unsaved');
  await page.mouse.up();
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('resizing-card')), false);
  assert.equal(await page.evaluate(key => localStorage.getItem(key), resizeStorageKey), null, 'clicking the grip keeps automatic height unsaved');

  await page.mouse.down();
  await page.mouse.move(edgeX, (await page.viewportSize()).height - 2, { steps: 3 });
  await page.waitForFunction(() => window.scrollY > 30 && Number(document.querySelector('[data-testid="slurm-resize-handle"]').getAttribute('aria-valuenow')) === 2400);
  await expectCardHeight(page, 2400, 'actual edge dragging auto-scrolls to the maximum');
  const edgeScroll = await page.evaluate(() => window.scrollY);
  await page.waitForTimeout(80);
  assert.equal(await page.evaluate(() => window.scrollY), edgeScroll, 'edge scrolling stops at maximum height');
  await page.mouse.up();
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('resizing-card')), false);
  await handle.dblclick();
  await page.setViewportSize({ width: 1366, height: 1500 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await dragCard(page, 220);
  const enlarged = await cardMetrics(page);
  assert(Math.abs(enlarged.height - initial.height - 220) <= 1, 'dragging downward grows the whole card by the pointer distance');
  assert(enlarged.tableHeight >= initial.tableHeight + 219, 'a larger card exposes more table rows');
  assert(Math.abs(enlarged.width - initial.width) <= 1, 'vertical resizing preserves card width');
  await dragCard(page, -100);
  const smaller = await cardMetrics(page);
  assert(Math.abs(smaller.height - enlarged.height + 100) <= 1, 'dragging upward shrinks the card');
  await handle.evaluate(element => element.addEventListener('pointerdown', event => { window.__slurmPointerId = event.pointerId; }, { once: true }));
  await handle.scrollIntoViewIfNeeded();
  const cancelGrip = await handle.boundingBox();
  const cancelX = cancelGrip.x + cancelGrip.width / 2;
  const cancelY = cancelGrip.y + cancelGrip.height / 2;
  await page.mouse.move(cancelX, cancelY);
  await page.mouse.down();
  await page.mouse.move(cancelX, cancelY + 16, { steps: 3 });
  await handle.dispatchEvent('pointercancel', { pointerId: await page.evaluate(() => window.__slurmPointerId), isPrimary: true });
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('resizing-card')), false, 'pointer cancellation removes the drag cursor and selection lock');
  const cancelledHeight = (await cardMetrics(page)).height;
  await page.mouse.move(cancelX, cancelY + 50);
  await page.mouse.up();
  await page.waitForTimeout(80);
  assert.equal((await cardMetrics(page)).height, cancelledHeight, 'cancelled pointer movement no longer resizes the card');
  const chosenHeight = Number(await handle.getAttribute('aria-valuenow'));
  assert.equal(Number(await page.evaluate(key => localStorage.getItem(key), resizeStorageKey)), chosenHeight, 'drag release saves the chosen height');

  await page.getByTestId('slurm-refresh').click();
  await page.getByTestId('slurm-loading').waitFor();
  await expectCardHeight(page, chosenHeight, 'loading preserves height');
  await page.getByTestId('slurm-table').waitFor();
  await expectCardHeight(page, chosenHeight, 'refresh preserves height');
  await page.getByTestId('slurm-scope-all').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 4);
  await expectCardHeight(page, chosenHeight, 'all scope preserves height with fewer rows');
  await page.getByTestId('slurm-scope-mine').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 3);
  await expectCardHeight(page, chosenHeight, 'personal scope preserves height');
  await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.availability = 'unavailable'; send(); });
  await card.waitFor({ state: 'hidden' });
  await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.availability = 'available'; state.slurm.jobs = largeJobs; send(); });
  await card.waitFor();
  await expectCardHeight(page, chosenHeight, 'remount preserves height');

  await handle.focus();
  await handle.press('ArrowDown');
  await expectCardHeight(page, chosenHeight + 24, 'keyboard expands');
  await handle.press('Shift+ArrowUp');
  await expectCardHeight(page, chosenHeight + 24 - 80, 'shift keyboard shrinks');
  await handle.press('Home');
  await expectCardHeight(page, 280, 'minimum keyboard height');
  await page.setViewportSize({ width: 800, height: 1000 });
  await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'error'; state.slurm.error = Array(60).fill('Slurm 控制器暂时无法连接，请检查集群状态并等待自动重试。').join('\n'); send(); });
  await card.locator('.slurm-warning').waitFor();
  await handle.scrollIntoViewIfNeeded();
  const measureWarning = () => page.evaluate(() => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    const warning = card.querySelector('.slurm-warning');
    const footer = card.querySelector('.slurm-footer');
    const grip = card.querySelector('[data-testid="slurm-resize-handle"]');
    const bounds = card.getBoundingClientRect();
    const footerBounds = footer.getBoundingClientRect();
    const gripBounds = grip.getBoundingClientRect();
    const atGrip = document.elementFromPoint(gripBounds.x + gripBounds.width / 2, gripBounds.y + gripBounds.height / 2);
    return { warningHeight: warning.clientHeight, warningScrollHeight: warning.scrollHeight, footerInside: footerBounds.top >= bounds.top && footerBounds.bottom <= gripBounds.top + 1, gripInside: gripBounds.bottom <= bounds.bottom + 1, gripReachable: atGrip === grip || grip.contains(atGrip), viewport: innerWidth, document: document.documentElement.scrollWidth };
  });
  const warningLayout = await measureWarning();
  assert(warningLayout.warningHeight <= 71 && warningLayout.warningScrollHeight > warningLayout.warningHeight, `long warnings scroll within minimum cards: ${JSON.stringify(warningLayout)}`);
  assert(warningLayout.footerInside && warningLayout.gripInside && warningLayout.gripReachable, `warning must leave footer and resize grip reachable: ${JSON.stringify(warningLayout)}`);
  await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; state.slurm.jobs = []; send(); });
  await page.getByTestId('slurm-loading').waitFor();
  const loadingWarningLayout = await measureWarning();
  assert(loadingWarningLayout.footerInside && loadingWarningLayout.gripInside && loadingWarningLayout.gripReachable, `loading and warning must leave footer and grip reachable at800px: ${JSON.stringify(loadingWarningLayout)}`);
  assert(loadingWarningLayout.document <= loadingWarningLayout.viewport + 1, `loading warning causes outer overflow: ${JSON.stringify(loadingWarningLayout)}`);
  await card.screenshot({ path: path.join(output, 'slurm-card-min-warning.png') });
  await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.status = 'ready'; state.slurm.error = ''; state.slurm.jobs = largeJobs; send(); });
  await card.locator('.slurm-warning').waitFor({ state: 'hidden' });
  await page.setViewportSize({ width: 1366, height: 1500 });
  await handle.press('ArrowUp');
  await expectCardHeight(page, 280, 'minimum clamps further shrinking');
  await handle.press('End');
  await expectCardHeight(page, 2400, 'maximum keyboard height');
  await handle.press('ArrowDown');
  await expectCardHeight(page, 2400, 'maximum clamps further growing');
  await handle.press('Home');
  await handle.dblclick();
  assert.equal(await page.evaluate(key => localStorage.getItem(key), resizeStorageKey), null, 'double click restores automatic height');
  const reset = await cardMetrics(page);
  assert(reset.tableHeight <= 340, 'reset restores the default table height');
  await handle.focus();
  await handle.press('Home');
  for (let index = 0; index < 6; index++) await handle.press('Shift+ArrowDown');
  await expectCardHeight(page, 760, 'chosen restart height');
  await page.setViewportSize({ width: 880, height: 1000 });
  const narrow = await cardMetrics(page);
  assert(narrow.document <= narrow.viewport + 1, `resized page overflow: ${JSON.stringify(narrow)}`);
  await expectCardHeight(page, 760, 'window width changes preserve height');
  await page.reload();
  await card.waitFor();
  await expectCardHeight(page, 760, 'reload restores saved height');
  await card.screenshot({ path: path.join(output, 'slurm-card-resized.png') });
  return { initialHeight: initial.height, enlargedHeight: enlarged.height, smallerHeight: smaller.height, savedHeight: 760, stationaryPress: true, edgeAutoScroll: { clampedAt: 2400, scrollY: edgeScroll }, pointerCancel: true, warningLayout, loadingWarningLayout, refreshStable: true, scopeStable: true, remountStable: true, keyboard: true, reset: true, reload: true, narrow };
}

async function verifyPersistentRestart(errors) {
  const profileDirectory = await fs.mkdtemp(path.join(output, 'slurm-resize-profile-'));
  const options = { ...chromeOptions, locale: 'zh-CN', viewport: { width: 1366, height: 1500 }, colorScheme: 'light' };
  let context;
  try {
    context = await chromium.launchPersistentContext(profileDirectory, options);
    const firstPage = await context.newPage();
    firstPage.on('pageerror', error => errors.push(error.message));
    await installMockApi(firstPage);
    await firstPage.goto(fixtureUrl);
    const handle = firstPage.getByTestId('slurm-resize-handle');
    await handle.waitFor();
    await handle.focus();
    await handle.press('Home');
    for (let index = 0; index < 6; index++) await handle.press('Shift+ArrowDown');
    await expectCardHeight(firstPage, 760, 'saved before browser exit');
    assert.equal(await firstPage.evaluate(key => localStorage.getItem(key), resizeStorageKey), '760');
    await context.close();
    context = undefined;

    context = await chromium.launchPersistentContext(profileDirectory, options);
    const reopenedPage = await context.newPage();
    reopenedPage.on('pageerror', error => errors.push(error.message));
    await installMockApi(reopenedPage);
    await reopenedPage.goto(fixtureUrl);
    await reopenedPage.getByTestId('slurm-card').waitFor();
    await expectCardHeight(reopenedPage, 760, 'fresh browser process restores saved height');
    assert.equal(await reopenedPage.evaluate(key => localStorage.getItem(key), resizeStorageKey), '760');
    await reopenedPage.getByTestId('slurm-card').screenshot({ path: path.join(output, 'slurm-card-restarted.png') });
    return { passed: true, restoredHeight: 760, isolatedBrowserProcess: true };
  } finally {
    await context?.close();
    assert.equal(path.dirname(profileDirectory), output, 'temporary browser profile stays inside test-output');
    assert(path.basename(profileDirectory).startsWith('slurm-resize-profile-'));
    await fs.rm(profileDirectory, { recursive: true, force: true });
  }
}

(async () => {
  await fs.mkdir(output, { recursive: true });
  const browser = await chromium.launch(chromeOptions);
  const errors = [];
  try {
    const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 1366, height: 1000 }, colorScheme: 'light' });
    page.on('pageerror', error => errors.push(error.message));
    await installMockApi(page);
    await page.goto(fixtureUrl);
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
    const resize = await verifyResize(page);
    const restart = await verifyPersistentRestart(errors);
    assert.deepEqual(errors, []);
    await fs.writeFile(path.join(output, 'slurm-ui-smoke.json'), JSON.stringify({ passed: true, headers: 11, scopes: ['mine', 'all'], themes: ['light', 'dark'], pagination: { total: 203, pageSize: 100, refreshStable: true, shrinkClamped: true }, resize, restart, layout, errors }, null, 2));
    console.log('Slurm UI smoke passed: translated columns, scope loading, themes, contained scrolling, stale results, capability hiding, pagination, pointer/keyboard resizing and saved height after browser restart.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
