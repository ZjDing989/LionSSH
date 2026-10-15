'use strict';

// These tests drive the compiled Rust service and a simulated loopback SSH
// server. Node.js/ssh2 are development tools only and are never distributed.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { createPrivateKey } = require('node:crypto');
const { ServiceHarness } = require('./helpers/service-harness.cjs');
const { localSshServer, echoServer, freePort, roundTrip } = require('./helpers/local-ssh.cjs');
const { profile, tunnel, delay } = require('./helpers/fixtures.cjs');
const { mineQueue, allQueue } = require('./helpers/slurm-output.cjs');
const commands = require('./fixtures/commands.json');
const packageInfo = require('../package.json');

const SCAN = 'LION_STORAGE_V1\t1000\nHOME\t4096\t3\t2\t0\nTMP\t8192\t4\t3\t0\n';
const PREVIEW = 'LION_TMP_PREVIEW_V1\t1000\nTMP\t8192\t4\t3\t0\n';
const CLEANUP = 'LION_TMP_CLEANUP_V1\t1000\nRESULT\t3\t0\n';
const jobRecord = (id, owner = 'tester', uid = 1000) => `JobId=${id} JobName=train\n   UserId=${owner}(${uid}) GroupId=research(1000)\n   Priority=12345 Nice=0 Account=research QOS=normal\n   JobState=RUNNING Reason=None\n`;
function storageResponse(command) {
  if (command === commands.storage.scan) return SCAN;
  if (command === commands.storage.preview) return PREVIEW;
  if (command === commands.storage.cleanup1000) return CLEANUP;
  throw new Error('Unexpected storage fixture command');
}
async function dataDirectory(t) {
  const output = path.resolve(__dirname, '../test-output');
  await fs.mkdir(output, { recursive: true });
  const dir = await fs.mkdtemp(path.join(output, 'native-backend-'));
  t.after(async () => {
    assert.equal(path.dirname(dir).toLowerCase(), output.toLowerCase());
    await fs.rm(dir, { recursive: true, force: true });
  });
  return dir;
}
async function fixture(t, options = {}, backendOptions = {}) {
  const dir = await dataDirectory(t);
  const ssh = await localSshServer({ squeueResponse: command => command === 'squeue' ? allQueue : mineQueue, storageResponse, ...options });
  const backend = new ServiceHarness(dir, backendOptions);
  t.after(async () => { await backend.stop(); await ssh.close(); });
  await backend.invoke('initial');
  return { dir, ssh, backend };
}
async function connect(backend, input, secrets = { password: 'test-password' }) {
  await backend.invoke('save-profile', input);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id, secrets);
  const key = await backend.trust(input.id, true, watermark);
  const state = await backend.state(input.id, value => value.status === 'connected', 'authenticated SSH');
  assert.equal(key.fingerprint.startsWith('SHA256:'), true);
  return state;
}
async function monitors(backend, id, predicate = () => true) {
  return backend.state(id, state => state.slurm.status === 'ready' && state.storage.status === 'ready' && predicate(state), 'Slurm and storage monitoring');
}

const integration = { timeout: 40_000 };

