'use strict';

// The renderer uses an in-memory IPC fixture here. Native SSH is covered by
// tests/terminal.integration.test.cjs; no real server or terminal is touched.
const { chromium } = require('playwright');
const fs = require('node:fs/promises');
const path = require('node:path');
const { createHash } = require('node:crypto');
const assert = require('node:assert/strict');
const { probeText, verifyTerminalRendering, verifyTerminalClipboard, withTerminalClipboardStub, selectTerminalText, rightClickTerminal } = require('../tests/helpers/terminal-rendering.cjs');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'test-output');

async function verifyContextMenuTarget(target) {
  assert.deepEqual(await target.evaluate(element => {
    let bubbled = false;
    const onBubble = () => { bubbled = true; };
    window.addEventListener('contextmenu', onBubble);
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 });
    const accepted = element.dispatchEvent(event);
    window.removeEventListener('contextmenu', onBubble);
    return { prevented: !accepted && event.defaultPrevented, bubbled };
  }), { prevented: true, bubbled: true }, 'browser menus are suppressed without blocking event propagation');
}

(async () => {
  await fs.mkdir(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, channel: 'chrome', args: ['--allow-file-access-from-files'] });
  const page = await browser.newPage({ viewport: { width: 1380, height: 950 } });
  const errors = [];
  const cspErrors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => {
    if (message.type() === 'error' && /Content Security Policy|style-src|script-src/i.test(message.text())) cspErrors.push(message.text());
  });
  try {
    const security = JSON.parse(await fs.readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8')).app.security;
    const index = await fs.readFile(path.join(root, 'dist/index.html'), 'utf8');
    let productionCsp = security.csp;
    const disabled = security.dangerousDisableAssetCspModification;
    // Tauri adds a nonce/hash source for HTML styles unless style-src is
    // excluded. Chromium then ignores unsafe-inline, including xterm's runtime
    // styles. Reproduce that production behavior so a CSP regression cannot
    // pass merely because the browser preview omitted the policy.
    if (disabled !== true && !(Array.isArray(disabled) && disabled.includes('style-src'))) {
      const inlineStyle = index.match(/<style[^>]*>([\s\S]*?)<\/style>/i)?.[1] || '';
      const hash = `'sha256-${createHash('sha256').update(inlineStyle).digest('base64')}'`;
      productionCsp = productionCsp.replace(/(^|;)\s*style-src\s+([^;]+)/, (_, prefix, sources) => `${prefix} style-src ${sources} ${hash}`);
    }
    const origin = 'https://lion-terminal.test';
    await page.route(`${origin}/**`, async route => {
      const pathname = decodeURIComponent(new URL(route.request().url()).pathname);
      const filename = path.resolve(root, 'dist', `.${pathname === '/' ? '/index.html' : pathname}`);
      assert.ok(filename.startsWith(`${path.join(root, 'dist')}${path.sep}`));
      try {
        const body = await fs.readFile(filename);
        await route.fulfill({ status: 200, contentType: filename.endsWith('.js') ? 'text/javascript' : filename.endsWith('.css') ? 'text/css' : 'text/html', headers: { 'Content-Security-Policy': productionCsp }, body });
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
        await route.fulfill({ status: 404, body: 'Not found' });
      }
    });
    await page.addInitScript(() => {
      const clone = value => JSON.parse(JSON.stringify(value));
      const encode = bytes => {
        let binary = '';
        for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
        return btoa(binary);
      };
      const decode = data => Uint8Array.from(atob(data), char => char.charCodeAt(0));
      const profiles = ['alpha', 'beta'].map(id => ({ id, name: id === 'alpha' ? 'Alpha 服务器' : 'Beta 服务器', host: '127.0.0.1', port: 22, username: 'tester', authType: 'key', keyPath: '/fixture/key', autoReconnect: false, gpu: { enabled: false, intervalSec: 5, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 }, tunnels: [] }));
      const states = Object.fromEntries(profiles.map(profile => [profile.id, { profileId: profile.id, status: 'connected', gpu: { status: 'unknown', availability: 'unavailable', cards: [], idleCount: 0, total: 0, intervalSec: 5 }, slurm: { availability: 'unavailable', status: 'unknown', scope: 'mine', jobs: [], intervalSec: 30 }, storage: { status: 'unknown', homePath: '', homeBytes: null, tmpBytes: null, totalBytes: null, intervalSec: 300 }, tunnels: [], events: [] }]));
      let onState = () => {}, onTheme = () => {}, sequence = 0;
      const sessions = { alpha: [], beta: [] };
      const outputs = {};
      const lookup = (profileId, id) => {
        const session = sessions[profileId]?.find(item => item.id === id);
        if (!session) throw Error('模拟终端不存在');
        return session;
      };
      const append = (id, bytes) => {
        const old = outputs[id] || new Uint8Array();
        const result = new Uint8Array(old.length + bytes.length);
        result.set(old); result.set(bytes, old.length); outputs[id] = result;
      };
      const mock = window.terminalMock = {
        profiles, states, sessions, outputs, calls: [], failList: '', failOpen: false,
        emit(profileId) { onState(clone(states[profileId])); },
        emitOutput(id, text) { append(id, new TextEncoder().encode(text)); },
        disconnect(profileId) {
          states[profileId].status = 'disconnected';
          for (const session of sessions[profileId]) { session.status = 'exited'; session.exitCode = null; }
          this.emit(profileId);
        },
      };
      window.lion = {
        getInitial: async () => ({ profiles: clone(profiles), states: clone(Object.values(states)), version: '1.8.0', theme: { preference: 'dark', resolved: 'dark' } }),
        onState: callback => { onState = callback; return () => {}; },
        onTheme: callback => { onTheme = callback; return () => {}; },
        onHostKey: () => () => {}, onShutdown: () => () => {},
        setTheme: async preference => { const value = { preference, resolved: preference }; onTheme(value); return value; },
        listTerminals: async profileId => {
          mock.calls.push(['list', profileId]);
          if (mock.failList === profileId) { mock.failList = ''; throw Error('模拟终端列表加载失败'); }
          return clone(sessions[profileId]);
        },
        openTerminal: async (profileId, cols, rows) => {
          mock.calls.push(['open', profileId, cols, rows]);
          if (mock.failOpen) { mock.failOpen = false; throw Error('模拟新建终端失败'); }
          if (states[profileId].status !== 'connected') throw Error('服务器尚未连接');
          const id = `${profileId}-${++sequence}`;
          const session = { id, profileId, title: `终端 ${sequence}`, status: 'running', exitCode: null, error: null, cols, rows };
          sessions[profileId].push(session);
          append(id, new TextEncoder().encode(`\x1b[32m${profileId}\x1b[0m 中文 🦁\r\ntester@fixture:~$ `));
          return clone(session);
        },
        readTerminal: async (profileId, id, cursor) => {
          const session = lookup(profileId, id);
          mock.calls.push(['read', profileId, id, cursor]);
          const bytes = outputs[id].slice(cursor, cursor + 65536);
          return { data: encode(bytes), startCursor: cursor, cursor: cursor + bytes.length, session: clone(session) };
        },
        writeTerminal: async (profileId, id, data) => {
          const session = lookup(profileId, id);
          if (session.status !== 'running' || states[profileId].status !== 'connected') throw Error('终端已断开');
          mock.calls.push(['write', profileId, id, data]); append(id, decode(data));
        },
        resizeTerminal: async (profileId, id, cols, rows) => {
          mock.calls.push(['resize', profileId, id, cols, rows]);
          Object.assign(lookup(profileId, id), { cols, rows });
        },
        renameTerminal: async (profileId, id, title) => {
          mock.calls.push(['rename', profileId, id, title]);
          const session = lookup(profileId, id); session.title = title; return clone(session);
        },
        closeTerminal: async (profileId, id) => {
          mock.calls.push(['close', profileId, id]); lookup(profileId, id);
          sessions[profileId] = sessions[profileId].filter(item => item.id !== id); delete outputs[id];
        },
      };
    });
    await page.goto(`${origin}/`);
    await page.getByTestId('tool-terminal').click();
    await page.getByTestId('terminal-page').waitFor({ state: 'visible' });
    await page.getByTestId('terminal-tab').waitFor();
    await page.waitForFunction(() => window.terminalMock.calls.some(item => item[0] === 'read'));
    assert.equal(await page.getByTestId('terminal-tab').count(), 1, 'the first visit creates exactly one terminal');
    const firstId = await page.getByTestId('terminal-tab').first().getAttribute('data-terminal-id');
    const rendering = [];
    const probe = async id => {
      await page.evaluate(({ id, text }) => window.terminalMock.emitOutput(id, text), { id, text: probeText });
      const result = await verifyTerminalRendering(page, id);
      rendering.push(result);
      return result;
    };
    await probe(firstId);
    await verifyContextMenuTarget(page.locator('html'));
    await verifyContextMenuTarget(page.locator('.terminal-page-header'));
    await page.getByTestId('terminal-add').click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 2);
    await page.getByTestId('terminal-add').click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 3);
    const ids = await page.getByTestId('terminal-tab').evaluateAll(tabs => tabs.map(tab => tab.dataset.terminalId));
    const tab = id => page.locator(`[data-testid="terminal-tab"][data-terminal-id="${id}"]`);
    const close = id => page.locator(`[data-testid="terminal-close"][data-terminal-id="${id}"]`);
    await tab(ids[1]).dblclick();
    await verifyContextMenuTarget(page.getByTestId('terminal-title-input'));
    await page.getByTestId('terminal-title-input').fill('训练任务 🦁');
    await page.getByTestId('terminal-title-input').press('Enter');
    await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-tab"][data-terminal-id="${id}"]`)?.textContent.includes('训练任务 🦁'), ids[1]);
    await page.waitForFunction(id => window.terminalMock.calls.some(item => item[0] === 'rename' && item[2] === id && item[3] === '训练任务 🦁'), ids[1]);
    // Move focus out of an already active terminal, then click its active tab.
    // The user must be able to resume typing without clicking the terminal body.
    await page.getByTestId('theme-dark').click();
    await tab(ids[1]).click();
    await page.keyboard.type('echo hello');
    await page.keyboard.press('Enter');
    await page.keyboard.press('Control+c');
    await page.waitForFunction(id => {
      const writes = window.terminalMock.calls.filter(item => item[0] === 'write' && item[2] === id).map(item => atob(item[3])).join('');
      return writes.includes('echo hello\r\x03');
    }, ids[1]);
    assert.equal(await page.evaluate(id => window.terminalMock.calls.filter(item => item[0] === 'write' && item[2] !== id).length, ids[1]), 0, 'keyboard input goes only to the active tab');
    await probe(ids[1]);
    const clipboard = await verifyTerminalClipboard(page, ids[1]);
    assert.equal(await page.evaluate(id => window.terminalMock.calls.filter(item => item[0] === 'write' && item[2] !== id).length, ids[1]), 0, 'clipboard and keyboard input stay in the active terminal');
    const resizeCount = await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'resize').length);
    await page.setViewportSize({ width: 1120, height: 820 });
    await page.waitForFunction(count => window.terminalMock.calls.filter(item => item[0] === 'resize').length > count, resizeCount);
    assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'resize').every(item => item[3] > 0 && item[4] > 0)), true);
    await page.setViewportSize({ width: 1380, height: 950 });
    const surface = page.locator(`[data-testid="terminal-surface"][data-terminal-id="${ids[1]}"]`);
    const xtermBefore = await surface.locator('.xterm').evaluate(element => { element.dataset.retentionProof = 'same-emulator'; return element.dataset.retentionProof; });
    const opensBefore = await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'open').length);
    await page.getByTestId('terminal-back').click();
    await page.getByTestId('tools-card').waitFor({ state: 'visible' });
    await page.getByTestId('tool-terminal').click();
    await page.getByTestId('terminal-page').waitFor({ state: 'visible' });
    assert.deepEqual(await page.getByTestId('terminal-tab').evaluateAll(tabs => tabs.map(item => item.dataset.terminalId)), ids);
    assert.equal(await surface.locator('.xterm').getAttribute('data-retention-proof'), xtermBefore, 'leaving the page retains the same terminal emulator and scrollback');
    assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'open').length), opensBefore, 'returning never duplicates sessions');
    assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'close').length), 0);
    await verifyTerminalRendering(page, ids[1]);
    await page.evaluate(() => { window.terminalMock.failList = 'beta'; });
    const choose = name => page.locator('.server-item').filter({ hasText: name }).click();
    await choose('Beta 服务器');
    await page.getByTestId('terminal-error').waitFor();
    await page.getByTestId('terminal-error').getByRole('button', { name: '重试', exact: true }).click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 1);
    const betaId = await page.getByTestId('terminal-tab').first().getAttribute('data-terminal-id');
    assert.match(betaId, /^beta-/);
    await probe(betaId);
    await page.getByTestId('terminal-add').click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 2);
    assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'open' && item[1] === 'beta').length), 2, 'plus opens another shell on the currently selected server');
    const betaSecondId = await page.getByTestId('terminal-tab').last().getAttribute('data-terminal-id');
    await probe(betaSecondId);
    await choose('Alpha 服务器');
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 3);
    assert.deepEqual(await page.getByTestId('terminal-tab').evaluateAll(tabs => tabs.map(item => item.dataset.terminalId)), ids);
    await page.evaluate(id => window.terminalMock.emitOutput(id, '\r\nBACKGROUND-OUTPUT 中文 🦁\r\n'), firstId);
    await tab(firstId).click();
    await page.waitForFunction(id => window.terminalMock.calls.filter(item => item[0] === 'read' && item[2] === id).some(item => item[3] > 0), firstId);
    for (const theme of ['light', 'dark']) {
      await page.getByTestId(`theme-${theme}`).click();
      await verifyTerminalRendering(page, firstId);
      await page.screenshot({ path: path.join(output, `terminal-tool-${theme}.png`), animations: 'disabled' });
      assert.equal(await page.getByTestId('terminal-tab').count(), 3);
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await verifyTerminalRendering(page, firstId);
    await page.waitForFunction(id => {
      const cursor = document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-cursor`);
      return cursor && !cursor.classList.contains('xterm-cursor-blink') && getComputedStyle(cursor).animationName === 'none';
    }, firstId);
    await page.screenshot({ path: path.join(output, 'terminal-tool-reduced-motion.png') });
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await verifyTerminalRendering(page, firstId);
    const clipboardEdges = await withTerminalClipboardStub(page, async () => {
      const writes = () => page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'write').length);
      const dismiss = async () => { await page.getByTestId('terminal-error').getByRole('button', { name: '关闭终端提示' }).click(); };
      const settle = async () => { await page.waitForTimeout(150); };
      await page.evaluate(() => { window.__lionTerminalClipboardTest.failWrite = true; });
      await selectTerminalText(page, firstId);
      await rightClickTerminal(page, firstId);
      await page.getByTestId('terminal-error').filter({ hasText: '复制失败' }).waitFor();
      assert.equal(await page.locator(`[data-testid="terminal-surface"][data-terminal-id="${firstId}"] .xterm-selection`).evaluate(element => element.childElementCount > 0), true, 'failed copy keeps the selected text');
      await dismiss();
      await page.evaluate(() => { window.__lionTerminalClipboardTest.failWrite = false; });
      await rightClickTerminal(page, firstId);
      await page.waitForFunction(id => !document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-selection`)?.childElementCount, firstId);
      await page.evaluate(() => { window.__lionTerminalClipboardTest.failRead = true; });
      const failedPasteBefore = await writes();
      await rightClickTerminal(page, firstId);
      await page.getByTestId('terminal-error').filter({ hasText: '粘贴失败' }).waitFor();
      assert.equal(await writes(), failedPasteBefore, 'failed clipboard read sends no terminal input');
      await dismiss();
      await page.evaluate(() => { Object.assign(window.__lionTerminalClipboardTest, { failRead: false, holdReads: true }); });
      const deferredPaste = async navigate => {
        const count = await writes();
        await rightClickTerminal(page, firstId);
        await page.waitForFunction(() => window.__lionTerminalClipboardTest.pendingReads.length === 1);
        await navigate();
        await page.evaluate(() => window.__lionTerminalClipboardTest.resolveReads('DO_NOT_SEND_LATE_CLIPBOARD'));
        await settle();
        assert.equal(await writes(), count, 'late clipboard result never reaches an inactive or reactivated terminal');
      };
      await deferredPaste(async () => {
        await page.getByTestId('terminal-back').click();
        await page.getByTestId('tool-terminal').click();
        await page.getByTestId('terminal-page').waitFor({ state: 'visible' });
      });
      await deferredPaste(async () => { await tab(ids[1]).click(); await tab(firstId).click(); });
      await deferredPaste(async () => {
        await choose('Beta 服务器');
        await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 2);
        await choose('Alpha 服务器');
        await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 3);
        await tab(firstId).click();
      });
      await page.evaluate(({ id }) => {
        Object.assign(window.__lionTerminalClipboardTest, { holdReads: false, text: 'LION_BRACKETED 中文\nBRACKET_NEXT' });
        window.terminalMock.emitOutput(id, '\x1b[?2004h\r\nLION_BRACKET_MODE\r\n$ ');
      }, { id: firstId });
      await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('LION_BRACKET_MODE'), firstId);
      let before = await writes();
      await rightClickTerminal(page, firstId);
      await page.waitForFunction(count => window.terminalMock.calls.filter(item => item[0] === 'write').length > count, before);
      let bytes = await page.evaluate(count => window.terminalMock.calls.filter(item => item[0] === 'write').slice(count).map(item => atob(item[3])).join(''), before);
      assert.equal(Buffer.from(bytes, 'binary').toString('utf8'), '\x1b[200~LION_BRACKETED 中文\rBRACKET_NEXT\x1b[201~', 'xterm owns multiline and bracketed-paste encoding');
      await page.evaluate(id => {
        window.__lionTerminalClipboardTest.text = 'LION_TMUX_PASTE';
        window.terminalMock.emitOutput(id, '\x1b[?2004l\x1b[?1000h\x1b[?1006h\r\nLION_MOUSE_MODE\r\n$ ');
      }, firstId);
      await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('LION_MOUSE_MODE'), firstId);
      before = await writes();
      await rightClickTerminal(page, firstId);
      await page.waitForFunction(count => window.terminalMock.calls.filter(item => item[0] === 'write').length > count, before);
      await settle();
      bytes = await page.evaluate(count => window.terminalMock.calls.filter(item => item[0] === 'write').slice(count).map(item => atob(item[3])).join(''), before);
      assert.equal(bytes, 'LION_TMUX_PASTE', 'tmux/vim mouse mode does not receive right-button mouse reports');
      await page.evaluate(id => window.terminalMock.emitOutput(id, '\x1b[?1000l\x1b[?1006l\r\nLION_MOUSE_OFF\r\n$ '), firstId);
      await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('LION_MOUSE_OFF'), firstId);
      return { failedCopyKeepsSelection: true, failedPasteSendsNothing: true, latePastePageIsolation: true, latePasteTabIsolation: true, latePasteServerIsolation: true, bracketedPaste: true, mouseProtocolIsolation: true };
    });
    await page.evaluate(() => window.terminalMock.disconnect('alpha'));
    await page.getByTestId('terminal-connection-note').waitFor();
    assert.equal(await page.getByTestId('terminal-add').isDisabled(), true);
    assert.equal(await page.getByTestId('terminal-tab').count(), 3, 'disconnect keeps every tab');
    const writesBefore = await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'write').length);
    await page.locator(`[data-testid="terminal-surface"][data-terminal-id="${firstId}"] .xterm-helper-textarea`).focus();
    await page.keyboard.type('do-not-send');
    assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'write').length), writesBefore, 'disconnected terminals reject keyboard input');
    await withTerminalClipboardStub(page, async () => {
      await selectTerminalText(page, firstId);
      await rightClickTerminal(page, firstId);
      await page.waitForFunction(() => window.__lionTerminalClipboardTest.calls.length === 1);
      assert.equal(await page.evaluate(() => window.__lionTerminalClipboardTest.calls[0].text), 'LION_GREEN', 'disconnected terminal output can still be copied');
      await page.waitForFunction(id => !document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-selection`)?.childElementCount, firstId);
      await rightClickTerminal(page, firstId);
      await page.waitForTimeout(100);
      assert.equal(await page.evaluate(() => window.__lionTerminalClipboardTest.calls.filter(item => item.op === 'read').length), 0, 'disconnected terminal never reads clipboard text for paste');
      assert.equal(await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'write').length), writesBefore, 'disconnected right click never sends input');
    });
    clipboardEdges.disconnectedCopyOnly = true;
    await close(ids[1]).click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 2);
    assert.equal(await tab(firstId).count(), 1, 'closing one terminal preserves its siblings');
    await close(firstId).click();
    await close(ids[2]).click();
    await page.getByTestId('terminal-empty').waitFor();
    assert.equal(await page.getByTestId('terminal-tab').count(), 0);
    await page.evaluate(() => { window.terminalMock.states.alpha.status = 'connected'; window.terminalMock.emit('alpha'); });
    await page.getByTestId('terminal-back').click();
    await page.getByTestId('tool-terminal').click();
    await page.getByTestId('terminal-empty').waitFor();
    assert.equal(await page.getByTestId('terminal-tab').count(), 0, 'explicitly closing all tabs never creates a replacement on return');
    const noReopenCount = await page.evaluate(() => window.terminalMock.calls.filter(item => item[0] === 'open' && item[1] === 'alpha').length);
    assert.equal(noReopenCount, 3);
    await page.evaluate(() => { window.terminalMock.failOpen = true; });
    await page.getByTestId('terminal-add').click();
    await page.getByTestId('terminal-error').waitFor();
    assert.equal(await page.getByTestId('terminal-tab').count(), 0, 'failed creation never leaves a phantom tab');
    await page.getByTestId('terminal-add').click();
    await page.waitForFunction(() => document.querySelectorAll('[data-testid="terminal-tab"]').length === 1);
    assert.equal(await page.getByTestId('terminal-error').isVisible(), false, 'manual creation retries after failure');
    await page.setViewportSize({ width: 900, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false);
    assert.deepEqual(errors, []);
    assert.deepEqual(cspErrors, [], 'production-style CSP permits all terminal rendering styles');
    const report = { ok: true, operations: 'renderer mock only', defaultOneTerminal: true, multipleTabs: true, renameByDoubleClick: true, inputAndResize: true, activeTabRefocus: true, pageRetention: true, serverIsolation: true, noAutomaticReopenAfterClosingAll: true, loadingRetry: true, creationRetry: true, disconnectedTabsRetained: true, darkAndLightThemes: true, terminalRendering: rendering[0], renderingAcrossNewSessions: rendering.length, reducedMotionKeepsCursor: true, contextMenuPreservesPropagationOutsideTerminal: true, clipboard, clipboardEdges, productionStyleCsp: true, cspErrors, rendererErrors: errors };
    await fs.writeFile(path.join(output, 'terminal-ui-smoke.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
