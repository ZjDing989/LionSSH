'use strict';

// Real renderer, isolated demo data. No SSH connections or credentials are used.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const root = path.resolve(__dirname, '..');

async function fixture(page) {
  await page.addInitScript(() => {
    const now = Date.now();
    const languageState = () => {
      const preference = localStorage.getItem('fixture-language') || 'system';
      return { preference, resolved: preference === 'system' ? /^zh(?:[-_]|$)/i.test(navigator.language) ? 'zh' : 'en' : preference };
    };
    const gpu = { enabled: true, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 };
    const tunnels = [
      { id: 'jupyter', name: 'Jupyter Lab', direction: 'local', bindHost: '127.0.0.1', bindPort: 8888, targetHost: '127.0.0.1', targetPort: 8888, enabled: true, healthIntervalSec: 10 },
      { id: 'web', name: 'Development Web', direction: 'remote', bindHost: '127.0.0.1', bindPort: 8080, targetHost: '127.0.0.1', targetPort: 3000, enabled: true, healthIntervalSec: 10 },
    ];
    const profile = { id: 'demo-gpu', name: 'GPU Training Server', host: '192.0.2.10', port: 22, username: 'demo', authType: 'key', keyPath: '', autoReconnect: true, gpu, tunnels };
    const profiles = [profile, { ...profile, id: 'demo-dev', name: 'Development Server', host: '192.0.2.20', tunnels: [] }];
    const cards = Array.from({ length: 8 }, (_, index) => ({ index, uuid: `DEMO-GPU-${index}`, name: 'NVIDIA A100 80GB', memoryTotalMiB: 81920, memoryUsedMiB: index < 4 ? 0 : 49152, utilization: index < 4 ? 0 : 84, computeProcesses: index < 4 ? [] : [{ pid: 2000 + index, name: 'python', memoryUsedMiB: 49152 }], idle: index < 4, stableCount: 2 }));
    const jobs = [
      { id: '167981', partition: 'gpu', name: 'train_model', user: 'demo', stateCode: 'R', stateLabel: '运行中', stateTone: 'positive', elapsed: '2:03:11', nodes: 1, nodeList: 'gpu01', reasonCode: '', reasonLabel: '' },
      { id: '167986', partition: 'gpu-long', name: 'evaluation', user: 'demo', stateCode: 'PD', stateLabel: '排队中', stateTone: 'pending', elapsed: '0:00', nodes: 1, nodeList: '', reasonCode: 'Resources', reasonLabel: '等待可用资源' },
    ];
    const state = { profileId: profile.id, status: 'connected', gpu: { status: 'ready', availability: 'available', cards, idleCount: 4, total: 8, intervalSec: 5, lastUpdated: now, nextPollAt: now + 5000 }, slurm: { availability: 'available', status: 'ready', scope: 'mine', jobs, lastUpdated: now, intervalSec: 30 }, storage: { status: 'ready', homePath: '/home/demo', homeBytes: 64 * 1024 ** 3, tmpBytes: 32 * 1024 ** 2, totalBytes: 64 * 1024 ** 3 + 32 * 1024 ** 2, intervalSec: 300, lastUpdated: now }, tunnels: tunnels.map(item => ({ id: item.id, status: 'active', healthStatus: 'tcp-ok', healthVerified: true })), events: [] };
    const terminals = [];
    const outputs = {};
    const terminalMock = window.terminalLanguageMock = { opens: 0, renames: [], terminals };
    const terminalById = id => {
      const session = terminals.find(item => item.id === id);
      if (!session) throw Error('Fixture terminal not found');
      return session;
    };
    const encode = bytes => btoa(String.fromCharCode(...bytes));
    let onLanguage = () => {};
    window.lion = {
      getInitial: async () => ({ profiles, states: [structuredClone(state)], version: '1.9.0', theme: { preference: 'dark', resolved: 'dark' }, language: languageState() }),
      setLanguage: async preference => { localStorage.setItem('fixture-language', preference); const next = languageState(); onLanguage(next); return next; },
      onLanguage: callback => { onLanguage = callback; return () => {}; },
      setTheme: async preference => ({ preference, resolved: preference === 'system' ? 'dark' : preference }),
      onState: () => () => {}, onHostKey: () => () => {}, onTheme: () => () => {}, onShutdown: () => () => {},
      listTerminals: async profileId => structuredClone(terminals.filter(item => item.profileId === profileId)),
      openTerminal: async (profileId, cols, rows) => {
        const number = ++terminalMock.opens;
        const session = { id: `demo-terminal-${number}`, profileId, title: `终端 ${number}`, defaultTitleNumber: number, status: 'running', exitCode: null, error: null, cols, rows };
        terminals.push(session);
        outputs[session.id] = new TextEncoder().encode('远程输出保持中文 🦁\r\ndemo@gpu:~$ ');
        return structuredClone(session);
      },
      readTerminal: async (_profileId, id, cursor) => {
        const bytes = outputs[id].slice(cursor);
        return { data: encode(bytes), startCursor: cursor, cursor: cursor + bytes.length, session: structuredClone(terminalById(id)) };
      },
      writeTerminal: async () => {},
      resizeTerminal: async (_profileId, id, cols, rows) => { Object.assign(terminalById(id), { cols, rows }); },
      renameTerminal: async (_profileId, id, title) => {
        terminalMock.renames.push({ id, title });
        const session = terminalById(id);
        Object.assign(session, { title, defaultTitleNumber: null });
        return structuredClone(session);
      },
      closeTerminal: async (_profileId, id) => { terminals.splice(terminals.findIndex(item => item.id === id), 1); delete outputs[id]; },
    };
  });
}