test('native service initializes the existing data format, handles errors, and preserves encrypted credentials across restarts', integration, async t => {
  const dir = await dataDirectory(t);
  let backend = new ServiceHarness(dir);
  const ssh = await localSshServer();
  t.after(async () => { await backend.stop(); await ssh.close(); });
  const initial = await backend.invoke('initial');
  assert.deepEqual(initial.profiles, []);
  assert.deepEqual(initial.states, []);
  assert.equal(initial.version, packageInfo.version);
  assert.equal(initial.theme.preference, 'system');
  for (const name of ['profiles.json', 'credentials.json', 'known-hosts.json', 'settings.json']) JSON.parse(await fs.readFile(path.join(dir, name), 'utf8'));
  await assert.rejects(backend.invoke('constructor'), /不支持/);
  await assert.rejects(backend.invoke('save-profile', { name: 'invalid' }), /认证方式/);
  await assert.rejects(backend.invoke('set-theme', 'pink'), /白天|夜晚|系统/);
  assert.deepEqual(await backend.invoke('set-theme', 'dark'), { preference: 'dark', resolved: 'dark' });
  const input = profile(ssh, { autoReconnect: false });
  const saved = await backend.invoke('save-profile', input, { password: 'test-password', remember: true });
  assert.equal(saved.hasSavedSecret, true);
  const onDisk = JSON.parse(await fs.readFile(path.join(dir, 'profiles.json'), 'utf8'));
  assert.equal(onDisk[0].id, input.id);
  assert.equal(onDisk[0].hasSavedSecret, undefined);
  assert.deepEqual(onDisk[0].gpu, input.gpu);
  const ciphertext = await fs.readFile(path.join(dir, 'credentials.json'), 'utf8');
  assert.match(JSON.parse(ciphertext)[input.id], /^native:v1:/);
  assert.equal(ciphertext.includes('test-password'), false);
  await backend.stop();
  backend = new ServiceHarness(dir);
  const restored = await backend.invoke('initial');
  assert.equal(restored.profiles[0].hasSavedSecret, true);
  assert.deepEqual(restored.theme, { preference: 'dark', resolved: 'dark' });
  await backend.invoke('connect', input.id);
  const request = await backend.waitFor(messages => messages.find(message => message.event === 'lion:host-key')?.payload, 'host trust request');
  assert.equal(request.changed, false);
  assert.equal(request.host, input.host);
  assert.equal(request.port, input.port);
  assert.deepEqual((await backend.invoke('initial')).pendingHostKeys, [request]);
  await assert.rejects(backend.invoke('trust-host', input.id, true, 'wrong-fingerprint'), /已过期/);
  await backend.invoke('trust-host', input.id, true, request.fingerprint);
  await backend.state(input.id, state => state.status === 'connected', 'saved DPAPI secret authenticates');
  assert.deepEqual((await backend.invoke('initial')).pendingHostKeys, []);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(dir, 'known-hosts.json'), 'utf8'))[`127.0.0.1:${ssh.port}`], [ssh.fingerprint]);
  await backend.invoke('disconnect', input.id);
  await backend.invoke('delete-profile', input.id);
  assert.deepEqual((await backend.invoke('initial')).profiles, []);
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(dir, 'credentials.json'), 'utf8')), {});
});

test('startup reports damaged data, recovers after repair, and never overwrites opaque legacy credentials', integration, async t => {
  const dir = await dataDirectory(t);
  await fs.writeFile(path.join(dir, 'profiles.json'), '{broken');
  const backend = new ServiceHarness(dir);
  t.after(() => backend.stop());
  await assert.rejects(backend.invoke('initial'), /无法读取配置 profiles.json/);
  assert.equal(await fs.readFile(path.join(dir, 'profiles.json'), 'utf8'), '{broken');
  const input = profile({ port: 2222 });
  const profileBytes = JSON.stringify([{ ...input, futureField: 'preserved' }]);
  await fs.writeFile(path.join(dir, 'profiles.json'), profileBytes);
  await fs.writeFile(path.join(dir, 'credentials.json'), JSON.stringify({ [input.id]: 'opaque-legacy-ciphertext' }));
  await fs.writeFile(path.join(dir, 'settings.json'), JSON.stringify({ theme: 'dark', futureField: 42 }));
  assert.equal((await backend.invoke('initial')).profiles[0].hasSavedSecret, true);
  assert.equal(await fs.readFile(path.join(dir, 'profiles.json'), 'utf8'), profileBytes);
  await assert.rejects(backend.invoke('connect', input.id), /重新输入密码或密钥口令/);
  await backend.invoke('set-theme', 'light');
  assert.equal(JSON.parse(await fs.readFile(path.join(dir, 'profiles.json'), 'utf8'))[0].futureField, 'preserved');
  assert.equal(JSON.parse(await fs.readFile(path.join(dir, 'settings.json'), 'utf8')).futureField, 42);
  assert.equal(JSON.parse(await fs.readFile(path.join(dir, 'credentials.json'), 'utf8'))[input.id], 'opaque-legacy-ciphertext');
});

