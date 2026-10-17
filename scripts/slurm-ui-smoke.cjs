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
        state.slurm.scope = scope; state.slurm.status = 'loading'; state.slurm.jobs = []; delete state.slurm.lastUpdated; delete state.slurm.error; send();
        setTimeout(() => { state.slurm.jobs = scope === 'mine' ? ownJobs : allJobs; state.slurm.status = 'ready'; state.slurm.lastUpdated = Date.now(); send(); }, 650);
      },
      refreshSlurm: async () => {
        const jobs = window.__slurmUi.nextRefreshJobs || state.slurm.jobs;
        delete window.__slurmUi.nextRefreshJobs;
        state.slurm.status = 'loading'; send();
        setTimeout(() => { state.slurm.jobs = jobs; state.slurm.status = 'ready'; state.slurm.error = ''; state.slurm.lastUpdated = Date.now(); send(); }, 650);
      },
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

async function captureRefresh(page) {
  return page.evaluate(() => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    const table = card.querySelector('[data-testid="slurm-table"]');
    const wrap = card.querySelector('.slurm-table-wrap');
    const empty = card.querySelector('[data-testid="slurm-empty"]');
    const pagination = card.querySelector('[data-testid="slurm-pagination"]');
    const rect = element => {
      const { x, y, width, height } = element.getBoundingClientRect();
      return { x, y, width, height };
    };
    const geometry = {
      card: rect(card), footer: rect(card.querySelector('.slurm-footer')),
      content: rect(wrap || empty), scrollTop: wrap?.scrollTop || 0, scrollLeft: wrap?.scrollLeft || 0,
      windowX: scrollX, windowY: scrollY, documentHeight: document.documentElement.scrollHeight,
    };
    window.__slurmRefreshCapture = { table, wrap, empty, pagination, rows: Array.from(table?.querySelectorAll('tbody tr') || []) };
    return { geometry, count: card.querySelector('.count-tag').textContent, page: pagination?.textContent };
  });
}

async function assertRefreshStable(page, before, description) {
  const actual = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    const table = card.querySelector('[data-testid="slurm-table"]');
    const wrap = card.querySelector('.slurm-table-wrap');
    const empty = card.querySelector('[data-testid="slurm-empty"]');
    const pagination = card.querySelector('[data-testid="slurm-pagination"]');
    const old = window.__slurmRefreshCapture;
    const rows = Array.from(table?.querySelectorAll('tbody tr') || []);
    const rect = element => {
      const { x, y, width, height } = element.getBoundingClientRect();
      return { x, y, width, height };
    };
    return {
      geometry: {
        card: rect(card), footer: rect(card.querySelector('.slurm-footer')),
        content: rect(wrap || empty), scrollTop: wrap?.scrollTop || 0, scrollLeft: wrap?.scrollLeft || 0,
        windowX: scrollX, windowY: scrollY, documentHeight: document.documentElement.scrollHeight,
      },
      count: card.querySelector('.count-tag').textContent, page: pagination?.textContent,
      sameTable: table === old.table && wrap === old.wrap,
      sameRows: rows.length === old.rows.length && rows.every((row, index) => row === old.rows[index]),
      sameEmpty: empty === old.empty, samePagination: pagination === old.pagination,
      loadingPlaceholder: Boolean(card.querySelector('[data-testid="slurm-loading"]')),
    };
  });
  assert(actual.sameTable && actual.sameRows && actual.sameEmpty && actual.samePagination, `${description}: content nodes must be retained`);
  assert.equal(actual.loadingPlaceholder, false, `${description}: known results do not flash a loading placeholder`);
  assert.equal(actual.count, before.count, `${description}: total count stays visible`);
  assert.equal(actual.page, before.page, `${description}: pagination stays visible on its selected page`);
  for (const [key, value] of Object.entries(before.geometry)) {
    if (typeof value === 'number') assert(Math.abs(actual.geometry[key] - value) <= 1, `${description}: ${key} changed from ${value} to ${actual.geometry[key]}`);
    else for (const [axis, coordinate] of Object.entries(value)) assert(Math.abs(actual.geometry[key][axis] - coordinate) <= 1, `${description}: ${key}.${axis} changed from ${coordinate} to ${actual.geometry[key][axis]}`);
  }
  return { retainedNodes: true, geometry: actual.geometry, count: actual.count, page: actual.page };
}

