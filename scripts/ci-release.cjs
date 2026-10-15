'use strict';

// Release-time only. The application consumes the standard Tauri latest.json;
// its payload signatures are always produced by the official Tauri signer.
const fs = require('node:fs/promises');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const VERSION = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/;
const NOTES = 'Windows：安装版 EXE，或解压多文件便携 ZIP 后运行 Lion SSH.exe。macOS：按 Apple Silicon / Intel 选择 DMG。Linux：AppImage。升级前请备份 data 目录。';

function releaseArtifacts(version) {
  if (!VERSION.test(version)) throw new Error('Invalid release version');
  const prefix = `Lion-SSH-${version}`;
  return [
    { key: 'windows-x86_64', file: `${prefix}-windows-x64-setup.exe`, alias: 'windows-x86_64-nsis' },
    { key: 'windows-x86_64-portable', file: `${prefix}-windows-x64-portable.zip` },
    { key: 'darwin-aarch64', file: `${prefix}-macos-aarch64.app.tar.gz`, alias: 'darwin-aarch64-app' },
    { key: 'darwin-x86_64', file: `${prefix}-macos-x86_64.app.tar.gz`, alias: 'darwin-x86_64-app' },
    { key: 'linux-x86_64', file: `${prefix}-linux-x86_64.AppImage`, alias: 'linux-x86_64-appimage' },
  ];
}
function requiredAssets(version) {
  return [
    ...releaseArtifacts(version).flatMap(artifact => [artifact.file, `${artifact.file}.sig`]),
    `Lion-SSH-${version}-macos-aarch64.dmg`,
    `Lion-SSH-${version}-macos-x86_64.dmg`,
  ];
}
function signatureVersion(signature, version) {
  const encoded = String(signature).trim();
  if (!encoded || encoded.length > 16384 || !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) throw new Error('Invalid updater signature encoding');
  const decoded = Buffer.from(encoded, 'base64').toString('utf8');
  const comments = decoded.split(/\r?\n/).filter(line => line.startsWith('trusted comment: '));
  if (comments.length !== 1) throw new Error('Updater signature has no unambiguous trusted comment');
  const fields = comments[0].slice('trusted comment: '.length).split('\t').filter(field => field.startsWith('version:'));
  if (fields.length !== 1 || fields[0] !== `version:${version}`) throw new Error('Updater signature is not bound to this release version');
  return encoded;
}
async function validate(project = root, tag = process.env.RELEASE_TAG, requireSigning = false) {
  const pkg = JSON.parse(await fs.readFile(path.join(project, 'package.json'), 'utf8'));
  const config = JSON.parse(await fs.readFile(path.join(project, 'src-tauri', 'tauri.conf.json'), 'utf8'));
  const cargo = await fs.readFile(path.join(project, 'src-tauri', 'Cargo.toml'), 'utf8');
  const packageSection = /^\[package\]\s*\r?\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargo)?.[1] || '';
  const cargoVersion = /^version\s*=\s*"([^"]+)"\s*$/m.exec(packageSection)?.[1];
  if (!VERSION.test(pkg.version) || config.version !== pkg.version || cargoVersion !== pkg.version) throw new Error('package.json, Cargo.toml and tauri.conf.json versions must match');
  if (tag !== `v${pkg.version}`) throw new Error(`Release tag must be v${pkg.version}`);
  if (config.bundle?.createUpdaterArtifacts !== true) throw new Error('Release builds require bundle.createUpdaterArtifacts=true');
  const updater = config.plugins?.updater;
  if (!updater?.pubkey || /PLACEHOLDER|REPLACE|YOUR_|CONTENT FROM/i.test(updater.pubkey)) throw new Error('Configure the real updater public key before publishing');
  if (updater.requireSignedVersion !== true) throw new Error('Release builds require updater.requireSignedVersion=true');
  if (requireSigning && !process.env.TAURI_SIGNING_PRIVATE_KEY) throw new Error('Configure the TAURI_SIGNING_PRIVATE_KEY repository Actions secret');
  return { tag, version: pkg.version, prerelease: pkg.version.includes('-') };
}

async function prepareRelease({ github, context, core }) {
  const info = await validate();
  let release;
  try {
    release = (await github.rest.repos.getReleaseByTag({ ...context.repo, tag: info.tag })).data;
    if (!release.draft) throw new Error('This tag already has a published release; create a new version instead');
  } catch (error) {
    if (error.status !== 404) throw error;
    release = (await github.rest.repos.createRelease({ ...context.repo, tag_name: info.tag, target_commitish: context.sha,
      name: `Lion SSH ${info.tag}`, body: NOTES, draft: true, prerelease: info.prerelease })).data;
  }
  core.setOutput('release_id', String(release.id));
  core.setOutput('tag', info.tag);
  core.setOutput('version', info.version);
  return release.id;
}

