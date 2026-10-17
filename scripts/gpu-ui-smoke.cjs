'use strict';
// Real renderer with isolated GPU samples; no native service or SSH connection.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');

async function fixture(page) {
  await page.addInitScript(() => {
    const clone = value => JSON.parse(JSON.stringify(value));
    const cards = [
      { utilization: 65, memoryUsedMiB: 18119, memoryTotalMiB: 24576, idle: false, computeProcesses: 1 },
      { utilization: 0, memoryUsedMiB: 15, memoryTotalMiB: 24576, idle: true, computeProcesses: 0 },
      { utilization: null, memoryUsedMiB: null, memoryTotalMiB: null, idle: false, computeProcesses: 0 },
      { utilization: 100, memoryUsedMiB: 24576, memoryTotalMiB: 24576, idle: false, computeProcesses: 2 },
      { utilization: 2, memoryUsedMiB: 0, memoryTotalMiB: 24576, idle: true, computeProcesses: 0 },
      { utilization: 30, memoryUsedMiB: 1024, memoryTotalMiB: null, idle: false, computeProcesses: 1 },
      { utilization: 68, memoryUsedMiB: 21225, memoryTotalMiB: 24576, idle: false, computeProcesses: 1 },
      { utilization: 0, memoryUsedMiB: 15, memoryTotalMiB: 24576, idle: true, computeProcesses: 0 },
    ].map((card, index) => ({ index, uuid: `fixture-gpu-${index}`, name: 'NVIDIA GeForce RTX 4090', stableCount: 2, ...card }));
    const gpu = { enabled: true, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 };
    const profile = { id: 'gpu-fixture', name: 'GPU 服务器', host: 'gpu.example', port: 22, username: 'user', authType: 'key', keyPath: '', autoReconnect: true, gpu, tunnels: [] };
    const state = { profileId: profile.id, status: 'connected', gpu: { status: 'ready', availability: 'available', cards, total: cards.length, idleCount: 2, intervalSec: 5, lastUpdated: Date.now(), nextPollAt: Date.now() + 5000 }, tunnels: [], events: [] };
    let onState = () => {};
    let onLanguage = () => {};
    window.__gpuUi = { state, send() { onState(clone(state)); } };
    window.lion = {
      getInitial: async () => ({ profiles: [clone(profile)], states: [clone(state)], version: '1.9.2', theme: { preference: 'light', resolved: 'light' }, language: { preference: 'zh', resolved: 'zh' } }),
      setTheme: async preference => ({ preference, resolved: preference }),
      setLanguage: async preference => { const next = { preference, resolved: preference }; onLanguage(next); return next; },
      onState: callback => { onState = callback; return () => {}; },
      onLanguage: callback => { onLanguage = callback; return () => {}; },
      onTheme: () => () => {}, onHostKey: () => () => {}, onShutdown: () => () => {},
    };
  });
}

