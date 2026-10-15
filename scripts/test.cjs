'use strict';

const { spawn } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs/promises');
const { buildEnvironment } = require('./tauri.cjs');
const root = path.resolve(__dirname, '..');

function run(command, args, env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env, stdio: 'inherit', windowsHide: true });
    child.once('error', reject);
    child.once('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`${command} 失败（${code ?? signal}）`)));
  });
}

async function main() {
  const env = buildEnvironment();
  const manifest = ['--manifest-path', 'src-tauri/Cargo.toml'];
  await run('cargo', ['test', ...manifest], env);
  await run('cargo', ['build', ...manifest], env);
  const base = process.env.CARGO_TARGET_DIR ? path.resolve(root, process.env.CARGO_TARGET_DIR) : path.join(root, 'src-tauri', 'target');
  const target = process.env.CARGO_BUILD_TARGET;
  env.LION_TEST_BINARY = path.join(base, ...(target ? [target] : []), 'debug', process.platform === 'win32' ? 'lion-ssh.exe' : 'lion-ssh');
  const tests = (await fs.readdir(path.join(root, 'tests'))).filter(name => name.endsWith('.test.cjs')).sort().map(name => path.join('tests', name));
  await run(process.execPath, ['--test', ...tests], env);
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
