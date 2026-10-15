'use strict';

const net = require('node:net');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { generateKeyPairSync, createHash } = require('node:crypto');
const { Server, utils } = require('ssh2');
const { summaryFixture } = require('./nvidia-summary.cjs');
const commandFixtures = require('../fixtures/commands.json');
const allowedStorageCommands = new Set(Object.values(commandFixtures.storage));
const allowedJobCommands = new Set(Object.values(commandFixtures.jobs).flatMap(Object.values));

// All fixtures bind to loopback. These tests never contact the user's server.
function createKeys() {
  const privateKey = generateKeyPairSync('rsa', {
    modulusLength: 2048,
    privateKeyEncoding: { type: 'pkcs1', format: 'pem' },
    publicKeyEncoding: { type: 'spki', format: 'pem' },
  }).privateKey;
  const parsed = utils.parseKey(privateKey);
  if (parsed instanceof Error) throw parsed;
  return { privateKey, parsed, fingerprint: `SHA256:${createHash('sha256').update(parsed.getPublicSSH()).digest('base64').replace(/=+$/, '')}` };
}

const hostKeys = createKeys();
const userKeys = createKeys();

function listen(server, port = 0) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => {
      server.removeListener('error', reject);
      resolve(server.address().port);
    });
  });
}

function closeServer(server) {
  // ssh2.Server wraps net.Server and exposes address()/close(), not listening.
  if (!server.address()) return Promise.resolve();
  return new Promise((resolve) => server.close(resolve));
}

async function freePort() {
  const server = net.createServer();
  const port = await listen(server);
  await closeServer(server);
  return port;
}

async function echoServer(port = 0) {
  const sockets = new Set();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on('error', () => {});
    socket.on('data', (data) => socket.write(data));
    socket.on('close', () => sockets.delete(socket));
  });
  const actualPort = await listen(server, port);
  return {
    port: actualPort,
    async close() {
      for (const socket of sockets) socket.destroy();
      await closeServer(server);
    },
  };
}

function roundTrip(port, text, timeout = 5000) {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: '127.0.0.1', port });
    const chunks = [];
    let length = 0;
    const expected = Buffer.byteLength(text);
    const timer = setTimeout(() => finish(new Error(`Loopback tunnel ${port} timed out`)), timeout);
    function finish(error, value) {
      clearTimeout(timer);
      socket.destroy();
      if (error) reject(error);
      else resolve(value);
    }
    socket.once('connect', () => socket.write(text));
    socket.once('error', (error) => finish(error));
    socket.on('data', (data) => {
      chunks.push(data);
      length += data.length;
      if (length >= expected) finish(null, Buffer.concat(chunks).toString());
    });
  });
}

