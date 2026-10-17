'use strict';

// Exercise persisted connection intent through actual native-process exits and
// fresh startups. All SSH endpoints, credentials and tunnels are test fixtures.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { ServiceHarness } = require('./helpers/service-harness.cjs');
const { localSshServer, echoServer, freePort, roundTrip } = require('./helpers/local-ssh.cjs');
const { profile, tunnel, delay } = require('./helpers/fixtures.cjs');

const integration = { timeout: 60_000 };

async function fixture(t) {
  const output = path.resolve(__dirname, '../test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'connection-restore-'));
  const context = { dir, backend: null, servers: [] };
  t.after(async () => {
    await context.backend?.stop();
    await Promise.all(context.servers.map(server => server.close()));
    assert.equal(path.dirname(dir).toLowerCase(), output.toLowerCase());
    await fs.rm(dir, { recursive: true, force: true });
  });
  context.launch = () => (context.backend = new ServiceHarness(dir));
  context.ssh = async () => {
    const server = await localSshServer();
    context.servers.push(server);
    return server;
  };
  return context;
}

async function connectedIds(dir) {
  const settings = JSON.parse(await fs.readFile(path.join(dir, 'settings.json'), 'utf8'));
  return [...(settings.connectedProfileIds || [])].sort();
}

async function connect(backend, input, secrets = { password: 'test-password', remember: true }) {
  await backend.invoke('save-profile', input, secrets);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id, secrets);
  // Later profiles can share a previously pinned endpoint.
  const first = await backend.waitFor(messages => messages.slice(watermark).find(message =>
    message.payload?.profileId === input.id && (message.event === 'lion:host-key' ||
      (message.event === 'lion:state' && message.payload.status === 'connected'))), 'connection or host-key confirmation');
  if (first.event === 'lion:host-key') {
    await backend.invoke('trust-host', input.id, true, first.payload.fingerprint);
  }
  return backend.state(input.id, state => state.status === 'connected', 'authenticated fixture connection');
}

async function exitOnEof(backend) {
  backend.child.stdin.end();
  assert.equal(await backend.closed, 0, 'stdin EOF performs a clean native shutdown');
}