test('forward and reverse tunnels carry actual bytes, verify route health, update live, and close on stdin EOF', integration, async t => {
  const { backend, ssh } = await fixture(t);
  const echo = await echoServer(); t.after(() => echo.close());
  const localPort = await freePort(); const remotePort = await freePort();
  const input = profile(ssh, { autoReconnect: false, tunnels: [tunnel('forward', localPort, echo.port), tunnel('reverse', remotePort, echo.port, 'remote')] });
  await connect(backend, input);
  const state = await backend.state(input.id, value => value.tunnels.length === 2 && value.tunnels.every(tunnel => tunnel.status === 'running' && tunnel.healthVerified), 'verified bidirectional tunnels');
  assert.ok(state.tunnels.every(tunnel => ['tcp-ok', 'probe-restricted'].includes(tunnel.healthStatus)));
  assert.equal(await roundTrip(localPort, 'Rust-forward-中文'), 'Rust-forward-中文');
  assert.equal(await roundTrip(remotePort, 'Rust-reverse-中文'), 'Rust-reverse-中文');
  await backend.invoke('set-tunnel', input.id, 'forward', false);
  await backend.state(input.id, value => value.tunnels.find(tunnel => tunnel.id === 'forward')?.status === 'stopped', 'disabled forward tunnel');
  await assert.rejects(roundTrip(localPort, 'disabled', 1000));
  assert.equal(await roundTrip(remotePort, 'reverse-still-live'), 'reverse-still-live');
  await backend.invoke('set-tunnel', input.id, 'forward', true);
  await backend.state(input.id, value => value.tunnels.find(tunnel => tunnel.id === 'forward')?.status === 'running', 'enabled forward tunnel');
  await backend.invoke('restart-tunnel', input.id, 'reverse');
  await backend.state(input.id, value => value.tunnels.find(tunnel => tunnel.id === 'reverse')?.status === 'running', 'restarted reverse tunnel');
  assert.equal(await roundTrip(remotePort, 'restarted'), 'restarted');
  backend.child.stdin.end();
  assert.equal(await backend.closed, 0);
  await assert.rejects(roundTrip(localPort, 'after-eof', 1000));
  await assert.rejects(roundTrip(remotePort, 'after-eof', 1000));
  assert.deepEqual(ssh.stats.rejectedCommands, []);
});

test('public-key authentication and pinned trust reconnect without another trust prompt', integration, async t => {
  const { backend, ssh } = await fixture(t);
  const input = profile(ssh, { authType: 'key', keyPath: ssh.keyPath, autoReconnect: false });
  await connect(backend, input, {});
  assert.ok(ssh.stats.authenticationMethods.includes('publickey'));
  await backend.invoke('disconnect', input.id);
  const watermark = backend.messages.length;
  await backend.invoke('connect', input.id);
  await backend.state(input.id, state => state.status === 'connected', 'reconnect using pinned public key');
  assert.equal(backend.messages.slice(watermark).some(message => message.event === 'lion:host-key'), false);
  assert.ok(ssh.stats.authenticated >= 2);
});

test('encrypted private keys accept saved passphrases after a native backend restart', integration, async t => {
  const dir = await dataDirectory(t);
  const ssh = await localSshServer();
  let backend = new ServiceHarness(dir);
  t.after(async () => { await backend.stop(); await ssh.close(); });
  const passphrase = 'fixture-key-passphrase-秘密';
  const keyPath = path.join(dir, 'fixture-encrypted-private-key.pem');
  const key = createPrivateKey(await fs.readFile(ssh.keyPath));
  await fs.writeFile(keyPath, key.export({ type: 'pkcs8', format: 'pem', cipher: 'aes-256-cbc', passphrase }));
  const input = profile(ssh, { authType: 'key', keyPath, autoReconnect: false });
  await backend.invoke('save-profile', input, { passphrase, remember: true });
  const credentials = await fs.readFile(path.join(dir, 'credentials.json'), 'utf8');
  assert.equal(credentials.includes(passphrase), false);
  assert.match(JSON.parse(credentials)[input.id], /^native:v1:/);
  await backend.stop();
  backend = new ServiceHarness(dir);
  await backend.invoke('connect', input.id);
  await backend.trust(input.id);
  await backend.state(input.id, state => state.status === 'connected', 'saved encrypted-key passphrase authenticates');
  assert.ok(ssh.stats.authenticationMethods.includes('publickey'));
});

test('host-key rejection and changed pinned fingerprints require a fresh confirmation', integration, async t => {
  const { backend, ssh, dir } = await fixture(t);
  const input = profile(ssh, { autoReconnect: false });
  await backend.invoke('save-profile', input);
  await backend.invoke('connect', input.id, { password: 'test-password' });
  const request = await backend.trust(input.id, false);
  await backend.state(input.id, state => state.status === 'error', 'rejected host key');
  assert.equal(ssh.stats.authenticated, 0);
  await assert.rejects(backend.invoke('trust-host', input.id, true, request.fingerprint), /已过期/);
  await backend.stop();
  await fs.writeFile(path.join(dir, 'known-hosts.json'), JSON.stringify({ [`127.0.0.1:${ssh.port}`]: ['SHA256:changed-old-key'] }));
  const changed = new ServiceHarness(dir); t.after(() => changed.stop());
  await changed.invoke('connect', input.id, { password: 'test-password' });
  const warning = await changed.trust(input.id, false);
  assert.equal(warning.changed, true);
  await changed.state(input.id, state => state.status === 'error', 'changed host key rejected');
  assert.equal(ssh.stats.authenticated, 0);
});

