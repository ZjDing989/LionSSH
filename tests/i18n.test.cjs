'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const ts = require('typescript');
const { computed } = require('vue');

function loadI18n(locale, saved = null) {
  const values = new Map(saved ? [['lion-language', saved]] : []);
  const document = { documentElement: { lang: '' }, title: '', querySelector: () => null };
  const cache = new Map();
  function load(file) {
    if (cache.has(file)) return cache.get(file);
    const result = {};
    cache.set(file, result);
    const compiled = ts.transpileModule(fs.readFileSync(file, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
    const context = { exports: result, navigator: { language: locale }, document, localStorage: { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) }, require: name => name.startsWith('.') ? load(path.resolve(path.dirname(file), `${name}.ts`)) : require(name) };
    vm.runInNewContext(compiled, context, { filename: file });
    return result;
  }
  return { ...load(path.resolve(__dirname, '../src/i18n.ts')), values, document };
}

test('Chinese system locales resolve to Chinese and all other locales to English', () => {
  const i18n = loadI18n('en-US');
  for (const locale of ['zh', 'zh-CN', 'zh-TW', 'zh-Hant-HK', 'zh_CN.UTF-8']) assert.equal(i18n.resolveLanguage('system', locale), 'zh', locale);
  for (const locale of ['en', 'en-US', 'fr-FR', 'ja-JP', 'C', '', 'zho']) assert.equal(i18n.resolveLanguage('system', locale), 'en', locale);
  assert.equal(i18n.resolveLanguage('en', 'zh-CN'), 'en');
  assert.equal(i18n.resolveLanguage('zh', 'en-US'), 'zh');
});

test('manual preference survives startup and switching updates reactive UI and document language', () => {
  const i18n = loadI18n('zh-CN', 'en');
  const label = computed(() => i18n.t('添加设备'));
  assert.equal(label.value, 'Add device');
  assert.equal(i18n.document.documentElement.lang, 'en');
  i18n.applyLanguage({ preference: 'zh', resolved: 'zh' });
  assert.equal(label.value, '添加设备');
  assert.equal(i18n.document.documentElement.lang, 'zh-CN');
  assert.equal(i18n.values.get('lion-language'), 'zh');
  i18n.applyLanguage({ preference: 'system', resolved: 'zh' });
  assert.equal(i18n.values.get('lion-language'), 'system');
  assert.equal(loadI18n('fr-FR', 'system').language.value.resolved, 'en');
});

test('interpolation preserves user values and translates app-owned terminal errors only', () => {
  const i18n = loadI18n('en-US');
  const name = '我的服务器 $1 {name}';
  assert.equal(i18n.t('编辑隧道 {0}', { 0: name }), `Edit tunnel ${name}`);
  assert.equal(i18n.message(`复制失败：${name}`), `Copy failed: ${name}`);
  assert.equal(i18n.message(name), name);
  assert.equal(i18n.t('终端 {number}', { number: 3 }), 'Terminal 3');
});
