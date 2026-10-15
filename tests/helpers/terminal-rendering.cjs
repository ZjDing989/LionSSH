'use strict';

const assert = require('node:assert/strict');

// Only fixtures emit this output. It exercises the public terminal rendering
// surface without reading xterm internals or changing a user's remote shell.
const probeText = '\r\n\x1b[0m\x1b[31mLION_RED\x1b[0m \x1b[32mLION_GREEN\x1b[0m \x1b[34mLION_BLUE\x1b[0m\r\n\x1b[38;5;202mLION_256\x1b[0m \x1b[38;2;12;210;100mLION_RGB\x1b[0m\r\n$ ';

async function verifyTerminalRendering(page, terminalId) {
  const selector = `[data-testid="terminal-surface"][data-terminal-id=${JSON.stringify(terminalId)}]`;
  const surface = page.locator(selector);
  await surface.waitFor({ state: 'visible' });
  await surface.locator('.xterm-helper-textarea').focus();
  await page.waitForFunction(({ selector }) => {
    const surface = document.querySelector(selector);
    const rows = surface?.querySelector('.xterm-rows');
    if (!rows) return false;
    const spans = [...rows.querySelectorAll('span')];
    const colors = [
      ['LION_RED', 'rgb(237, 135, 150)'],
      ['LION_GREEN', 'rgb(140, 212, 174)'],
      ['LION_BLUE', 'rgb(133, 172, 244)'],
      ['LION_256', 'rgb(255, 95, 0)'],
      ['LION_RGB', 'rgb(12, 210, 100)'],
    ];
    if (!colors.every(([marker, color]) => spans.some(span => span.textContent.includes(marker) && getComputedStyle(span).color === color))) return false;
    if (!/(Cascadia|Consolas|SFMono-Regular|Menlo|monospace)/i.test(getComputedStyle(rows).fontFamily)) return false;
    const cursor = surface.querySelector('.xterm-cursor');
    if (!cursor) return false;
    const bounds = cursor.getBoundingClientRect();
    const style = getComputedStyle(cursor);
    const backgroundVisible = !['transparent', 'rgba(0, 0, 0, 0)'].includes(style.backgroundColor);
    const outlineVisible = style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0 && !['transparent', 'rgba(0, 0, 0, 0)'].includes(style.outlineColor);
    return bounds.width > 0 && bounds.height > 0 && (backgroundVisible || outlineVisible);
  }, { selector }, { timeout: 10000 });
  const contextMenu = await surface.evaluate(element => {
    let bubbled = false;
    const onBubble = () => { bubbled = true; };
    window.addEventListener('contextmenu', onBubble);
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 });
    const accepted = element.dispatchEvent(event);
    window.removeEventListener('contextmenu', onBubble);
    return { prevented: !accepted && event.defaultPrevented, bubbled };
  });
  assert.deepEqual(contextMenu, { prevented: true, bubbled: false }, 'the terminal owns its right-click gesture before browser/xterm handlers');
  return { ansi16: true, ansi256: true, trueColor: true, cursorVisible: true, monospace: true, contextMenuSuppressed: true };
}

function terminalSurface(page, terminalId) {
  return page.locator(`[data-testid="terminal-surface"][data-terminal-id=${JSON.stringify(terminalId)}]`);
}