async function makeUpdaterJson({ version, tag, repository, assets, readSignature, notes = NOTES, publishedAt = new Date().toISOString(), serverUrl = 'https://github.com' }) {
  if (!VERSION.test(version) || tag !== `v${version}`) throw new Error('Release version/tag mismatch');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error('Invalid GitHub repository');
  const server = new URL(serverUrl);
  if (server.protocol !== 'https:') throw new Error('Updater downloads require HTTPS');
  if (!Number.isFinite(Date.parse(publishedAt))) throw new Error('Invalid publication timestamp');
  const indexed = new Map();
  for (const asset of assets) {
    if (indexed.has(asset.name)) throw new Error(`Duplicate release asset: ${asset.name}`);
    indexed.set(asset.name, asset);
  }
  for (const name of requiredAssets(version)) {
    const asset = indexed.get(name);
    if (!asset || !Number.isSafeInteger(asset.size) || asset.size <= 0) throw new Error(`Missing or empty release asset: ${name}`);
  }
  const platforms = {};
  for (const artifact of releaseArtifacts(version)) {
    const sigAsset = indexed.get(`${artifact.file}.sig`);
    if (sigAsset.size > 16384) throw new Error(`Updater signature asset is too large: ${sigAsset.name}`);
    const signature = signatureVersion(await readSignature(sigAsset), version);
    const entry = { signature, url: `${server.origin}/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(artifact.file)}` };
    platforms[artifact.key] = entry;
    if (artifact.alias) platforms[artifact.alias] = entry;
  }
  return { version, notes, pub_date: new Date(publishedAt).toISOString(), platforms };
}

async function finalizeRelease({ github, context, core }) {
  const { version, tag } = await validate();
  const releaseId = Number(process.env.RELEASE_ID);
  if (!Number.isSafeInteger(releaseId) || releaseId <= 0) throw new Error('Invalid release ID');
  const release = (await github.rest.repos.getRelease({ ...context.repo, release_id: releaseId })).data;
  if (!release.draft || release.tag_name !== tag) throw new Error('Only this version draft release may be finalized');
  const assets = await github.paginate(github.rest.repos.listReleaseAssets, { ...context.repo, release_id: releaseId, per_page: 100 });
  const metadata = await makeUpdaterJson({ version, tag, repository: `${context.repo.owner}/${context.repo.repo}`, assets,
    notes: release.body || NOTES, serverUrl: context.serverUrl || 'https://github.com', readSignature: async asset => {
      const response = await github.rest.repos.getReleaseAsset({ ...context.repo, asset_id: asset.id, headers: { accept: 'application/octet-stream' } });
      return typeof response.data === 'string' ? response.data : Buffer.from(response.data).toString('utf8');
    } });
  const bytes = Buffer.from(JSON.stringify(metadata, null, 2) + '\n');
  const existing = assets.find(asset => asset.name === 'latest.json');
  if (existing) await github.rest.repos.deleteReleaseAsset({ ...context.repo, asset_id: existing.id });
  await github.rest.repos.uploadReleaseAsset({ ...context.repo, release_id: releaseId, url: release.upload_url,
    name: 'latest.json', data: bytes, headers: { 'content-type': 'application/json', 'content-length': bytes.length } });
  await github.rest.repos.updateRelease({ ...context.repo, release_id: releaseId, draft: false, make_latest: version.includes('-') ? 'false' : 'true' });
  core.info(`Published ${tag} with ${Object.keys(metadata.platforms).length} signed updater targets`);
  return metadata;
}

async function uploadWindowsPortable({ github, context, core }) {
  const info = await validate();
  const releaseId = Number(process.env.RELEASE_ID);
  if (!Number.isSafeInteger(releaseId) || releaseId <= 0) throw new Error('Invalid release ID');
  const release = (await github.rest.repos.getRelease({ ...context.repo, release_id: releaseId })).data;
  if (!release.draft || release.tag_name !== info.tag) throw new Error('Portable assets must upload to this draft release');
  const assets = await github.paginate(github.rest.repos.listReleaseAssets, { ...context.repo, release_id: releaseId, per_page: 100 });
  const artifact = releaseArtifacts(info.version).find(artifact => artifact.key === 'windows-x86_64-portable');
  for (const name of [artifact.file, `${artifact.file}.sig`]) {
    const bytes = await fs.readFile(path.join(root, 'release', name));
    if (name.endsWith('.sig')) signatureVersion(bytes.toString('utf8'), info.version);
    const existing = assets.find(asset => asset.name === name);
    if (existing) await github.rest.repos.deleteReleaseAsset({ ...context.repo, asset_id: existing.id });
    await github.rest.repos.uploadReleaseAsset({ ...context.repo, release_id: releaseId, url: release.upload_url, name,
      data: bytes, headers: { 'content-type': 'application/octet-stream', 'content-length': bytes.length } });
    core.info(`Uploaded ${name}`);
  }
}

if (require.main === module) {
  if (process.argv[2] !== 'validate' || process.argv.length !== 3) throw new Error('Usage: node scripts/ci-release.cjs validate');
  validate(root, process.env.RELEASE_TAG, true).then(async info => {
    if (process.env.GITHUB_OUTPUT) await fs.appendFile(process.env.GITHUB_OUTPUT, `version=${info.version}\ntag=${info.tag}\n`);
    console.log(`Validated release ${info.tag}; signing key is configured`);
  }).catch(error => { console.error(error.message); process.exitCode = 1; });
}
module.exports = { releaseArtifacts, requiredAssets, signatureVersion, validate, prepareRelease, makeUpdaterJson, finalizeRelease, uploadWindowsPortable };