async function measureJobRows(page) {
  return page.getByTestId('slurm-table').evaluate(table => ({
    headerHeight: table.querySelector('thead').getBoundingClientRect().height,
    columnWidths: Array.from(table.querySelectorAll('thead th'), cell => cell.getBoundingClientRect().width),
    rows: Array.from(table.querySelectorAll('tbody tr'), row => ({
      id: row.querySelector('td').textContent,
      partition: row.querySelector('td:nth-child(2)').textContent,
      height: row.getBoundingClientRect().height,
    })),
  }));
}

async function verifyDenseQueueEnd(page, description) {
  const end = await page.getByTestId('slurm-table').evaluate(table => ({
    rows: table.querySelectorAll('tbody tr').length,
    tailHeight: table.getBoundingClientRect().bottom - table.querySelector('tbody tr:last-child').getBoundingClientRect().bottom,
    fillerHeight: table.querySelector('tfoot')?.getBoundingClientRect().height || 0,
  }));
  assert.equal(end.rows, 100, `${description}: full pages contain 100 real jobs`);
  assert(end.tailHeight <= 1 && end.fillerHeight <= 1, `${description}: dense queues must not append empty scrolling space: ${JSON.stringify(end)}`);
  return end;
}

async function verifySparseGrid(page, expectedRows, description, baseline) {
  await page.waitForFunction(count => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === count, expectedRows);
  const layout = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="slurm-card"]');
    const wrap = card.querySelector('.slurm-table-wrap');
    const table = wrap.querySelector('table');
    const wrapBounds = wrap.getBoundingClientRect();
    const tableBounds = table.getBoundingClientRect();
    const rows = Array.from(table.querySelectorAll('tbody tr'));
    const rowHeights = rows.map(row => row.getBoundingClientRect().height);
    const first = rows[0].querySelector('td').getBoundingClientRect();
    const last = rows[0].querySelector('td:last-child').getBoundingClientRect();
    const filler = table.querySelector('tfoot.slurm-grid-fill');
    const fillerCells = Array.from(filler?.querySelectorAll('td') || []);
    const headers = Array.from(table.querySelectorAll('thead th'));
    return {
      cardHeight: card.getBoundingClientRect().height,
      contentHeight: wrap.clientHeight, headerHeight: table.querySelector('thead').getBoundingClientRect().height,
      rowHeights, bottomGap: wrapBounds.top + wrap.clientHeight - tableBounds.bottom,
      blankHeight: wrap.clientHeight - table.querySelector('thead').getBoundingClientRect().height - rowHeights.reduce((sum, height) => sum + height, 0),
      scrollHeight: wrap.scrollHeight, scrollWidth: wrap.scrollWidth, contentWidth: wrap.clientWidth,
      firstColumnX: first.left - wrapBounds.left, actionRight: wrapBounds.left + wrap.clientWidth - last.right,
      filler: filler && {
        hidden: filler.getAttribute('aria-hidden'), height: filler.getBoundingClientRect().height,
        text: filler.textContent.trim(), interactive: filler.querySelectorAll('button, a, input, select, [role="button"]').length,
        cells: fillerCells.map((cell, index) => {
          const bounds = cell.getBoundingClientRect();
          const header = headers[index].getBoundingClientRect();
          const style = getComputedStyle(cell);
          return { left: bounds.left, width: bounds.width, headerLeft: header.left, headerWidth: header.width, background: style.backgroundImage, backgroundColor: style.backgroundColor, leftBorder: parseFloat(style.borderLeftWidth), rightBorder: parseFloat(style.borderRightWidth) };
        }),
      },
      viewport: innerWidth, document: document.documentElement.scrollWidth,
    };
  });
  assert.equal(layout.cardHeight, 760, `${description}: saved drag height remains intact`);
  assert(Math.abs(layout.bottomGap) <= 2, `${description}: table grid should reach the content bottom: ${JSON.stringify(layout)}`);
  const natural = await measureJobRows(page);
  assert(Math.abs(natural.headerHeight - baseline.headerHeight) <= 1, `${description}: column header height stays unchanged`);
  assert.equal(natural.rows.length, baseline.rows.length, `${description}: decorative blank space does not add jobs`);
  for (let index = 0; index < natural.rows.length; index++) {
    assert.equal(natural.rows[index].id, baseline.rows[index].id, `${description}: original job order stays intact`);
    assert(Math.abs(natural.rows[index].height - baseline.rows[index].height) <= 1, `${description}: job ${natural.rows[index].id} keeps natural height ${baseline.rows[index].height}, got ${natural.rows[index].height}`);
  }
  for (let index = 0; index < natural.columnWidths.length; index++) assert(Math.abs(natural.columnWidths[index] - baseline.columnWidths[index]) <= 1, `${description}: column ${index + 1} width stays unchanged`);
  assert(layout.blankHeight > 200, `${description}: extra height remains below the real jobs`);
  assert.equal(await page.getByTestId('slurm-card').locator('.count-tag').textContent(), String(expectedRows), `${description}: only real jobs contribute to the count`);
  assert(layout.filler, `${description}: blank grid exists under the real jobs`);
  assert.equal(layout.filler.hidden, 'true', `${description}: decorative grid is hidden from assistive technology`);
  assert.equal(layout.filler.text, '', `${description}: blank grid does not contain fake job data`);
  assert.equal(layout.filler.interactive, 0, `${description}: blank grid does not add actions`);
  assert.equal(layout.filler.cells.length, 11, `${description}: blank grid follows all original columns`);
  assert(Math.abs(layout.filler.height - layout.blankHeight) <= 2, `${description}: decorative grid covers the unused height`);
  for (const [index, cell] of layout.filler.cells.entries()) {
    assert(Math.abs(cell.left - cell.headerLeft) <= 1 && Math.abs(cell.width - cell.headerWidth) <= 1, `${description}: blank grid column ${index + 1} aligns with its header`);
    assert(cell.background.includes('linear-gradient'), `${description}: blank grid column ${index + 1} has visible horizontal dividers`);
  }
  assert(layout.filler.cells[0].rightBorder >= 1 && layout.filler.cells[9].leftBorder >= 1, `${description}: vertical sticky-column borders extend through blank space`);
  for (const index of [0, 9, 10]) assert(!['rgba(0, 0, 0, 0)', 'transparent'].includes(layout.filler.cells[index].backgroundColor), `${description}: pinned blank column ${index + 1} has an opaque surface`);
  assert(layout.scrollHeight <= layout.contentHeight + 1, `${description}: sparse rows do not introduce vertical scrolling`);
  assert(layout.document <= layout.viewport + 1, `${description}: horizontal overflow stays inside the card`);
  if (layout.scrollWidth > layout.contentWidth) {
    await page.locator('.slurm-table-wrap').evaluate(element => { element.scrollLeft = element.scrollWidth; });
    const sticky = await page.locator('.slurm-table-wrap').evaluate(element => {
      const bounds = element.getBoundingClientRect();
      const row = element.querySelector('tbody tr');
      return {
        scrollLeft: element.scrollLeft,
        firstColumnX: row.querySelector('td').getBoundingClientRect().left - bounds.left,
        actionRight: bounds.left + element.clientWidth - row.querySelector('td:last-child').getBoundingClientRect().right,
        fillerColumns: Array.from(element.querySelectorAll('tfoot td'), (cell, index) => {
          const cellBounds = cell.getBoundingClientRect();
          const headerBounds = element.querySelectorAll('thead th')[index].getBoundingClientRect();
          return { left: cellBounds.left, headerLeft: headerBounds.left, width: cellBounds.width, headerWidth: headerBounds.width };
        }),
      };
    });
    assert(sticky.scrollLeft > 0 && Math.abs(sticky.firstColumnX) <= 1 && Math.abs(sticky.actionRight) <= 1, `${description}: outer columns stay visible after horizontal scrolling: ${JSON.stringify(sticky)}`);
    for (const [index, cell] of sticky.fillerColumns.entries()) assert(Math.abs(cell.left - cell.headerLeft) <= 1 && Math.abs(cell.width - cell.headerWidth) <= 1, `${description}: blank grid column ${index + 1} stays aligned after scrolling`);
  }
  return layout;
}