async function localSshServer(options = {}) {
  const clients = new Set();
  const sockets = new Set();
  const listeners = new Map();
  const deliveryTimers = new Set();
  const terminalStreams = new Map();
  const stats = { connections: 0, authenticated: 0, authenticationMethods: [], commands: [], slurmCommands: [], jobCommands: [], storageCommands: [], allCommands: [], requests: [], rejectedCommands: [], deniedStructuredQueries: [], lifecycle: [], reverseDeliveries: [], terminalSessions: [], terminalInputs: [], terminalResizes: [] };
  let password = options.password || 'test-password';
  const keyDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'lion-ssh-test-'));
  const keyPath = path.join(keyDirectory, 'id_test_rsa');
  fs.writeFileSync(keyPath, userKeys.privateKey, { mode: 0o600 });
  const rows = options.gpuRows || '0, GPU-idle, NVIDIA Test Card, 24576, 19, 0\n1, GPU-busy, NVIDIA Test Card, 24576, 8192, 0\n';
  const computeRows = options.computeRows || 'GPU-busy, 4242, training, 8192\n';
  // Mutable responses let lifecycle tests change capabilities while keeping
  // the same SSH session. NVIDIA and Slurm replies stay independent.
  let nvidiaResponse = options.response;
  let squeueResponse = options.squeueResponse ?? { stderr: 'squeue: command not found\n', exitCode: 127 };
  let jobResponse = options.jobResponse;
  let storageResponse = options.storageResponse;

  function writeResponse(stream, response) {
    const result = typeof response === 'string' ? { stdout: response } : response;
    if (result.hang) return;
    const finish = () => {
      if (stream.destroyed) return;
      if (result.chunks) for (const chunk of result.chunks) stream.write(chunk);
      else if (result.stdout) stream.write(result.stdout);
      if (result.stderr) stream.stderr.write(result.stderr);
      stream.exit(result.exitCode ?? 0);
      stream.end();
    };
    if (result.delayMs) {
      const timer = setTimeout(() => { deliveryTimers.delete(timer); finish(); }, result.delayMs);
      deliveryTimers.add(timer);
    } else finish();
  }

  function trackSocket(socket) {
    sockets.add(socket);
    socket.on('error', () => {});
    socket.on('close', () => sockets.delete(socket));
    return socket;
  }
  function bridge(channel, socket) {
    channel.on('error', () => socket.destroy());
    channel.on('close', () => socket.destroy());
    socket.on('error', () => channel.destroy());
    socket.on('close', () => channel.destroy());
    socket.pipe(channel).pipe(socket);
  }
  async function removeListeners(client) {
    const closing = [];
    for (const [key, record] of listeners) {
      if (record.client !== client) continue;
      listeners.delete(key);
      for (const socket of record.sockets) socket.destroy();
      closing.push(closeServer(record.server));
    }
    await Promise.all(closing);
  }

  const server = new Server({ hostKeys: [hostKeys.privateKey] }, (client) => {
    stats.connections++;
    clients.add(client);
    client.on('error', () => {});
    client.on('close', () => {
      stats.lifecycle.push({ type: 'connection-close' });
      clients.delete(client);
      void removeListeners(client);
    });
    client.on('authentication', (ctx) => {
      if (ctx.username !== 'tester') return ctx.reject();
      if (ctx.method === 'password' && ctx.password === password) {
        stats.authenticationMethods.push(ctx.method);
        return ctx.accept();
      }
      if (ctx.method === 'publickey' && ctx.key.data.equals(userKeys.parsed.getPublicSSH()) &&
          (!ctx.signature || userKeys.parsed.verify(ctx.blob, ctx.signature, ctx.hashAlgo) === true)) {
        stats.authenticationMethods.push(ctx.method);
        return ctx.accept();
      }
      ctx.reject(['password', 'publickey']);
    });
    client.on('ready', () => {
      stats.authenticated++;
      client.on('session', (accept) => {
        const session = accept();
        let terminal;
        session.on('pty', (acceptPty, reject, info) => {
          if (!options.terminalEnabled || options.terminalRejectPty) return reject?.();
          terminal = { id: stats.terminalSessions.length + 1, pty: info, closed: false };
          stats.terminalSessions.push(terminal);
          stats.lifecycle.push({ type: 'terminal-pty', id: terminal.id, ...info });
          acceptPty?.();
        });
        session.on('window-change', (acceptResize, reject, info) => {
          if (!options.terminalEnabled || !terminal) return reject?.();
          stats.terminalResizes.push({ id: terminal.id, ...info });
          acceptResize?.();
        });
        session.on('shell', (acceptShell, reject) => {
          // Interactive access must be opted in by an individual terminal test.
          // Monitor/job/storage fixtures continue rejecting arbitrary shells.
          if (!options.terminalEnabled || !terminal || options.terminalRejectShell) return reject();
          const stream = acceptShell();
          terminalStreams.set(terminal.id, stream);
          stats.lifecycle.push({ type: 'terminal-shell-open', id: terminal.id });
          stream.on('error', () => {});
          stream.on('close', () => {
            terminal.closed = true;
            terminalStreams.delete(terminal.id);
            stats.lifecycle.push({ type: 'terminal-shell-close', id: terminal.id });
          });
          stream.on('data', data => {
            stats.terminalInputs.push({ id: terminal.id, data: data.toString('base64') });
            const response = typeof options.terminalOnInput === 'function' ? options.terminalOnInput(data, terminal, stats) : data;
            if (response != null) stream.write(response);
          });
          const greeting = typeof options.terminalGreeting === 'function' ? options.terminalGreeting(terminal) : options.terminalGreeting;
          stream.write(greeting ?? `\x1b[32mfixture-${terminal.id}\x1b[0m 中文 🦁\r\n$ `);
        });
        session.on('exec', (acceptExec, reject, info) => {
          stats.allCommands.push(info.command);
          if (info.command.startsWith('# Lion ')) {
            if (!allowedStorageCommands.has(info.command)) {
              stats.rejectedCommands.push(info.command);
              return reject();
            }
            stats.storageCommands.push(info.command);
            const stream = acceptExec();
            stream.on('error', () => {});
            stats.lifecycle.push({ type: 'storage-command-start', command: info.command });
            stream.once('close', () => stats.lifecycle.push({ type: 'storage-command-close', command: info.command }));
            const response = typeof storageResponse === 'function' ? storageResponse(info.command, stats) : storageResponse;
            writeResponse(stream, response ?? { stderr: 'find: command not found\n', exitCode: 127 });
            return;
          }
          if (/^(?:scontrol|scancel)(?:\s|$)/.test(info.command)) {
            if (!allowedJobCommands.has(info.command)) {
              stats.rejectedCommands.push(info.command);
              return reject();
            }
            stats.jobCommands.push(info.command);
            const stream = acceptExec();
            stream.on('error', () => {});
            stats.lifecycle.push({ type: 'job-command-start', command: info.command });
            stream.once('close', () => stats.lifecycle.push({ type: 'job-command-close', command: info.command }));
            const response = typeof jobResponse === 'function' ? jobResponse(info.command, stats) : jobResponse;
            writeResponse(stream, response ?? { stderr: 'slurm fixture: job action unavailable\n', exitCode: 1 });
            return;
          }
          if (/^squeue(?:\s|$)/.test(info.command)) {
            stats.slurmCommands.push(info.command);
            if (!/^squeue(?: -u [^\r\n]+)?$/.test(info.command) || /[;&|`\r\n]/.test(info.command)) {
              stats.rejectedCommands.push(info.command);
              return reject();
            }
            const stream = acceptExec();
            stream.on('error', () => {});
            stats.lifecycle.push({ type: 'slurm-query-start', command: info.command });
            stream.once('close', () => stats.lifecycle.push({ type: 'slurm-query-close', command: info.command }));
            const response = typeof squeueResponse === 'function' ? squeueResponse(info.command, stats) : squeueResponse;
            writeResponse(stream, response ?? { stderr: 'squeue: command not found\n', exitCode: 127 });
            return;
          }
          if (!/^nvidia-smi(?:\s|$)/.test(info.command) || /[;&|`\r\n]/.test(info.command)) {
            stats.rejectedCommands.push(info.command);
            return reject();
          }
          stats.commands.push(info.command);
          const stream = acceptExec();
          stream.on('error', () => {});
          if (nvidiaResponse !== undefined) {
            const response = typeof nvidiaResponse === 'function' ? nvidiaResponse(info.command, stats) : nvidiaResponse;
            if (response !== undefined) {
              writeResponse(stream, response);
              return;
            }
          }
          if (options.hangGpu) return;
          if (options.denyStructured && info.command !== 'nvidia-smi') {
            stats.deniedStructuredQueries.push(info.command);
            stream.stderr.write('Parameterized GPU queries are allowed only inside a Slurm allocation.\n');
            stream.exit(1);
            stream.end();
            return;
          }
          const isCompute = info.command.includes('--query-compute-apps');
          const output = typeof options.output === 'function' ? options.output(info.command, stats)
            : info.command === 'nvidia-smi' ? (options.summaryOutput || summaryFixture)
              : (isCompute ? computeRows : rows);
          stream.write(output);
          stream.exit(0);
          stream.end();
        });
      });
      if (!options.disableDirectForwarding) client.on('tcpip', (accept, reject, info) => {
        const socket = trackSocket(net.connect({ host: info.destIP, port: Number(info.destPort) }));
        const rejectConnect = () => reject();
        socket.once('connect', () => {
          socket.removeListener('error', rejectConnect);
          bridge(accept(), socket);
        });
        socket.once('error', rejectConnect);
      });
      client.on('request', (accept, reject, name, info) => {
        stats.requests.push({ name, ...info });
        stats.lifecycle.push({ type: 'request', name, ...info });
        const key = `${info.bindAddr}:${info.bindPort}`;
        if (name === 'tcpip-forward') {
          if (listeners.has(key)) return reject();
          const remoteSockets = new Set();
          const remoteServer = net.createServer((socket) => {
            const boundPort = remoteServer.address().port;
            trackSocket(socket);
            remoteSockets.add(socket);
            socket.on('close', () => remoteSockets.delete(socket));
            // A real SSH server confirms direct-tcpip as soon as this TCP
            // listener accepts. Delivery of the incoming forwarded-tcpip can
            // happen later, be refused, or be routed to a different listener.
            // These options reproduce that distinction without remote hosts.
            if (options.dropReverseDelivery) return;
            const deliver = () => {
              if (socket.destroyed) return;
              const destIP = options.reverseDestIP || info.bindAddr;
              stats.reverseDeliveries.push({ destIP, destPort: boundPort, time: Date.now() });
              client.forwardOut(destIP, boundPort,
                socket.remoteAddress || '127.0.0.1', socket.remotePort || 0, (error, channel) => {
                  if (error) return socket.destroy();
                  bridge(channel, socket);
                });
            };
            if (options.reverseDeliveryDelayMs) {
              const timer = setTimeout(() => { deliveryTimers.delete(timer); deliver(); }, options.reverseDeliveryDelayMs);
              deliveryTimers.add(timer);
            }
            else deliver();
          });
          remoteServer.once('error', () => reject());
          remoteServer.listen(info.bindPort, info.bindAddr || '127.0.0.1', () => {
            const actualPort = remoteServer.address().port;
            listeners.set(`${info.bindAddr}:${actualPort}`, { server: remoteServer, client, sockets: remoteSockets });
            accept(info.bindPort === 0 ? actualPort : undefined);
          });
          return;
        }
        if (name === 'cancel-tcpip-forward') {
          const record = listeners.get(key);
          if (!record || record.client !== client) return reject();
          listeners.delete(key);
          // OpenSSH releases the listening socket before acknowledging cancellation;
          // existing channels are closed separately by the client's stop operation.
          record.server.close(() => {});
          const acknowledge = () => {
            stats.lifecycle.push({ type: 'cancel-acknowledged', ...info });
            accept();
          };
          if (options.cancelDelayMs) setTimeout(acknowledge, options.cancelDelayMs);
          else acknowledge();
          return;
        }
        if (name.startsWith('keepalive')) return accept?.();
        reject?.();
      });
    });
  });
  const port = await listen(server);
  return {
    port, keyPath, fingerprint: hostKeys.fingerprint, stats, listeners, terminalStreams,
    writeTerminal(id, value) {
      const stream = terminalStreams.get(id);
      if (!stream || stream.destroyed) throw new Error(`No live fixture terminal ${id}`);
      return new Promise((resolve, reject) => stream.write(value, error => error ? reject(error) : resolve()));
    },
    exitTerminal(id, code = 0) {
      const stream = terminalStreams.get(id);
      if (!stream || stream.destroyed) throw new Error(`No live fixture terminal ${id}`);
      stream.exit(code);
      stream.end();
    },
    setNvidiaResponse(value) { nvidiaResponse = value; },
    setSqueueResponse(value) { squeueResponse = value; },
    setJobResponse(value) { jobResponse = value; },
    setStorageResponse(value) { storageResponse = value; },
    setPassword(value) { password = value; },
    disconnectClients() { for (const client of clients) client.end(); },
    async close() {
      for (const timer of deliveryTimers) clearTimeout(timer);
      deliveryTimers.clear();
      for (const socket of sockets) socket.destroy();
      for (const client of clients) { client.end(); client._sock?.destroy(); }
      for (const { server: remoteServer } of listeners.values()) if (remoteServer.listening) remoteServer.close();
      listeners.clear();
      await closeServer(server);
      fs.unlinkSync(keyPath);
      fs.rmdirSync(keyDirectory);
    },
  };
}

module.exports = { localSshServer, echoServer, freePort, roundTrip, listen, closeServer };