test('startup restores only connected profiles once, including live tunnels, and respects a later manual disconnect', integration, async t => {
  const context = await fixture(t);
  const ssh = await context.ssh();
  const inactiveSsh = await context.ssh();
  const echo = await echoServer(); context.servers.push(echo);
  const localPort = await freePort();
  const remotePort = await freePort();
  const primary = profile(ssh, {
    id: 'restore-primary', autoReconnect: false,
    tunnels: [tunnel('restored-forward', localPort, echo.port), tunnel('restored-reverse', remotePort, echo.port, 'remote')],
  });
  const secondary = profile(ssh, { id: 'restore-secondary', autoReconnect: false });
  const disconnected = profile(ssh, { id: 'manually-disconnected', autoReconnect: false });
  const inactive = profile(inactiveSsh, { id: 'never-connected', autoReconnect: false });
  let backend = context.launch();
  await backend.invoke('initial');
  await connect(backend, primary);
  await connect(backend, secondary);
  await connect(backend, disconnected);
  await backend.invoke('disconnect', disconnected.id);
  await backend.invoke('save-profile', inactive, { password: 'test-password', remember: true });
  const expected = [primary.id, secondary.id].sort();
  assert.deepEqual(await connectedIds(context.dir), expected);
  await backend.state(primary.id, state => state.tunnels.length === 2 && state.tunnels.every(item => item.status === 'running'), 'original tunnel listeners');
  assert.equal(await roundTrip(localPort, 'before-exit-中文'), 'before-exit-中文');
  const authenticatedBeforeRestart = ssh.stats.authenticated;

  await exitOnEof(backend);
  await assert.rejects(roundTrip(localPort, 'closed-forward', 1000));
  await assert.rejects(roundTrip(remotePort, 'closed-reverse', 1000));
  assert.deepEqual(await connectedIds(context.dir), expected, 'application shutdown must retain successful connection intent');
  backend = context.launch();
  await Promise.all(Array.from({ length: 4 }, () => backend.invoke('initial')));
  await backend.state(primary.id, state => state.status === 'connected' && state.tunnels.length === 2 && state.tunnels.every(item => item.status === 'running'), 'automatically restored bidirectional tunnels');
  await backend.state(secondary.id, state => state.status === 'connected', 'all previously connected servers restored');
  assert.equal(await roundTrip(localPort, 'restored-forward-中文'), 'restored-forward-中文');
  assert.equal(await roundTrip(remotePort, 'restored-reverse-中文'), 'restored-reverse-中文');
  await Promise.all(Array.from({ length: 3 }, () => backend.invoke('initial')));
  await delay(500);
  assert.equal(ssh.stats.authenticated, authenticatedBeforeRestart + 2, 'repeated initial requests must not duplicate SSH sessions');
  assert.equal(inactiveSsh.stats.connections, 0, 'a saved profile that was never connected must not start connecting');
  assert.equal(backend.messages.some(message => message.event === 'lion:host-key'), false, 'restoration reuses pinned host trust');
  const snapshot = await backend.invoke('initial');
  for (const id of [inactive.id, disconnected.id]) {
    assert.equal(snapshot.states.find(state => state.profileId === id)?.status, 'disconnected');
  }

  await backend.invoke('disconnect', primary.id);
  assert.deepEqual(await connectedIds(context.dir), [secondary.id], 'manual disconnect clears durable intent');
  await backend.stop();
  const authenticatedBeforeSecondRestart = ssh.stats.authenticated;
  backend = context.launch();
  await backend.invoke('initial');
  await backend.state(secondary.id, state => state.status === 'connected', 'remaining connected profile restores on the next launch');
  await delay(300);
  assert.equal(ssh.stats.authenticated, authenticatedBeforeSecondRestart + 1);
  assert.equal((await backend.invoke('initial')).states.find(state => state.profileId === primary.id)?.status, 'disconnected');
  await assert.rejects(roundTrip(localPort, 'manually-disconnected-stays-closed', 1000));
});

test('startup restores public-key authentication without persisting a password or passphrase', integration, async t => {
  const context = await fixture(t);
  const ssh = await context.ssh();
  const input = profile(ssh, { id: 'restore-key', authType: 'key', keyPath: ssh.keyPath, autoReconnect: false });
  let backend = context.launch();
  await backend.invoke('initial');
  await connect(backend, input, {});
  assert.deepEqual(await connectedIds(context.dir), [input.id]);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(context.dir, 'credentials.json'), 'utf8')), {});
  await exitOnEof(backend);

  const authenticatedBeforeRestart = ssh.stats.authenticated;
  backend = context.launch();
  await backend.invoke('initial');
  await backend.state(input.id, state => state.status === 'connected', 'automatic public-key authentication');
  assert.equal(ssh.stats.authenticated, authenticatedBeforeRestart + 1);
  assert.ok(ssh.stats.authenticationMethods.includes('publickey'));
  assert.equal(backend.messages.some(message => message.event === 'lion:host-key'), false);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(context.dir, 'credentials.json'), 'utf8')), {});
});