async function inspect(page, language, clamps = false) {
  const rows = await page.locator('.gpu-detail').evaluateAll(cards => cards.map(card => {
    const measure = selector => {
      const meter = card.querySelector(selector);
      const fill = meter.firstElementChild;
      const label = meter.previousElementSibling;
      const bounds = meter.getBoundingClientRect();
      const fillBounds = fill.getBoundingClientRect();
      const cardBounds = card.getBoundingClientRect();
      const labelBounds = label.getBoundingClientRect();
      return { width: bounds.width, fillWidth: fillBounds.width, percent: meter.getAttribute('aria-valuenow'), text: meter.getAttribute('aria-valuetext'), label: label.textContent, color: getComputedStyle(fill).backgroundColor, unknown: meter.classList.contains('unknown'), role: meter.getAttribute('role'), top: bounds.top, bottom: bounds.bottom, labelBottom: labelBounds.bottom, inside: bounds.left >= cardBounds.left && bounds.right <= cardBounds.right && bounds.bottom <= cardBounds.bottom };
    };
    return { utilization: measure('.utilization-meter'), memory: measure('.memory-meter') };
  }));
  assert.equal(rows.length, 8);
  const expectedUtilization = [65, 0, null, 100, 2, 30, clamps ? 100 : 68, 0];
  const expectedMemory = [18119 / 24576 * 100, 15 / 24576 * 100, null, 100, 0, null, clamps ? 100 : 21225 / 24576 * 100, clamps ? 0 : 15 / 24576 * 100];
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index];
    assert.notEqual(row.utilization.color, row.memory.color, `GPU ${index} needs distinct metric colors`);
    assert(row.memory.top > row.utilization.bottom, `GPU ${index} meters must be stacked`);
    for (const [name, expected] of [['utilization', expectedUtilization[index]], ['memory', expectedMemory[index]]]) {
      const metric = row[name];
      assert.equal(metric.role, 'progressbar');
      assert(metric.width > 100 && metric.inside, `GPU ${index} ${name} meter stays inside its card`);
      assert(metric.top >= metric.labelBottom, `GPU ${index} ${name} label precedes its meter`);
      assert(metric.label.includes(name === 'utilization' ? language === 'zh' ? 'GPU 利用率' : 'GPU utilization' : language === 'zh' ? '显存占用' : 'VRAM usage'));
      assert.equal(metric.unknown, expected === null, `GPU ${index} ${name} unknown state`);
      if (expected === null) {
        assert.equal(metric.percent, null, 'unknown values omit aria-valuenow');
        assert.equal(metric.fillWidth, 0, 'unknown values have no fake fill');
        assert.equal(metric.text, language === 'zh' ? '未知' : 'Unknown');
      } else {
        assert(Math.abs(Number(metric.percent) - expected) < 0.001, `GPU ${index} ${name} value matches sample`);
        assert(Math.abs(metric.fillWidth - metric.width * expected / 100) < 1, `GPU ${index} ${name} bar width matches sample`);
      }
    }
  }
  assert.equal(rows[1].utilization.fillWidth, 0, 'idle 0% utilization has no minimum fill');
  assert.equal(rows[4].memory.fillWidth, 0, '0 MiB has no minimum fill');
  const layout = await page.evaluate(() => ({ width: innerWidth, contentWidth: document.documentElement.scrollWidth }));
  assert(layout.contentWidth <= layout.width + 1, `page must fit the viewport: ${JSON.stringify(layout)}`);
  return { rows, layout };
}

async function main() {
  await fs.mkdir(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--allow-file-access-from-files'] });
  const page = await browser.newPage({ locale: 'zh-CN', viewport: { width: 1500, height: 1400 } });
  const errors = [];
  const reports = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await fixture(page);
    await page.goto(pathToFileURL(path.join(root, 'dist/index.html')).href);
    await page.getByTestId('gpu-card').waitFor();
    await page.getByRole('button', { name: '查看显卡详情', exact: true }).click();
    await page.getByTestId('gpu-utilization-meter').first().waitFor();
    for (const language of ['zh', 'en']) {
      await page.getByTestId('language-select').selectOption(language);
      for (const theme of ['light', 'dark']) {
        await page.getByTestId(`theme-${theme}`).click();
        await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
        for (const width of [1500, 800]) {
          await page.setViewportSize({ width, height: 1400 });
          const report = await inspect(page, language);
          reports.push({ language, theme, width, ...report });
          if (language === 'zh') await page.getByTestId('gpu-card').screenshot({ path: path.join(output, `gpu-meters-${theme}-${width}.png`), animations: 'disabled' });
        }
      }
    }
    await page.evaluate(() => {
      const { state, send } = window.__gpuUi;
      Object.assign(state.gpu.cards[6], { utilization: 150, memoryUsedMiB: 50000 });
      Object.assign(state.gpu.cards[7], { utilization: -5, memoryUsedMiB: -1 });
      send();
    });
    await page.waitForFunction(() => [...document.querySelectorAll('.gpu-meter > span')].every(fill => fill.getAnimations().every(animation => animation.playState === 'finished')));
    await inspect(page, 'en', true);
    await page.evaluate(() => { window.__gpuUi.state.status = 'disconnected'; window.__gpuUi.send(); });
    assert.equal(await page.locator('.gpu-detail.idle').count(), 0, 'stale samples do not retain idle badge styling');
    await inspect(page, 'en', true);
    assert.deepEqual(errors, []);
    await fs.writeFile(path.join(output, 'gpu-ui-smoke.json'), JSON.stringify({ passed: true, reports, errors }, null, 2));
    console.log('GPU UI smoke passed: utilization and VRAM widths, distinct colors, idle/unknown/stale samples, bilingual labels, light/dark themes and 800/1500px layouts.');
  } finally { await browser.close(); }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