test('wrong passwords fail while another device can still connect and transport reconnect restores rules', integration, async t => {
  const { backend, ssh } = await fixture(t);
  const echo = await echoServer(); t.after(() => echo.close());
  const bindPort = await freePort();
  const wrong = profile(ssh, { id: 'wrong-password', autoReconnect: false });
  await backend.invoke('save-profile', wrong);
  await backend.invoke('connect', wrong.id, { password: 'wrong-password' });
  await backend.trust(wrong.id);
  await backend.state(wrong.id, value => value.status === 'error', 'authentication failure');
  const input = profile(ssh, { tunnels: [tunnel('reconnect-forward', bindPort, echo.port)] });
  // The endpoint is already pinned from the first explicit confirmation.
  await backend.invoke('save-profile', input);
  await backend.invoke('connect', input.id, { password: 'test-password' });
  await backend.state(input.id, value => value.status === 'connected' && value.tunnels[0]?.status === 'running', 'healthy independent device');
  assert.equal(await roundTrip(bindPort, 'before-drop'), 'before-drop');
  const authenticated = ssh.stats.authenticated;
  ssh.disconnectClients();
  await backend.state(input.id, value => value.status === 'reconnecting', 'transport reconnect starts');
  await backend.state(input.id, value => value.status === 'connected' && value.tunnels[0]?.status === 'running' && ssh.stats.authenticated > authenticated, 'transport reconnect restores tunnel', 20_000);
  assert.equal(await roundTrip(bindPort, 'after-reconnect'), 'after-reconnect');
  await backend.invoke('disconnect', input.id);
  await assert.rejects(roundTrip(bindPort, 'after-disconnect', 1000));
});

test('GPU CSV monitoring confirms idle cards, falls back to summary, and keeps distinct capability states', integration, async t => {
  const { backend, ssh } = await fixture(t);
  const gpu = { enabled: true, intervalSec: 2, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 };
  const input = profile(ssh, { autoReconnect: false, gpu });
  await connect(backend, input);
  const state = await backend.state(input.id, value => value.gpu.status === 'ready' && value.gpu.idleCount === 1, 'initial stable idle GPU samples');
  assert.equal(state.gpu.queryMode, 'csv'); assert.equal(state.gpu.total, 2);
  assert.equal(state.gpu.cards.find(card => card.uuid === 'GPU-busy').idle, false);
  ssh.setNvidiaResponse(command => command === 'nvidia-smi' ? undefined : { stderr: 'Structured query restricted by allocation\n', exitCode: 1 });
  await backend.invoke('refresh-gpu', input.id);
  const fallback = await backend.state(input.id, value => value.gpu.status === 'ready' && value.gpu.queryMode === 'summary', 'summary fallback');
  assert.equal(fallback.gpu.total, 8);
  ssh.setNvidiaResponse({ stderr: 'Insufficient Permissions\n', exitCode: 4 });
  await backend.invoke('refresh-gpu', input.id);
  const denied = await backend.state(input.id, value => value.gpu.status === 'error', 'temporary GPU permission failure');
  assert.equal(denied.gpu.availability, 'available');
  assert.equal(denied.gpu.total, 8, 'a temporary permission error must preserve previous valid card data');
  for (const [availability, response] of [
    ['absent', { stdout: 'No devices were found\n', exitCode: 6 }],
    ['unavailable', { stderr: '/bin/sh: 1: nvidia-smi: not found\n', exitCode: 127 }],
  ]) {
    ssh.setNvidiaResponse(response);
    await backend.invoke('refresh-gpu', input.id);
    const unavailable = await backend.state(input.id, value => value.gpu.availability === availability, `GPU ${availability}`);
    assert.equal(unavailable.status, 'connected');
    assert.equal(unavailable.gpu.total, 0); assert.deepEqual(unavailable.gpu.cards, []);
    assert.ok(unavailable.gpu.availabilityReason);
    assert.ok(unavailable.gpu.nextPollAt - unavailable.gpu.availabilityCheckedAt >= 59_000);
  }
  await backend.invoke('update-gpu', input.id, { ...gpu, enabled: false });
  const count = ssh.stats.commands.length; await delay(350);
  assert.equal(ssh.stats.commands.length, count);
  assert.deepEqual(ssh.stats.rejectedCommands, []);
});