test('failed login is not remembered and startup silently skips an unsaved password without losing intent', integration, async t => {
  const context = await fixture(t);
  const ssh = await context.ssh();
  const input = profile(ssh, { id: 'restore-needs-password', autoReconnect: false });
  let backend = context.launch();
  await backend.invoke('initial');
  await backend.invoke('save-profile', input);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id, { password: 'wrong-password' });
  await backend.trust(input.id, true, watermark);
  await backend.state(input.id, state => state.status === 'error', 'failed authentication');
  assert.deepEqual(await connectedIds(context.dir), [], 'only authenticated successful connections are remembered');
  await connect(backend, input, { password: 'test-password', remember: false });
  assert.deepEqual(await connectedIds(context.dir), [input.id]);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(context.dir, 'credentials.json'), 'utf8')), {});
  await exitOnEof(backend);

  const authenticatedBeforeRestart = ssh.stats.authenticated;
  const connectionsBeforeRestart = ssh.stats.connections;
  backend = context.launch();
  const skipped = (await backend.invoke('initial')).states.find(state => state.profileId === input.id);
  assert.equal(skipped.status, 'disconnected', 'an unsaved password must leave the server offline');
  assert.equal(skipped.error ?? null, null, 'skipping a server must not report a login error');
  assert.deepEqual(skipped.events, [], 'skipping a server must not add a warning');
  await delay(300);
  assert.equal(ssh.stats.authenticated, authenticatedBeforeRestart);
  assert.equal(ssh.stats.connections, connectionsBeforeRestart, 'an unsaved password must not open an SSH transport');
  assert.equal(backend.messages.some(message => message.event === 'lion:host-key'), false, 'an unsaved password must not trigger a host-key prompt');
  assert.equal(backend.messages.some(message => message.event === 'lion:state' && message.payload.profileId === input.id), false, 'a skipped server must not enter connecting or error states');
  assert.deepEqual(await connectedIds(context.dir), [input.id], 'skipping must preserve intent for a later launch');
  for (const name of ['profiles.json', 'settings.json', 'known-hosts.json', 'credentials.json']) {
    assert.equal((await fs.readFile(path.join(context.dir, name), 'utf8')).includes('test-password'), false, `${name} must not contain the unsaved password`);
  }

  await backend.invoke('save-profile', input, { password: 'test-password', remember: true });
  const encrypted = await fs.readFile(path.join(context.dir, 'credentials.json'), 'utf8');
  assert.equal(encrypted.includes('test-password'), false);
  assert.match(JSON.parse(encrypted)[input.id], /^native:v1:/);
  await backend.stop();
  backend = context.launch();
  await backend.invoke('initial');
  await backend.state(input.id, state => state.status === 'connected', 'preserved intent retries successfully after credentials are saved');
  assert.equal(ssh.stats.authenticated, authenticatedBeforeRestart + 1);
});

