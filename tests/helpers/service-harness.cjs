'use strict';

const { spawn } = require('node:child_process');
const path = require('node:path');
const readline = require('node:readline');
const { delay } = require('./fixtures.cjs');

// Exercise the same native Service used by Tauri, without a GUI or global
// single-instance registration. Every child owns isolated fixture data only.
class ServiceHarness {
  constructor(dataDir, options = {}) {
    if (!path.isAbsolute(dataDir)) throw new Error('Test data directory must be absolute');
    this.messages = [];
    this.states = new Map();
    this.pending = new Map();
    this.nextId = 0;
    this.stderr = '';
    this.exited = false;
    const binary = options.binary || process.env.LION_TEST_BINARY || path.resolve(__dirname, '../../src-tauri/target/debug/lion-ssh.exe');
    this.child = spawn(binary, ['--backend-test'], {
      env: { ...process.env, ...options.env, LION_DATA_DIR: dataDir, LION_THEME_RESOLVED: 'dark' },
      stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
    });
    this.child.stdin.on('error', () => {});
    this.child.stderr.on('data', data => { this.stderr = (this.stderr + data.toString()).slice(-16000); });
    const rejectPending = error => {
      for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(error); }
      this.pending.clear();
    };
    this.child.on('error', error => { this.stderr += error.message; this.exited = true; rejectPending(error); });
    this.closed = new Promise(resolve => this.child.once('close', code => {
      this.exited = true;
      rejectPending(new Error(`Native backend exited (${code}): ${this.stderr}`));
      resolve(code);
    }));
    readline.createInterface({ input: this.child.stdout }).on('line', line => {
      let message;
      try { message = JSON.parse(line); }
      catch { this.stderr += `Invalid backend output: ${line}\n`; return; }
      this.messages.push(message);
      if (message.event === 'lion:state') this.states.set(message.payload.profileId, message.payload);
      if (message.id != null) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) pending.reject(new Error(message.error));
        else pending.resolve(message.result);
      }
    });
  }
  write(message) {
    if (this.exited || this.child.stdin.destroyed) throw new Error(`Native backend is closed: ${this.stderr}`);
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }
  invoke(method, ...args) {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Native request timed out: ${method}; ${this.stderr}`)); }, 25_000);
      this.pending.set(id, { resolve, reject, timer });
      try { this.write({ id, method, args }); }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
  }
  async waitFor(predicate, description = 'native backend event', timeout = 12_000) {
    const until = Date.now() + timeout;
    while (Date.now() < until) {
      const result = predicate(this.messages);
      if (result) return result;
      if (this.exited) throw new Error(`Native backend exited waiting for ${description}: ${this.stderr}`);
      await delay(25);
    }
    throw new Error(`Timeout waiting for ${description}; ${this.stderr}; last states ${JSON.stringify([...this.states.values()])}`);
  }
  state(id, predicate, description = 'native server state', timeout) {
    return this.waitFor(() => { const state = this.states.get(id); return state && predicate(state) && state; }, description, timeout);
  }
  async trust(id, accepted = true, afterIndex = 0) {
    const request = await this.waitFor(messages => messages.slice(afterIndex).find(message => message.event === 'lion:host-key' && message.payload.profileId === id)?.payload, 'host key prompt');
    await this.invoke('trust-host', id, accepted, request.fingerprint);
    return request;
  }
  async stop() {
    if (this.exited) return this.closed;
    const killTimer = setTimeout(() => this.child.kill(), 8000);
    try {
      await this.invoke('shutdown').catch(() => {});
      this.child.stdin.end();
      return await this.closed;
    } finally { clearTimeout(killTimer); }
  }
}
module.exports = { ServiceHarness };
