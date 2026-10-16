'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const test = require('node:test');
const ts = require('typescript');

const exportsObject = {};
const file = path.join(__dirname, '../src/backend-messages.ts');
const compiled = ts.transpileModule(fs.readFileSync(file, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
vm.runInNewContext(compiled, { exports: exportsObject }, { filename: file });
const { backendMessage: message } = exportsObject;

test('known native errors and nested app errors switch while arbitrary remote details stay verbatim', () => {
  assert.equal(message('SSH 认证失败，请检查用户名和登录凭据', 'en'), 'SSH authentication failed; check the username and credentials');
  assert.equal(message('目标不可用：SSH 连接已关闭', 'en'), 'Target unavailable: SSH connection closed');
  const remote = '远端原文 $1 {0}；中文数据 /home/用户';
  assert.equal(message(remote, 'en'), remote);
  assert.equal(message(`目标不可用：${remote}`, 'en'), `Target unavailable: ${remote}`);
  assert.equal(message(`Codex CLI 已更新，但 daemon 更新失败：官方 daemon 更新命令失败（退出码 1）：${remote}`, 'en'),
    `Codex CLI updated, but the daemon update failed: The official daemon update command failed (exit code 1): ${remote}`);
  assert.equal(message('SSH 连接已关闭', 'zh'), 'SSH 连接已关闭');
});

test('dynamic templates preserve names and paths even when the capture equals a translation key', () => {
  for (const name of ['我的隧道 $1 {0}', '已取消', 'SSH 连接已关闭']) {
    assert.equal(message(`${name}：隧道已建立`, 'en'), `${name}: tunnel established`);
    assert.equal(message(`不可用节点：${name}`, 'en'), `Unavailable nodes: ${name}`);
    assert.equal(message(`程序文件 ${name} 已被修改，请先备份后手动更新`, 'en'), `Application file ${name} was modified; back it up and update manually`);
  }
  assert.equal(message('监听端口 8080 在当前设备中重复', 'en'), 'Duplicate bind port 8080 on this server');
  assert.equal(message('空闲显存阈值应在 0–1048576 之间', 'en'), 'Idle memory threshold must be between 0 and 1048576');
});

test('Slurm resource quota families and combined reasons translate without changing node details', () => {
  assert.equal(message('服务质量配额的单用户处理器请求超过上限', 'en'), 'Quality of service quota: per-user cpus requested limit exceeded');
  assert.equal(message('关联配额的内存总量已达上限', 'en'), 'Association quota: memory total limit reached');
  assert.equal(message('内存请求未达到关联配额下限', 'en'), 'Memory request is below the association quota minimum');
  assert.equal(message('超过服务质量配额的单账户节点数上限', 'en'), 'Quality of service nodes limit per account exceeded');
  assert.equal(message('等待可用资源；等待更高优先级作业', 'en'), 'Waiting for resources; Waiting for higher-priority jobs');
  assert.equal(message('等待可用资源：节点[01-05],中文名称', 'en'), 'Waiting for resources: 节点[01-05],中文名称');
  assert.equal(message('未知状态', 'en'), 'Unknown state');
  assert.equal(message('排队中', 'en'), 'Pending');
});