test('multiple unsaved-password profiles do not block saved-password or public-key startup restoration', integration, async t => {
  const context = await fixture(t);
  const unsavedSsh = await context.ssh();
  const savedSsh = await context.ssh();
  const keySsh = await context.ssh();
  const unsaved = [
    profile(unsavedSsh, { id: 'unsaved-first', autoReconnect: false }),
    profile(unsavedSsh, { id: 'unsaved-second', autoReconnect: false }),
    profile(unsavedSsh, { id: 'password-removed', autoReconnect: false }),
  ];
  const saved = profile(savedSsh, { id: 'saved-password', autoReconnect: false });
  const key = profile(keySsh, { id: 'key-without-password', authType: 'key', keyPath: keySsh.keyPath, autoReconnect: false });
  let backend = context.launch();
  await backend.invoke('initial');
  // Interleave authentication types so skipped entries occur before and
  // between eligible servers in the durable restoration order.
  await connect(backend, unsaved[0], { password: 'test-password', remember: false });
  await connect(backend, saved);
  await connect(backend, unsaved[1], { password: 'test-password', remember: false });
  await connect(backend, key, {});
  await connect(backend, unsaved[2]);
  await backend.invoke('save-profile', unsaved[2], { remember: false });
  assert.equal((await backend.invoke('initial')).states.find(state => state.profileId === unsaved[2].id)?.status, 'connected', 'removing a saved password preserves the current session');
  const storedSecrets = JSON.parse(await fs.readFile(path.join(context.dir, 'credentials.json'), 'utf8'));
  assert.deepEqual(Object.keys(storedSecrets), [saved.id], 'only the eligible password server retains stored credentials');
  const expected = [...unsaved.map(input => input.id), saved.id, key.id].sort();
  assert.deepEqual(await connectedIds(context.dir), expected);
  await exitOnEof(backend);

  const unsavedConnections = unsavedSsh.stats.connections;
  const savedConnections = savedSsh.stats.authenticated;
  const keyConnections = keySsh.stats.authenticated;
  backend = context.launch();
  await Promise.all(Array.from({ length: 3 }, () => backend.invoke('initial')));
  await backend.state(saved.id, state => state.status === 'connected', 'saved password restores despite skipped accounts');
  await backend.state(key.id, state => state.status === 'connected', 'public key restores despite skipped accounts');
  await delay(300);
  const snapshot = await backend.invoke('initial');
  for (const input of unsaved) {
    const state = snapshot.states.find(state => state.profileId === input.id);
    assert.equal(state.status, 'disconnected', `${input.id} remains offline without a saved password`);
    assert.equal(state.error ?? null, null);
    assert.deepEqual(state.events, []);
    assert.equal(backend.messages.some(message => message.event === 'lion:state' && message.payload.profileId === input.id), false, `${input.id} must not attempt login`);
  }
  assert.equal(unsavedSsh.stats.connections, unsavedConnections, 'all unsaved profiles are excluded before opening SSH');
  assert.equal(savedSsh.stats.authenticated, savedConnections + 1, 'saved password starts exactly one connection');
  assert.equal(keySsh.stats.authenticated, keyConnections + 1, 'public key starts exactly one connection');
  assert.equal(backend.messages.some(message => message.event === 'lion:host-key'), false, 'skipped accounts do not prompt or block other restorations');
  assert.deepEqual(await connectedIds(context.dir), expected, 'credential eligibility must not erase prior connection intent');
});

test('manual disconnect still closes SSH and tunnels when recording the intent change fails', integration, async t => {
  const context = await fixture(t);
  const ssh = await context.ssh();
  const echo = await echoServer(); context.servers.push(echo);
  const bindPort = await freePort();
  const input = profile(ssh, {
    id: 'disconnect-write-failure', autoReconnect: false,
    tunnels: [tunnel('disconnect-on-write-error', bindPort, echo.port)],
  });
  const backend = context.launch();
  await backend.invoke('initial');
  await connect(backend, input);
  await backend.state(input.id, state => state.tunnels[0]?.status === 'running', 'tunnel before persistence failure');
  assert.equal(await roundTrip(bindPort, 'before-persistence-failure'), 'before-persistence-failure');
  const settingsPath = path.join(context.dir, 'settings.json');
  const backupPath = path.join(context.dir, 'settings-before-disconnect.json');
  const originalSettings = await fs.readFile(settingsPath);

  // A directory in place of the settings file reliably rejects the native
  // atomic replacement on Windows without changing permissions outside this
  // isolated fixture. Always put the original file back before cleanup.
  await fs.rename(settingsPath, backupPath);
  try {
    await fs.mkdir(settingsPath);
    try {
      await assert.rejects(backend.invoke('disconnect', input.id), /settings\.json/);
      await backend.state(input.id, state => state.status === 'disconnected', 'disconnect despite persistence failure');
      await assert.rejects(roundTrip(bindPort, 'must-be-closed', 1000));
      await backend.waitFor(() => ssh.stats.lifecycle.some(event => event.type === 'connection-close'), 'SSH transport closes despite persistence failure');
    } finally {
      await fs.rmdir(settingsPath);
    }
  } finally {
    await fs.rename(backupPath, settingsPath);
  }
  assert.deepEqual(await fs.readFile(settingsPath), originalSettings, 'failed persistence must leave the prior settings intact');
  assert.deepEqual(await connectedIds(context.dir), [input.id], 'the old on-disk intent survives a failed write');
});
