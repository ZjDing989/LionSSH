const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
function buildEnvironment() {
  const env = { ...process.env };
  const cargoBin = path.join(env.CARGO_HOME || path.join(os.homedir(), '.cargo'), 'bin');
  const pathKeys = Object.keys(env).filter(key => key.toLowerCase() === 'path');
  const originalPath = pathKeys.map(key => env[key]).find(Boolean) || '';
  for (const key of pathKeys) delete env[key];
  env.PATH = [path.dirname(process.execPath), cargoBin, originalPath].join(path.delimiter);
  return env;
}
function runTauri(args) {
  const cli = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  if (!fs.existsSync(cli)) throw new Error('请先运行 npm ci 安装 Tauri 开发依赖');
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cli, ...args], { cwd: root, env: buildEnvironment(), stdio: 'inherit', windowsHide: true });
    child.on('error', reject);
    child.on('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`Tauri ${args[0] || ''} 失败（${code ?? signal}）`)));
  });
}
if (require.main === module) runTauri(process.argv.slice(2)).catch(error => { console.error(error.message); process.exitCode = 1; });
module.exports = { runTauri, buildEnvironment };