async function verifyTerminalLanguage(page) {
  const select = page.getByTestId('language-select');
  await select.selectOption('en');
  await page.getByTestId('tool-terminal').click();
  await page.getByTestId('terminal-tab').waitFor();
  const firstTab = page.getByTestId('terminal-tab').first();
  const firstId = await firstTab.getAttribute('data-terminal-id');
  const title = firstTab.locator('.terminal-tab-title');
  await page.waitForFunction(() => document.querySelector('.terminal-tab-title')?.textContent === 'Terminal 1');
  assert.equal(await page.getByTestId('terminal-add').getAttribute('aria-label'), 'New terminal');
  const output = page.locator(`[data-testid="terminal-surface"][data-terminal-id="${firstId}"] .xterm-rows`);
  await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('远程输出保持中文 🦁'), firstId);
  const originalOutput = await output.textContent();

  await select.selectOption('zh');
  await page.waitForFunction(() => document.querySelector('.terminal-tab-title')?.textContent === '终端 1');
  assert.equal(await page.getByTestId('terminal-add').getAttribute('aria-label'), '新建终端');
  await select.selectOption('en');
  await page.waitForFunction(() => document.querySelector('.terminal-tab-title')?.textContent === 'Terminal 1');
  assert.equal(await firstTab.getAttribute('data-terminal-id'), firstId, 'switching language retains the terminal session');
  assert.equal(await output.textContent(), originalOutput, 'language switching preserves remote Chinese output');

  // A user can deliberately choose a title that looks like a Chinese default.
  // Explicit default metadata, rather than text matching, must control translation.
  await firstTab.dblclick();
  await page.getByTestId('terminal-title-input').fill('终端 1');
  await page.getByTestId('terminal-title-input').press('Enter');
  await page.waitForFunction(() => window.terminalLanguageMock.renames.length === 1);
  assert.equal(await title.innerText(), '终端 1');
  await page.getByTestId('terminal-add').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 2);
  assert.equal(await page.getByTestId('terminal-tab').last().locator('.terminal-tab-title').innerText(), 'Terminal 2');
  await select.selectOption('zh');
  await page.waitForFunction(() => document.querySelectorAll('.terminal-tab-title')[1]?.textContent === '终端 2');
  assert.equal(await title.innerText(), '终端 1', 'custom Chinese title is preserved in Chinese');
  await select.selectOption('en');
  await page.waitForFunction(() => document.querySelectorAll('.terminal-tab-title')[1]?.textContent === 'Terminal 2');
  assert.equal(await title.innerText(), '终端 1', 'custom Chinese title is preserved in English');
  assert.equal(await output.textContent(), originalOutput, 'remote Chinese output is never translated');
  assert.equal(await page.evaluate(() => window.terminalLanguageMock.opens), 2, 'language switching never reopens terminals');

  await page.evaluate(() => { const session = window.terminalLanguageMock.terminals[1]; session.status = 'exited'; session.exitCode = 9; });
  await page.waitForFunction(() => [...document.querySelectorAll('.terminal-session-notice')].some(item => item.textContent === 'The terminal session ended with exit code 9.'));
  await select.selectOption('zh');
  await page.waitForFunction(() => [...document.querySelectorAll('.terminal-session-notice')].some(item => item.textContent === '终端会话已结束，退出码 9。'));
  await select.selectOption('en');
  await page.waitForFunction(() => [...document.querySelectorAll('.terminal-session-notice')].some(item => item.textContent === 'The terminal session ended with exit code 9.'));
  await page.getByTestId('terminal-back').click();
  return { translatedDefaults: true, preservedCustomTitle: true, preservedRemoteOutput: true, localizedSessionNotice: true };
}