async function selectTerminalText(page, terminalId, marker = 'LION_GREEN') {
  const selector = `[data-testid="terminal-surface"][data-terminal-id=${JSON.stringify(terminalId)}]`;
  await terminalSurface(page, terminalId).waitFor({ state: 'visible' });
  // xterm replaces its text spans on a render. Waiting for a locator and then
  // measuring it in a separate browser call can catch a detached span during
  // the final render after disconnect. Measure a current glyph range in one
  // page execution, and wait for layout to settle before using the coordinates.
  const result = await page.waitForFunction(async ({ selector, marker }) => {
    const measure = () => {
      const surface = document.querySelector(selector);
      const screen = surface?.querySelector('.xterm-screen');
      if (!screen) return null;
      const screenBounds = screen.getBoundingClientRect();
      const spans = [...surface.querySelectorAll('.xterm-rows span')].reverse();
      for (const span of spans) {
        const walker = document.createTreeWalker(span, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          const start = node.textContent.indexOf(marker);
          if (start < 0) continue;
          const range = document.createRange();
          range.setStart(node, start);
          range.setEnd(node, start + marker.length);
          const bounds = range.getBoundingClientRect();
          const visible = bounds.width > 0 && bounds.height > 0 && bounds.left >= Math.max(0, screenBounds.left) && bounds.right <= Math.min(innerWidth, screenBounds.right) && bounds.top >= Math.max(0, screenBounds.top) && bounds.bottom <= Math.min(innerHeight, screenBounds.bottom);
          if (visible) return { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
        }
      }
      return null;
    };
    const before = measure();
    if (!before) return false;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const after = measure();
    if (!after || ['x', 'y', 'width', 'height'].some(key => Math.abs(before[key] - after[key]) > 0.25)) return false;
    return after;
  }, { selector, marker }, { timeout: 10000 });
  const bounds = await result.jsonValue();
  await result.dispose();
  assert.ok(bounds && bounds.width > 0 && bounds.height > 0);
  // xterm intentionally handles pointer events on its screen overlay. Use the
  // glyph boundaries for an actual drag selection, not DOM selection or a
  // synthetic event on its pointer-events:none text spans.
  const y = bounds.y + bounds.height / 2;
  await page.mouse.move(bounds.x + 1, y);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width + 1, y, { steps: 8 });
  await page.mouse.up();
  await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-selection`)?.childElementCount > 0, terminalId);
}

async function rightClickTerminal(page, terminalId) {
  // Click a blank part of the terminal rather than any output word. A right
  // click without an existing xterm selection must paste, never select a word.
  const surface = terminalSurface(page, terminalId);
  const bounds = await surface.boundingBox();
  assert.ok(bounds && bounds.width > 0 && bounds.height > 0);
  await page.mouse.click(bounds.x + bounds.width - 40, bounds.y + Math.min(bounds.height - 20, 120), { button: 'right' });
}

async function withTerminalClipboardStub(page, callback) {
  await page.evaluate(() => {
    const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    const state = window.__lionTerminalClipboardTest = {
      calls: [], text: '', failRead: false, failWrite: false, holdReads: false, pendingReads: [],
      resolveReads(text) { const pending = this.pendingReads.splice(0); for (const resolve of pending) resolve(text); },
    };
    const record = (op, text) => state.calls.push({ op, text, userActivation: navigator.userActivation?.isActive === true });
    // This stub is installed only in isolated fixture pages. Neither method
    // calls through to the desktop clipboard, including on rejection paths.
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
      writeText(text) {
        record('write', text);
        if (state.failWrite) return Promise.reject(Error('模拟复制失败'));
        state.text = text;
        return Promise.resolve();
      },
      readText() {
        record('read', state.text);
        if (state.failRead) return Promise.reject(Error('模拟粘贴失败'));
        if (state.holdReads) return new Promise(resolve => state.pendingReads.push(resolve));
        return Promise.resolve(state.text);
      },
    } });
    state.restore = () => {
      if (original) Object.defineProperty(navigator, 'clipboard', original);
      else delete navigator.clipboard;
      delete state.restore;
    };
  });
  try { return await callback(); }
  finally { await page.evaluate(() => window.__lionTerminalClipboardTest.restore()); }
}

async function verifyTerminalClipboard(page, terminalId) {
  return withTerminalClipboardStub(page, async () => {
    await selectTerminalText(page, terminalId);
    await rightClickTerminal(page, terminalId);
    await page.waitForFunction(() => window.__lionTerminalClipboardTest.calls.some(item => item.op === 'write'));
    assert.equal(await page.evaluate(() => window.__lionTerminalClipboardTest.calls[0].text), 'LION_GREEN', 'right click copies the exact xterm selection');
    await page.waitForFunction(id => !document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-selection`)?.childElementCount, terminalId);
    await page.evaluate(() => { window.__lionTerminalClipboardTest.text = 'LION_CLIPBOARD_PASTE 中文\nNEXT_LINE'; });
    await rightClickTerminal(page, terminalId);
    await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('NEXT_LINE'), terminalId);
    await selectTerminalText(page, terminalId);
    await terminalSurface(page, terminalId).locator('.xterm-helper-textarea').focus();
    await page.keyboard.press('Control+Shift+C');
    await page.waitForFunction(() => window.__lionTerminalClipboardTest.calls.filter(item => item.op === 'write').length === 2);
    assert.equal(await page.evaluate(() => window.__lionTerminalClipboardTest.calls.filter(item => item.op === 'write').at(-1).text), 'LION_GREEN');
    await page.waitForFunction(id => !document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-selection`)?.childElementCount, terminalId);
    await page.evaluate(() => { window.__lionTerminalClipboardTest.text = 'LION_KEYBOARD_PASTE'; });
    await page.keyboard.press('Control+Shift+V');
    await page.waitForFunction(id => document.querySelector(`[data-testid="terminal-surface"][data-terminal-id="${id}"] .xterm-rows`)?.textContent.includes('LION_KEYBOARD_PASTE'), terminalId);
    const calls = await page.evaluate(() => window.__lionTerminalClipboardTest.calls);
    assert.deepEqual(calls.map(item => item.op), ['write', 'read', 'write', 'read']);
    assert.equal(calls.every(item => item.userActivation), true, 'clipboard access starts synchronously inside a trusted user gesture');
    return { rightClickCopy: true, rightClickPaste: true, copyClearsSelection: true, keyboardCopy: true, keyboardPaste: true, userActivation: true, calls };
  });
}

module.exports = { probeText, verifyTerminalRendering, verifyTerminalClipboard, withTerminalClipboardStub, selectTerminalText, rightClickTerminal };