test('Slurm scopes, priority, owner rechecks, injected identifiers and one-use cancellation confirmations remain guarded', integration, async t => {
  let owner = 'tester'; let cancelled = false;
  const { backend, ssh } = await fixture(t, {
    squeueResponse: command => (command === 'squeue' ? allQueue : mineQueue).replace(cancelled ? /^.*4201.*\n/m : /$^/, ''),
    jobResponse: command => {
      if (command === commands.jobs['4201'].info) return jobRecord('4201', owner);
      if (command === commands.jobs['4203'].info) return jobRecord('4203', 'another');
      if (command === commands.jobs['4203'].priority) return 'Priority=98765\n';
      if (command === commands.jobs['4201'].cancel) { cancelled = true; return ''; }
      return { stderr: 'Invalid job id specified\n', exitCode: 1 };
    },
  });
  const input = profile(ssh, { autoReconnect: false }); await connect(backend, input); await monitors(backend, input.id);
  assert.ok(ssh.stats.slurmCommands.includes('squeue -u tester'));
  await backend.invoke('set-slurm-scope', input.id, 'all');
  await backend.state(input.id, value => value.slurm.scope === 'all' && value.slurm.jobs.length === 3, 'all Slurm jobs');
  assert.ok(ssh.stats.slurmCommands.includes('squeue'));
  assert.deepEqual(await backend.invoke('get-job-priority', input.id, '4203'), { jobId: '4203', priority: 98765, priorities: [98765] });
  await assert.rejects(backend.invoke('prepare-job-cancel', input.id, '4203'), /自己的作业|所属用户/);
  await assert.rejects(backend.invoke('cancel-job', input.id, '4201', 'unconfirmed'), /确认已失效/);
  for (const id of ['4201; id', '4201\nwhoami', '$(id)', '-1']) await assert.rejects(backend.invoke('get-job-priority', input.id, id), /作业 ID/);
  const previous = await backend.invoke('prepare-job-cancel', input.id, '4201');
  assert.equal(previous.owner, 'tester'); assert.ok(previous.expiresAt > Date.now());
  assert.equal(cancelled, false);
  owner = 'another';
  await assert.rejects(backend.invoke('cancel-job', input.id, '4201', previous.token), /自己的作业|所属用户/);
  assert.equal(cancelled, false);
  owner = 'tester';
  const preparation = await backend.invoke('prepare-job-cancel', input.id, '4201');
  const result = await backend.invoke('cancel-job', input.id, '4201', preparation.token);
  assert.deepEqual(result, { jobId: '4201', cancelled: true });
  await assert.rejects(backend.invoke('cancel-job', input.id, '4201', preparation.token), /确认已失效|最新作业列表/);
  assert.equal(ssh.stats.jobCommands.filter(command => command.startsWith('scancel ')).length, 1);
  assert.deepEqual(ssh.stats.rejectedCommands, []);
});

test('storage scan, guarded cleanup preview, root UID rejection and one-use tokens preserve the fixed command contract', integration, async t => {
  let previewUid = 1000; let cleanupUid = 1000;
  const { backend, ssh } = await fixture(t, { storageResponse: command => {
    if (command === commands.storage.scan) return SCAN;
    if (command === commands.storage.preview) return PREVIEW.replace('1000', String(previewUid));
    if (command === commands.storage.cleanup1000) return CLEANUP.replace('1000', String(cleanupUid));
    throw new Error('Unexpected storage command');
  } });
  const input = profile(ssh, { autoReconnect: false }); await connect(backend, input);
  const state = await monitors(backend, input.id);
  assert.equal(state.storage.homePath, '/home/tester');
  assert.equal(state.storage.homeBytes, 4096); assert.equal(state.storage.tmpBytes, 8192); assert.equal(state.storage.totalBytes, 12288);
  await assert.rejects(backend.invoke('cleanup-tmp', input.id, 'unconfirmed'), /确认已失效/);
  const preview = await backend.invoke('prepare-tmp-cleanup', input.id);
  assert.equal(preview.root, '/tmp'); assert.equal(preview.username, 'tester'); assert.equal(preview.uid, 1000);
  assert.equal(preview.fileCount, 3); assert.equal(preview.bytes, 8192); assert.ok(preview.expiresAt > Date.now());
  assert.equal(ssh.stats.storageCommands.filter(command => command === commands.storage.cleanup1000).length, 0);
  const result = await backend.invoke('cleanup-tmp', input.id, preview.token);
  assert.equal(result.deletedCount, 3); assert.equal(result.cleanupStatus, 'complete');
  await assert.rejects(backend.invoke('cleanup-tmp', input.id, preview.token), /确认已失效/);
  previewUid = 0;
  await assert.rejects(backend.invoke('prepare-tmp-cleanup', input.id), /root|UID/);
  previewUid = 1000;
  const mismatch = await backend.invoke('prepare-tmp-cleanup', input.id);
  cleanupUid = 1001;
  await assert.rejects(backend.invoke('cleanup-tmp', input.id, mismatch.token), /UID 不一致/);
  await assert.rejects(backend.invoke('cleanup-tmp', input.id, mismatch.token), /确认已失效/);
  assert.deepEqual(ssh.stats.rejectedCommands, []);
});

