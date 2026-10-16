'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { ServiceHarness } = require('./helpers/service-harness.cjs');

test('native language preference migrates existing settings, emits changes and survives restart', { timeout: 40_000 }, async t => {
  const output = path.resolve(__dirname, '../test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'language-native-'));
  let backend;
  t.after(async () => {
    await backend?.stop();
    await fs.rm(dir, { recursive: true, force: true });
  });
  await fs.writeFile(path.join(dir, 'settings.json'), JSON.stringify({ theme: 'dark', future: 'retained' }));
  backend = new ServiceHarness(dir);
  const initial = await backend.invoke('initial');
  assert.equal(initial.language.preference, 'system');
  assert(['zh', 'en'].includes(initial.language.resolved));
  assert.equal(initial.theme.preference, 'dark');
  assert.deepEqual(await backend.invoke('set-language', 'en'), { preference: 'en', resolved: 'en' });
  await backend.waitFor(messages => messages.some(message => message.event === 'lion:language' && message.payload.resolved === 'en'));
  assert.deepEqual(await backend.invoke('set-language', 'zh'), { preference: 'zh', resolved: 'zh' });
  await assert.rejects(backend.invoke('set-language', 'fr'));
  assert.equal((await backend.invoke('initial')).language.preference, 'zh');
  const saved = JSON.parse(await fs.readFile(path.join(dir, 'settings.json'), 'utf8'));
  assert.equal(saved.language, 'zh');
  assert.equal(saved.theme, 'dark');
  assert.equal(saved.future, 'retained');
  await backend.stop();
  backend = new ServiceHarness(dir);
  assert.deepEqual((await backend.invoke('initial')).language, { preference: 'zh', resolved: 'zh' });
  const system = await backend.invoke('set-language', 'system');
  assert.equal(system.preference, 'system');
  assert.equal(system.resolved, initial.language.resolved);
});
