'use strict';

// Real SSH channels against an opt-in loopback fixture. No user servers,
// shell commands on the test machine, or production credentials are used.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { ServiceHarness } = require('./helpers/service-harness.cjs');
const { localSshServer } = require('./helpers/local-ssh.cjs');
const { profile, delay } = require('./helpers/fixtures.cjs');

async function fixture(t, options = {}) {
  const output = path.resolve(__dirname, '../test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'terminal-backend-'));
  const ssh = await localSshServer({ terminalEnabled: true, ...options });
  const backend = new ServiceHarness(dir);
  t.after(async () => {
    await backend.stop();
    await ssh.close();
    assert.equal(path.dirname(dir).toLowerCase(), output.toLowerCase());
    await fs.rm(dir, { recursive: true, force: true });
  });
  await backend.invoke('initial');
  const input = profile(ssh, { id: 'terminal-alpha', autoReconnect: false });
  await backend.invoke('save-profile', input);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id, { password: 'test-password' });
  await backend.trust(input.id, true, watermark);
  await backend.state(input.id, state => state.status === 'connected', 'terminal SSH connection');
  return { backend, ssh, dir, input };
}

async function until(predicate, description, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await delay(25);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

async function sessionStatus(backend, profileId, id, predicate) {
  return until(async () => {
    const value = (await backend.invoke('terminal-list', profileId)).find(item => item.id === id);
    return value && predicate(value) && value;
  }, 'terminal session status');
}

async function readUntil(backend, profileId, id, predicate, cursor = 0) {
  const chunks = [];
  let latest;
  await until(async () => {
    latest = await backend.invoke('terminal-read', profileId, id, cursor);
    const bytes = Buffer.from(latest.data, 'base64');
    assert.equal(latest.cursor, latest.startCursor + bytes.length, 'read cursor counts bytes, not UTF-16 characters');
    assert.ok(bytes.length <= 64 * 1024, 'each IPC read is bounded');
    assert.ok(latest.startCursor >= cursor, 'output cursor is monotonic');
    chunks.push(bytes);
    cursor = latest.cursor;
    return predicate(Buffer.concat(chunks), latest);
  }, 'terminal output');
  return { bytes: Buffer.concat(chunks), cursor, read: latest };
}

test('interactive SSH terminals preserve PTY bytes, independent tabs, titles and closed-session history', { timeout: 40_000 }, async t => {
  const { backend, ssh, dir, input } = await fixture(t);
  const id = input.id;
  const otherProfile = profile(ssh, { id: 'terminal-beta', autoReconnect: false });
  await backend.invoke('save-profile', otherProfile);
  const files = ['profiles.json', 'credentials.json', 'known-hosts.json', 'settings.json'];
  const before = await Promise.all(files.map(file => fs.readFile(path.join(dir, file), 'utf8')));
  assert.deepEqual(await backend.invoke('terminal-list', id), []);
  const first = await backend.invoke('terminal-open', id, 100, 30);
  const second = await backend.invoke('terminal-open', id, 90, 26);
  assert.notEqual(first.id, second.id);
  assert.equal(first.profileId, id);
  assert.ok(first.title.length);
  await sessionStatus(backend, id, first.id, value => value.status === 'running');
  await sessionStatus(backend, id, second.id, value => value.status === 'running');
  await until(() => ssh.stats.terminalSessions.length === 2, 'two remote shells');
  assert.equal(ssh.stats.authenticated, 1, 'tabs reuse the authenticated SSH connection');
  assert.equal(ssh.stats.terminalSessions[0].pty.cols, 100);
  assert.equal(ssh.stats.terminalSessions[0].pty.rows, 30);
  assert.match(ssh.stats.terminalSessions[0].pty.term, /^xterm/);
  const greeting = await readUntil(backend, id, first.id, bytes => bytes.includes(Buffer.from('$ ')));
  assert.equal(greeting.bytes.toString(), '\x1b[32mfixture-1\x1b[0m 中文 🦁\r\n$ ');
  const otherGreeting = await readUntil(backend, id, second.id, bytes => bytes.includes(Buffer.from('$ ')));
  assert.match(otherGreeting.bytes.toString(), /fixture-2/);
  const inputBytes = Buffer.from('printf "你好 🦁"\r\x03\x1b[A', 'utf8');
  await backend.invoke('terminal-write', id, first.id, inputBytes.toString('base64'));
  const echoed = await readUntil(backend, id, first.id, bytes => bytes.length >= inputBytes.length, greeting.cursor);
  assert.deepEqual(echoed.bytes, inputBytes, 'ANSI, Unicode, enter and control input remain exact bytes');
  assert.equal(Buffer.concat(ssh.stats.terminalInputs.filter(item => item.id === 1).map(item => Buffer.from(item.data, 'base64'))).equals(inputBytes), true);
  const untouched = await backend.invoke('terminal-read', id, second.id, otherGreeting.cursor);
  assert.equal(untouched.data, '', 'input and output from the first shell cannot leak into the second');
  await backend.invoke('terminal-resize', id, first.id, 132, 42);
  await until(() => ssh.stats.terminalResizes.some(item => item.id === 1 && item.cols === 132 && item.rows === 42), 'remote PTY resize');
  const renamed = await backend.invoke('terminal-rename', id, first.id, '训练任务 🦁');
  assert.equal(renamed.title, '训练任务 🦁');
  assert.equal((await backend.invoke('terminal-list', id)).find(item => item.id === first.id).title, '训练任务 🦁');
  await assert.rejects(backend.invoke('terminal-write', id, first.id, '%%%invalid-base64%%%'));
  assert.deepEqual(await backend.invoke('terminal-list', otherProfile.id), []);
  await assert.rejects(backend.invoke('terminal-read', otherProfile.id, first.id, 0));
  await assert.rejects(backend.invoke('terminal-write', otherProfile.id, first.id, inputBytes.toString('base64')));
  await assert.rejects(backend.invoke('terminal-rename', otherProfile.id, first.id, 'wrong owner'));
  await assert.rejects(backend.invoke('terminal-close', otherProfile.id, first.id));
  await backend.invoke('terminal-close', id, first.id);
  await until(() => ssh.stats.terminalSessions[0].closed, 'explicit channel close');
  const remaining = await backend.invoke('terminal-list', id);
  assert.deepEqual(remaining.map(item => item.id), [second.id]);
  assert.equal(ssh.stats.terminalSessions[1].closed, false, 'closing one tab does not close the other PTY');
  await backend.invoke('terminal-write', id, second.id, Buffer.from('still running\r').toString('base64'));
  await readUntil(backend, id, second.id, bytes => bytes.toString().includes('still running'), otherGreeting.cursor);
  ssh.exitTerminal(2, 7);
  const exited = await sessionStatus(backend, id, second.id, value => value.status === 'exited');
  assert.equal(exited.exitCode, 7);
  assert.equal((await backend.invoke('terminal-list', id)).length, 1, 'remote exit retains its tab until explicitly closed');
  await assert.rejects(backend.invoke('terminal-write', id, second.id, Buffer.from('after exit').toString('base64')));
  assert.deepEqual(await Promise.all(files.map(file => fs.readFile(path.join(dir, file), 'utf8'))), before, 'transient terminal sessions and output do not change the existing data format');
});

test('terminal output is bounded with byte cursors and remains readable after SSH disconnect', { timeout: 40_000 }, async t => {
  const { backend, ssh, input } = await fixture(t, { terminalGreeting: 'READY\r\n' });
  const id = input.id;
  const session = await backend.invoke('terminal-open', id, 80, 24);
  const greeting = await readUntil(backend, id, session.id, bytes => bytes.toString().includes('READY'));
  const output = Buffer.alloc(2 * 1024 * 1024 + 32 * 1024, 0x78);
  Buffer.from('\x1b[31mOUTPUT-END 中文 🦁\x1b[0m\r\n').copy(output, output.length - Buffer.byteLength('\x1b[31mOUTPUT-END 中文 🦁\x1b[0m\r\n'));
  await ssh.writeTerminal(1, output);
  await until(async () => (await backend.invoke('terminal-read', id, session.id, 0)).startCursor > greeting.cursor, 'bounded ring drops old output');
  // Wait until the final bytes are captured without consuming or modifying
  // the server-side buffer. Repeated readers are independent byte cursors.
  let full;
  await until(async () => {
    let cursor = 0;
    const chunks = [];
    let firstCursor;
    for (let n = 0; n < 40; n++) {
      const read = await backend.invoke('terminal-read', id, session.id, cursor);
      const bytes = Buffer.from(read.data, 'base64');
      assert.ok(bytes.length <= 64 * 1024);
      firstCursor ??= read.startCursor;
      chunks.push(bytes);
      cursor = read.cursor;
      if (!bytes.length) break;
    }
    const bytes = Buffer.concat(chunks);
    if (!bytes.includes(Buffer.from('OUTPUT-END'))) return false;
    full = { bytes, firstCursor, cursor };
    return true;
  }, 'complete buffered terminal output');
  assert.equal(full.bytes.length, 2 * 1024 * 1024);
  assert.deepEqual(full.bytes, output.subarray(output.length - full.bytes.length));
  assert.equal(full.cursor, greeting.cursor + output.length);
  assert.equal(full.firstCursor, full.cursor - full.bytes.length);
  const repeat = await backend.invoke('terminal-read', id, session.id, full.firstCursor);
  assert.deepEqual(Buffer.from(repeat.data, 'base64'), full.bytes.subarray(0, 64 * 1024));
  await backend.invoke('disconnect', id);
  const ended = await sessionStatus(backend, id, session.id, value => ['exited', 'error'].includes(value.status));
  assert.equal(ended.id, session.id);
  assert.equal((await backend.invoke('terminal-list', id)).length, 1);
  const preserved = await backend.invoke('terminal-read', id, session.id, full.firstCursor);
  assert.deepEqual(Buffer.from(preserved.data, 'base64'), full.bytes.subarray(0, 64 * 1024));
  await assert.rejects(backend.invoke('terminal-open', id, 80, 24));
  await backend.invoke('terminal-close', id, session.id);
  assert.deepEqual(await backend.invoke('terminal-list', id), []);
});

test('native service shutdown closes every live PTY without leaving remote shells', { timeout: 30_000 }, async t => {
  const { backend, ssh, input } = await fixture(t);
  for (let n = 0; n < 3; n++) await backend.invoke('terminal-open', input.id, 80, 24);
  await until(() => ssh.terminalStreams.size === 3, 'three open shell channels');
  assert.equal(await backend.stop(), 0);
  await until(() => ssh.stats.terminalSessions.every(session => session.closed), 'all channels closed during shutdown');
  assert.equal(ssh.terminalStreams.size, 0);
});

test('a server that rejects PTY allocation produces an actionable error without opening a shell', { timeout: 30_000 }, async t => {
  const { backend, ssh, input } = await fixture(t, { terminalRejectPty: true });
  await assert.rejects(backend.invoke('terminal-open', input.id, 80, 24), error => Boolean(error.message));
  assert.deepEqual(await backend.invoke('terminal-list', input.id), [], 'failed PTY creation leaves no orphan reservation');
  assert.equal(ssh.terminalStreams.size, 0);
  assert.equal(ssh.stats.authenticated, 1, 'PTY failure leaves the shared SSH connection intact');
});
