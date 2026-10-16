'use strict';

const { spawn } = require('node:child_process');
const { randomUUID } = require('node:crypto');
const path = require('node:path');
const fs = require('node:fs/promises');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const { freePort } = require('../tests/helpers/local-ssh.cjs');
const root = path.resolve(__dirname, '..');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function main() {
  if (process.platform !== 'win32') throw Error('This native WebView2 smoke check requires Windows');
  const output = path.join(root, 'test-output');
  await fs.mkdir(output, { recursive: true });
  const fixture = await fs.mkdtemp(path.join(output, 'language-desktop-'));
  const dataDir = path.join(fixture, 'data');
  await fs.mkdir(dataDir);
  const executable = process.env.LION_TEST_BINARY || path.join(root, 'src-tauri', 'target', 'debug', 'lion-ssh.exe');
  const report = { passed: false, isolatedVerificationNamespace: true, rendererErrors: [] };
  async function launch() {
    const port = await freePort();
    let exited = false;
    let log = '';
    const child = spawn(executable, ['--verification-instance'], {
      cwd: path.dirname(executable), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, LION_DATA_DIR: dataDir, LION_VERIFY_ROOT: fixture, LION_VERIFY_INSTANCE_ID: randomUUID(), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` },
    });
    const closed = new Promise(resolve => child.once('close', code => { exited = true; resolve(code); }));
    child.on('error', error => { log += error.message; });
    child.stdout.on('data', data => { log += data; });
    child.stderr.on('data', data => { log += data; });
    let browser;
    try {
      const deadline = Date.now() + 40000;
      while (Date.now() < deadline && !exited) {
        try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 700 }); break; } catch { await delay(150); }
      }
      if (!browser) throw Error(`Native app launch failed: ${log}`);
      let page;
      while (Date.now() < deadline) {
        page = browser.contexts().flatMap(context => context.pages()).find(page => page.url() !== 'about:blank');
        if (page) break;
        await delay(100);
      }
      if (!page) throw Error('Native app did not create a page');
      page.on('pageerror', error => report.rendererErrors.push(error.message));
      await page.getByTestId('language-select').waitFor({ timeout: 20000 });
      return { page, async stop() { if (!exited) child.kill(); await closed; await browser.close().catch(() => {}); } };
    } catch (error) { if (!exited) child.kill(); await closed; await browser?.close().catch(() => {}); throw error; }
  }
  let app;
  try {
    app = await launch();
    const select = app.page.getByTestId('language-select');
    await select.selectOption('en');
    await app.page.waitForFunction(() => document.documentElement.lang === 'en');
    assert.equal((JSON.parse(await fs.readFile(path.join(dataDir, 'settings.json'), 'utf8'))).language, 'en');
    assert.match(await app.page.locator('body').innerText(), /workspace/i);
    await app.stop();
    app = await launch();
    await app.page.waitForFunction(() => document.documentElement.lang === 'en');
    assert.equal(await app.page.getByTestId('language-select').inputValue(), 'en');
    await app.page.getByTestId('language-select').selectOption('zh');
    await app.page.waitForFunction(() => document.documentElement.lang === 'zh-CN');
    assert((await app.page.locator('body').innerText()).includes('连接工作台'));
    await app.page.getByTestId('language-select').selectOption('system');
    await app.page.waitForFunction(() => document.querySelector('[data-testid="language-select"]').value === 'system');
    assert.equal((JSON.parse(await fs.readFile(path.join(dataDir, 'settings.json'), 'utf8'))).language, 'system');
    assert.deepEqual(report.rendererErrors, []);
    report.passed = true;
    report.nativePersistenceAcrossRestart = true;
    report.liveLanguageSwitch = true;
    console.log('Native language desktop smoke passed: real WebView2, manual switching and saved language across app restart.');
  } finally {
    await app?.stop();
    await fs.writeFile(path.join(output, 'language-desktop-smoke.json'), JSON.stringify(report, null, 2));
    if (report.passed) await fs.rm(fixture, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
