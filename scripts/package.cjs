const fs = require('node:fs/promises');
const path = require('node:path');
const { createHash } = require('node:crypto');
const { zipSync } = require('fflate');
const { runTauri } = require('./tauri.cjs');

const root = path.resolve(__dirname, '..');
function parseArguments(args) {
  const result = { portableOnly: false, skipBuild: false, signPortable: false, target: process.env.CARGO_BUILD_TARGET || '' };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === '--portable-only') result.portableOnly = true;
    else if (argument === '--skip-build') result.skipBuild = true;
    else if (argument === '--sign-portable') result.signPortable = true;
    else if (argument === '--target') {
      const target = args[++index];
      if (!target || !/^(?:x86_64|aarch64|i686)-pc-windows-(?:msvc|gnu)$/.test(target)) throw new Error('Windows target 格式不正确');
      result.target = target;
    } else throw new Error(`不支持的打包参数：${argument}`);
  }
  return result;
}
function windowsArchitecture(target = '', architecture = process.arch) {
  if (!target) {
    if (!['x64', 'arm64', 'ia32'].includes(architecture)) throw new Error('不支持的 Windows 架构');
    return architecture;
  }
  const architectures = { x86_64: 'x64', aarch64: 'arm64', i686: 'ia32' };
  if (!/^(?:x86_64|aarch64|i686)-pc-windows-(?:msvc|gnu)$/.test(target)) throw new Error('Windows target 格式不正确');
  return architectures[target.split('-')[0]];
}
function buildDirectory(project, target = '') {
  const base = process.env.CARGO_TARGET_DIR ? path.resolve(project, process.env.CARGO_TARGET_DIR) : path.join(project, 'src-tauri', 'target');
  return path.join(base, ...(target ? [target] : []), 'release');
}
async function collect(directory, base = directory, result = {}) {
  for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
    if (entry.name === 'data') continue;
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) await collect(file, base, result);
    else result[path.relative(base, file).replaceAll('\\', '/')] = new Uint8Array(await fs.readFile(file));
  }
  return result;
}
async function distributionFiles(portable, resources) {
  const resourceFiles = await collect(resources);
  const files = Object.fromEntries(Object.entries(resourceFiles).map(([name, value]) => [`licenses/${name}`, value]));
  for (const name of ['Lion SSH.exe', 'LICENSE.txt', 'portable.json', '使用说明.txt']) files[name] = new Uint8Array(await fs.readFile(path.join(portable, name)));
  return files;
}
async function removeManifestFiles(portable, manifest) {
  const base = await fs.realpath(portable);
  const parents = new Set();
  for (const item of manifest?.files || []) {
    if (typeof item.path !== 'string') throw new Error('便携版清单路径无效');
    const file = path.resolve(portable, item.path);
    const relative = path.relative(portable, file);
    if (!relative || relative.startsWith('..' + path.sep) || relative === '..' || path.isAbsolute(relative) || /^data(?:[\\/]|$)/i.test(relative)) throw new Error('便携版清单包含不安全的路径');
    let actual;
    try { actual = await fs.realpath(file); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (actual) {
      const resolved = path.relative(base, actual);
      if (!resolved || resolved.startsWith('..' + path.sep) || resolved === '..' || path.isAbsolute(resolved) || /^data(?:[\\/]|$)/i.test(resolved)) throw new Error('便携版清单路径指向分发目录外或用户数据');
      await fs.rm(file, { force: true });
    }
    for (let directory = path.dirname(file); directory !== portable; directory = path.dirname(directory)) parents.add(directory);
  }
  // Only prune empty directories belonging to the previous manifest.
  for (const directory of [...parents].sort((a, b) => b.length - a.length)) {
    try { await fs.rmdir(directory); }
    catch (error) { if (!['ENOENT', 'ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error; }
  }
}
async function packageWindows(args = process.argv.slice(2)) {
  if (process.platform !== 'win32') throw new Error('dist:win 必须在 Windows 上执行');
  const { portableOnly, skipBuild, signPortable, target } = parseArguments(args);
  if (signPortable && !process.env.TAURI_SIGNING_PRIVATE_KEY && !process.env.TAURI_SIGNING_PRIVATE_KEY_PATH) throw new Error('签名便携更新包需要 TAURI_SIGNING_PRIVATE_KEY 或 TAURI_SIGNING_PRIVATE_KEY_PATH');
  const pkg = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
  if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(pkg.version)) throw new Error('版本号格式不正确');
  const config = JSON.parse(await fs.readFile(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
  if (config.version !== pkg.version) throw new Error('package.json 与 tauri.conf.json 的版本号必须一致');
  if (!skipBuild) await runTauri(['build', ...(portableOnly ? ['--no-bundle'] : ['--bundles', 'nsis']), ...(target ? ['--target', target] : [])]);
  const build = buildDirectory(root, target);
  const architecture = windowsArchitecture(target);
  const release = path.join(root, 'release');
  const stem = `Lion-SSH-${pkg.version}-windows-${architecture}`;
  const portable = path.join(release, `${stem}-portable`);
  await fs.mkdir(portable, { recursive: true });
  // Rebuild only manifest-owned program files. User data and added files survive.
  const manifestPath = path.join(portable, 'manifest.json');
  let oldManifest;
  try { oldManifest = JSON.parse(await fs.readFile(manifestPath, 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  await removeManifestFiles(portable, oldManifest);
  await fs.copyFile(path.join(build, 'lion-ssh.exe'), path.join(portable, 'Lion SSH.exe'));
  await fs.cp(path.join(root, 'src-tauri', 'resources', 'licenses'), path.join(portable, 'licenses'), { recursive: true });
  await fs.copyFile(path.join(root, 'LICENSE'), path.join(portable, 'LICENSE.txt'));
  await fs.writeFile(path.join(portable, 'portable.json'), JSON.stringify({ portable: true }, null, 2));
  await fs.writeFile(path.join(portable, '使用说明.txt'), 'Lion SSH ' + pkg.version + '\r\n\r\n运行 Lion SSH.exe，licenses 文件夹为第三方开源声明。SSH、隧道和监测均由 Rust 主程序执行，无需安装 Node.js。\r\n首次启动会在程序旁建立 data 文件夹。升级前备份 data，升级时保留该目录及其中的原有 JSON 文件。安装版也读取程序旁的 data。\r\nWindows 需要 Microsoft Edge WebView2 Runtime；便携包不附带浏览器运行时。\r\n现有 Tauri 版 native:v1 加密凭据可在同一 Windows 账户下继续使用，早期旧加密凭据需重新输入。\r\n');
  const files = await distributionFiles(portable, path.join(root, 'src-tauri', 'resources', 'licenses'));
  const manifest = { version: pkg.version, arch: architecture, files: Object.entries(files).map(([name, value]) => ({ path: name, size: value.length, sha256: createHash('sha256').update(value).digest('hex') })) };
  files['manifest.json'] = new TextEncoder().encode(JSON.stringify(manifest, null, 2));
  await fs.writeFile(manifestPath, files['manifest.json']);
  // Keep private data out of archives, even when rebuilding an already-used folder.
  const archive = Object.fromEntries(Object.entries(files).map(([name, value]) => [`${stem}-portable/${name}`, value]));
  const portableArchive = path.join(release, `${stem}-portable.zip`);
  await fs.writeFile(portableArchive, zipSync(archive, { level: 6 }));
  if (signPortable) await runTauri(['signer', 'sign', '--app-version', pkg.version, portableArchive]);
  if (!portableOnly) {
    const nsis = path.join(build, 'bundle', 'nsis');
    const candidates = (await fs.readdir(nsis)).filter(name => name.endsWith('-setup.exe') && name.includes(pkg.version));
    if (candidates.length !== 1) throw new Error(`无法唯一定位本次 NSIS 安装包：${candidates.join(', ')}`);
    await fs.copyFile(path.join(nsis, candidates[0]), path.join(release, `${stem}-setup.exe`));
    const signature = path.join(nsis, `${candidates[0]}.sig`);
    try { await fs.copyFile(signature, path.join(release, `${stem}-setup.exe.sig`)); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  console.log(`已生成多文件便携版：${portable}`);
  console.log(`已生成便携 ZIP：${path.join(release, `${stem}-portable.zip`)}`);
  if (!portableOnly) console.log(`已生成安装版：${path.join(release, `${stem}-setup.exe`)}`);
}
if (require.main === module) packageWindows().catch(error => { console.error(error); process.exitCode = 1; });
module.exports = { collect, distributionFiles, removeManifestFiles, parseArguments, windowsArchitecture, buildDirectory, packageWindows };