test('disconnect cancels a stalled remote query promptly and invalidates confirmations across reconnects', integration, async t => {
  const { backend, ssh } = await fixture(t, { jobResponse: command => command === commands.jobs['4201'].info ? jobRecord('4201') : { hang: true } });
  const input = profile(ssh, { autoReconnect: false }); await connect(backend, input); await monitors(backend, input.id);
  const job = await backend.invoke('prepare-job-cancel', input.id, '4201');
  const cleanup = await backend.invoke('prepare-tmp-cleanup', input.id);
  const pending = backend.invoke('get-job-priority', input.id, '4201').then(() => ({ ok: true }), error => ({ error }));
  await backend.waitFor(() => ssh.stats.jobCommands.includes(commands.jobs['4201'].priority), 'stalled query reached server');
  const start = Date.now(); await backend.invoke('disconnect', input.id);
  assert.ok(Date.now() - start < 5000);
  assert.ok((await pending).error);
  await backend.invoke('connect', input.id);
  await backend.state(input.id, value => value.status === 'connected', 'reconnected SSH'); await monitors(backend, input.id);
  await assert.rejects(backend.invoke('cancel-job', input.id, '4201', job.token), /确认已失效/);
  await assert.rejects(backend.invoke('cleanup-tmp', input.id, cleanup.token), /确认已失效/);
  assert.equal(ssh.stats.jobCommands.some(command => command.startsWith('scancel ')), false);
  assert.equal(ssh.stats.storageCommands.some(command => command.startsWith('# Lion tmp cleanup')), false);
});

test('forwarding-only diagnostics disable all remote execution channels', integration, async t => {
  const { backend, ssh } = await fixture(t, {}, { env: { LION_SLURM_DISABLED: '1', LION_STORAGE_DISABLED: '1' } });
  const input = profile(ssh, { autoReconnect: false }); await connect(backend, input);
  await delay(350); assert.deepEqual(ssh.stats.allCommands, []);
});

test('Codex RPC stays profile-scoped and connecting never starts a remote installation', integration, async t => {
  const { backend, ssh } = await fixture(t);
  const input = profile(ssh, { autoReconnect: false });
  await backend.invoke('save-profile', input);
  for (const method of ['refresh-codex', 'install-codex']) {
    await assert.rejects(backend.invoke(method, input.id), /请先连接设备/);
    await assert.rejects(backend.invoke(method, input.id, 'curl arbitrary-command'), /只接受设备标识/);
  }
  assert.equal(ssh.stats.allCommands.some(command => command.includes('# Lion Codex')), false);
  await connect(backend, input);
  const state = await monitors(backend, input.id);
  assert.equal(state.codex.status, 'unknown');
  assert.equal(state.codex.installed, null);
  for (const method of ['refresh-codex', 'install-codex']) {
    await assert.rejects(backend.invoke(method, input.id, 'curl arbitrary-command'), /只接受设备标识/);
  }
  assert.equal(ssh.stats.allCommands.some(command => command.includes('# Lion Codex')), false);
  await backend.invoke('disconnect', input.id);
  await assert.rejects(backend.invoke('install-codex', input.id), /请先连接设备/);
  assert.equal(ssh.stats.allCommands.some(command => command.includes('# Lion Codex')), false);
  assert.deepEqual(ssh.stats.rejectedCommands, []);
});
