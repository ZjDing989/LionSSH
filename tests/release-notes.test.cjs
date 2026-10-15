'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const { releaseNotes, prepareRelease, finalizeRelease, requiredAssets } = require('../scripts/ci-release.cjs');

const root = path.resolve(__dirname, '..');
const expectedNotes = "1. 新增'codex更新'工具 一键更新重启云端codex\n2. 新增'命令行'工具 在软件内也可以使用简单的命令行操作";
const context = { repo: { owner: 'ZjDing989', repo: 'LionSSH' }, sha: 'fixture-commit', serverUrl: 'https://github.com' };
const core = { setOutput() {}, info() {} };

test('v1.8.2 release notes contain only the two requested changes', async () => {
  assert.equal(await releaseNotes('1.8.2'), expectedNotes);
  assert.notEqual(await releaseNotes('999.0.0'), expectedNotes, 'Future versions must not reuse v1.8.2 changes');
  await assert.rejects(releaseNotes('../private'), /Invalid release version/);
});

async function withReleaseEnvironment(action) {
  const { version } = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
  const previous = { RELEASE_TAG: process.env.RELEASE_TAG, RELEASE_ID: process.env.RELEASE_ID };
  process.env.RELEASE_TAG = `v${version}`;
  process.env.RELEASE_ID = '42';
  try { await action(version, await releaseNotes(version)); }
  finally {
    for (const [name, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

test('release preparation refreshes stale draft notes without generating extra text', async () => {
  await withReleaseEnvironment(async (version, notes) => {
    let updated;
    const draft = { id: 42, draft: true, tag_name: `v${version}`, body: 'Old generated notes' };
    const github = { rest: { repos: {
      getReleaseByTag: async () => ({ data: draft }),
      updateRelease: async input => { updated = input; return { data: { ...draft, body: input.body } }; },
    } } };
    assert.equal(await prepareRelease({ github, context, core }), 42);
    assert.equal(updated.body, notes);
    assert.equal(updated.release_id, 42);
    assert.equal(updated.draft, undefined, 'Preparation must leave the release draft');
  });
});

test('release creation starts as a draft with the version-specific notes', async () => {
  await withReleaseEnvironment(async (version, notes) => {
    let created;
    const github = { rest: { repos: {
      getReleaseByTag: async () => { const error = new Error('Not found'); error.status = 404; throw error; },
      createRelease: async input => { created = input; return { data: { id: 42, ...input } }; },
    } } };
    assert.equal(await prepareRelease({ github, context, core }), 42);
    assert.equal(created.body, notes);
    assert.equal(created.draft, true);
    assert.equal(created.tag_name, `v${version}`);
  });
});

test('final publishing and latest.json share exact version notes despite a stale draft body', async () => {
  await withReleaseEnvironment(async (version, notes) => {
    const assets = requiredAssets(version).map((name, index) => ({ id: index + 1, name, size: 200 }));
    const signature = Buffer.from(`untrusted comment: fixture\nFAKE\ntrusted comment: timestamp:123\tfile:fixture\tversion:${version}\nGLOBAL\n`).toString('base64');
    let published;
    let uploaded;
    const github = { paginate: async () => assets, rest: { repos: {
      getRelease: async () => ({ data: { id: 42, draft: true, tag_name: `v${version}`, body: 'Unexpected generated notes', upload_url: 'https://uploads.github.com/fixture' } }),
      listReleaseAssets() {},
      getReleaseAsset: async () => ({ data: signature }),
      uploadReleaseAsset: async input => { uploaded = input; },
      updateRelease: async input => { published = input; },
    } } };
    const metadata = await finalizeRelease({ github, context, core });
    assert.equal(metadata.notes, notes);
    assert.equal(JSON.parse(uploaded.data.toString('utf8')).notes, notes);
    assert.equal(uploaded.name, 'latest.json');
    assert.equal(published.body, notes);
    assert.equal(published.draft, false);
    assert.equal(published.make_latest, 'true');
  });
});