async function main() {
  const output = path.join(root, 'test-output');
  const screenshots = path.join(root, 'assets', 'screenshots');
  await fs.mkdir(output, { recursive: true });
  await fs.mkdir(screenshots, { recursive: true });
  const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--allow-file-access-from-files'] });
  const errors = [];
  const reports = [];
  try {
    for (const locale of ['en-US', 'zh-CN', 'zh-TW', 'ja-JP', 'de-DE']) {
      const page = await browser.newPage({ locale, viewport: { width: 1500, height: 1500 }, colorScheme: 'dark' });
      page.on('pageerror', error => errors.push(error.message));
      await fixture(page);
      await page.goto(pathToFileURL(path.join(root, 'dist', 'index.html')).href);
      await page.getByTestId('storage-card').waitFor();
      const select = page.getByTestId('language-select');
      await select.waitFor();
      const expected = locale.startsWith('zh') ? 'zh-CN' : 'en';
      await page.waitForFunction(expected => document.documentElement.lang === expected, expected);
      assert.equal(await select.inputValue(), 'system');
      const body = await page.locator('body').innerText();
      assert(locale.startsWith('zh') ? body.includes('连接工作台') : /workspace/i.test(body), `default language for ${locale}`);
      if (!locale.startsWith('zh')) {
        const visibleChinese = body.split('\n').filter(line => /[\u4e00-\u9fff]/u.test(line) && !['中文', '简体中文'].includes(line.trim()));
        assert.deepEqual(visibleChinese, [], `English workspace contains untranslated UI: ${visibleChinese.join('; ')}`);
      }
      if (locale === 'en-US' || locale === 'zh-CN') {
        await page.evaluate(() => document.fonts.ready);
        const filename = `workspace-${locale === 'en-US' ? 'en' : 'zh'}.png`;
        await page.screenshot({ path: path.join(process.argv.includes('--screenshots') ? screenshots : output, filename), fullPage: true, animations: 'disabled' });
      }
      await select.selectOption('en');
      await page.waitForFunction(() => document.documentElement.lang === 'en');
      assert((await page.locator('body').innerText()).includes('GPU Training Server'), 'language changes preserve user-provided names');
      await select.selectOption('zh');
      await page.waitForFunction(() => document.documentElement.lang === 'zh-CN');
      await page.reload();
      await page.getByTestId('storage-card').waitFor();
      await page.waitForFunction(() => document.documentElement.lang === 'zh-CN');
      assert.equal(await page.getByTestId('language-select').inputValue(), 'zh', 'saved language survives reload');
      await page.getByTestId('language-select').selectOption('system');
      await page.waitForFunction(expected => document.documentElement.lang === expected, expected);
      const terminal = locale === 'en-US' ? await verifyTerminalLanguage(page) : undefined;
      const layout = await page.evaluate(() => ({ viewport: innerWidth, content: document.documentElement.scrollWidth }));
      assert(layout.content <= layout.viewport + 1, `${locale} workspace overflows: ${JSON.stringify(layout)}`);
      reports.push({ locale, defaultLanguage: expected, manualSwitch: true, savedPreference: true, layout, ...(terminal ? { terminal } : {}) });
      await page.close();
    }
    assert.deepEqual(errors, []);
    await fs.writeFile(path.join(output, 'language-ui-smoke.json'), JSON.stringify({ passed: true, reports, errors }, null, 2));
    console.log('Language UI smoke passed: five system locales, live switching, saved selection, English UI coverage, terminal language preservation and bilingual README screenshots.');
  } finally { await browser.close(); }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