async function verifyResize(page, sparseBaselines) {
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

  await page.getByTestId('slurm-refresh').scrollIntoViewIfNeeded();
  const beforeRefresh = await captureRefresh(page);
  await page.getByTestId('slurm-refresh').click();
  await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
  await assertRefreshStable(page, beforeRefresh, 'resized manual refresh');
  await expectCardHeight(page, chosenHeight, 'loading preserves height');
  await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'false');
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
  await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; send(); });
  await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
  assert.equal(await page.getByTestId('slurm-loading').count(), 0, 'refresh with a warning retains the previous table');
  assert.equal(await card.locator('tbody tr').count(), 100);
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
  const denseQueueEnd = await verifyDenseQueueEnd(page, 'enlarged dense queue');
  await page.evaluate(() => { const { state, send, ownJobs } = window.__slurmUi; state.slurm.jobs = ownJobs; send(); });
  const sparseThree = await verifySparseGrid(page, 3, 'enlarged personal queue', sparseBaselines.mine);
  await page.getByTestId('slurm-scope-all').click();
  const sparseFour = await verifySparseGrid(page, 4, 'enlarged all-users queue', sparseBaselines.all);
  await card.screenshot({ path: path.join(output, 'slurm-card-sparse-grid.png') });
  await page.reload();
  await card.waitFor();
  await expectCardHeight(page, 760, 'reload restores saved height');
  const sparseReload = await verifySparseGrid(page, 3, 'saved height after reload', sparseBaselines.mine);
  await card.screenshot({ path: path.join(output, 'slurm-card-resized.png') });
  return { initialHeight: initial.height, enlargedHeight: enlarged.height, smallerHeight: smaller.height, savedHeight: 760, stationaryPress: true, edgeAutoScroll: { clampedAt: 2400, scrollY: edgeScroll }, pointerCancel: true, warningLayout, loadingWarningLayout, refreshStable: true, scopeStable: true, remountStable: true, keyboard: true, reset: true, reload: true, narrow, denseQueueEnd, sparseThree, sparseFour, sparseReload };
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
    const sparseBaseline = await measureJobRows(firstPage);
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
    await verifySparseGrid(reopenedPage, 3, 'saved height after full browser restart', sparseBaseline);
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
    const sparseBaselines = { mine: await measureJobRows(page) };
    await page.getByTestId('slurm-refresh').scrollIntoViewIfNeeded();
    await page.evaluate(() => { window.__slurmUi.nextRefreshJobs = window.__slurmUi.ownJobs.map((job, index) => index ? job : { ...job, elapsed: '2-03:11:35' }); });
    const beforeManual = await captureRefresh(page);
    await page.getByTestId('slurm-refresh').click();
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
    const manualRefresh = await assertRefreshStable(page, beforeManual, 'automatic-height manual refresh while loading');
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'false');
    await assertRefreshStable(page, beforeManual, 'automatic-height manual refresh after completion');
    assert.equal(await card.locator('tbody tr').first().locator('td').nth(5).textContent(), '2-03:11:35', 'fresh values update the retained row');
    const beforeScopeLoading = await cardMetrics(page);
    await page.getByTestId('slurm-scope-all').click();
    await page.getByTestId('slurm-loading').waitFor();
    assert.equal(await page.getByTestId('slurm-table').count(), 0, 'old-scope rows must disappear while switching');
    assert(Math.abs((await cardMetrics(page)).height - beforeScopeLoading.height) <= 1, 'uncached scope loading preserves card height');
    assert.equal(await page.getByTestId('slurm-scope-all').getAttribute('aria-pressed'), 'true');
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 4);
    assert.equal(await card.getByText('colleague', { exact: true }).count(), 1);
    sparseBaselines.all = await measureJobRows(page);
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
    const beforeEmpty = await captureRefresh(page);
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
    const emptyRefresh = await assertRefreshStable(page, beforeEmpty, 'known-empty automatic refresh');
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'ready'; send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'false');
    await assertRefreshStable(page, beforeEmpty, 'known-empty automatic refresh completion');
    await page.evaluate(() => {
      const { state, send, ownJobs } = window.__slurmUi;
      state.slurm.jobs = [
        { ...ownJobs[0], id: 'duplicate-42', partition: 'gpu-alpha', name: 'partition_alpha' },
        { ...ownJobs[0], id: 'duplicate-42', partition: 'gpu-beta', name: 'partition_beta' },
      ];
      send();
    });
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="slurm-table"] tbody tr').length === 2);
    await page.evaluate(() => {
      const rows = document.querySelectorAll('[data-testid="slurm-table"] tbody tr');
      window.__slurmDuplicateRows = { alpha: rows[0], beta: rows[1] };
      const { state, send } = window.__slurmUi;
      state.slurm.jobs = [...state.slurm.jobs].reverse(); send();
    });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-table"] tbody tr td:nth-child(2)').textContent === 'gpu-beta');
    const distinctPartitions = await page.getByTestId('slurm-table').evaluate(table => {
      const rows = table.querySelectorAll('tbody tr');
      return rows[0] === window.__slurmDuplicateRows.beta && rows[1] === window.__slurmDuplicateRows.alpha
        && rows[0].querySelector('td:nth-child(3)').textContent === 'partition_beta'
        && rows[1].querySelector('td:nth-child(3)').textContent === 'partition_alpha';
    });
    assert(distinctPartitions, 'jobs with the same ID in different partitions retain their own row nodes when reordered');
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
    await verifyDenseQueueEnd(page, 'automatic-height dense queue');
    await page.getByTestId('slurm-next-page').click();
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '2 / 3 页');
    assert.equal(await card.locator('tbody tr').first().locator('td').first().textContent(), '300100');
    await page.getByTestId('slurm-next-page').click();
    assert.equal(await card.locator('tbody tr').count(), 3);
    assert.equal(await page.getByTestId('slurm-next-page').isDisabled(), true);
    await page.getByTestId('slurm-prev-page').click();
    await page.locator('.slurm-table-wrap').evaluate(element => { element.scrollTop = 220; element.scrollLeft = 210; });
    const beforeAutomatic = await captureRefresh(page);
    assert(beforeAutomatic.geometry.scrollTop > 0 && beforeAutomatic.geometry.scrollLeft > 0, 'refresh regression starts after scrolling in both directions');
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
    const automaticRefresh = await assertRefreshStable(page, beforeAutomatic, 'paginated automatic refresh while loading');
    await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.status = 'ready'; state.slurm.jobs = largeJobs.map(job => ({ ...job, elapsed: '2-03:12:05' })); send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'false');
    await assertRefreshStable(page, beforeAutomatic, 'paginated automatic refresh after completion');
    assert.equal(await card.locator('tbody tr').first().locator('td').nth(5).textContent(), '2-03:12:05', 'automatic refresh updates values in place');
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '2 / 3 页', 'automatic refresh preserves the selected page');
    await captureRefresh(page);
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'loading'; send(); });
    await page.waitForFunction(() => document.querySelector('[data-testid="slurm-card"]').getAttribute('aria-busy') === 'true');
    await page.evaluate(() => { const { state, send } = window.__slurmUi; state.slurm.status = 'error'; state.slurm.error = '刷新读取超时'; send(); });
    await card.getByText('刷新读取超时 · 当前展示上次结果', { exact: true }).waitFor();
    assert.equal(await card.locator('.count-tag').textContent(), '203', 'failed refresh retains the last job count');
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '2 / 3 页', 'failed refresh retains the selected page');
    assert.equal(await page.getByTestId('slurm-table').evaluate(element => element === window.__slurmRefreshCapture.table), true, 'failed refresh retains the same table');
    assert.equal(await card.locator('tbody tr').count(), 100, 'failed refresh retains the last rows');
    await page.evaluate(() => { const { state, send, largeJobs } = window.__slurmUi; state.slurm.status = 'ready'; state.slurm.error = ''; state.slurm.jobs = largeJobs; send(); });
    await card.locator('.slurm-warning').waitFor({ state: 'hidden' });
    await page.getByTestId('slurm-next-page').click();
    assert.equal(await page.getByTestId('slurm-page-number').textContent(), '3 / 3 页');
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
    const resize = await verifyResize(page, sparseBaselines);
    const restart = await verifyPersistentRestart(errors);
    assert.deepEqual(errors, []);
    await fs.writeFile(path.join(output, 'slurm-ui-smoke.json'), JSON.stringify({ passed: true, headers: 11, scopes: ['mine', 'all'], themes: ['light', 'dark'], refresh: { manual: manualRefresh, automatic: automaticRefresh, knownEmpty: emptyRefresh, errorRetainsResults: true, distinctPartitionRowIdentity: distinctPartitions }, pagination: { total: 203, pageSize: 100, refreshStable: true, shrinkClamped: true }, resize, restart, layout, errors }, null, 2));
    console.log('Slurm UI smoke passed: scope isolation, stable manual/automatic/empty refresh, retained nodes and scrolling, pagination, natural row and column sizes with extended blank grids, sticky columns, pointer/keyboard resizing and saved height after browser restart.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
