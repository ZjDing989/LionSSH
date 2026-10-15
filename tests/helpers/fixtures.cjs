'use strict';

// Development fixtures; production networking and scheduling live in Rust.
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
function profile(server, extra = {}) {
  return {
    id: 'test-device', name: 'Local fixture', host: '127.0.0.1', port: server.port,
    username: 'tester', authType: 'password', keyPath: '', autoReconnect: true,
    gpu: { enabled: false, intervalSec: 2, idleUtil: 5, idleMemoryMiB: 512, stableSamples: 2 },
    tunnels: [], ...extra,
  };
}
function tunnel(id, bindPort, targetPort, direction = 'local', extra = {}) {
  return {
    id, name: id, direction, bindHost: '127.0.0.1', bindPort,
    targetHost: '127.0.0.1', targetPort, enabled: true, healthIntervalSec: 5, ...extra,
  };
}
module.exports = { delay, profile, tunnel };
