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

test('release preparation finds a draft beyond the first page even though the tag endpoint returns 404', async () => {
  await withReleaseEnvironment(async (version, notes) => {
    let updated;
    const pagesRead = [];
    const draft = { id: 42, draft: true, tag_name: `v${version}`, body: 'Old generated notes' };
    const github = { paginate: async (endpoint, input) => {
      assert.equal(endpoint, github.rest.repos.listReleases);
      assert.equal(input.per_page, 100);
      const releases = [];
      for (let page = 1; ; page++) {
        const { data } = await endpoint({ ...input, page });
        releases.push(...data);
        if (data.length < input.per_page) return releases;
      }
    }, rest: { repos: {
      getReleaseByTag: async () => { const error = new Error('Draft releases are not returned by tag'); error.status = 404; throw error; },
      listReleases: async input => {
        pagesRead.push(input.page);
        return { data: input.page === 1
          ? Array.from({ length: 100 }, (_, id) => ({ id: 1000 + id, draft: false, tag_name: `v0.0.${id}` }))
          : [draft] };
      },
      createRelease: async () => assert.fail('An existing draft must not be duplicated'),
      updateRelease: async input => { updated = input; return { data: { ...draft, body: input.body } }; },
    } } };
    assert.equal(await prepareRelease({ github, context, core }), 42);
    assert.deepEqual(pagesRead, [1, 2]);
    assert.equal(updated.body, notes);
    assert.equal(updated.release_id, 42);
    assert.equal(updated.draft, undefined, 'Preparation must leave the release draft');
  });
});

test('release creation starts as a draft with the version-specific notes', async () => {
  await withReleaseEnvironment(async (version, notes) => {
    let created;
    const github = { paginate: async (endpoint, input) => {
      assert.equal(endpoint, github.rest.repos.listReleases);
      assert.equal(input.per_page, 100);
      return [{ id: 7, draft: true, tag_name: 'v0.0.1' }];
    }, rest: { repos: {
      listReleases() {},
      createRelease: async input => { created = input; return { data: { id: 42, ...input } }; },
    } } };
    assert.equal(await prepareRelease({ github, context, core }), 42);
    assert.equal(created.body, notes);
    assert.equal(created.draft, true);
    assert.equal(created.tag_name, `v${version}`);
  });
});

test('release preparation refuses a published release and multiple matching drafts', async () => {
  await withReleaseEnvironment(async (version) => {
    const draft = { id: 42, draft: true, tag_name: `v${version}` };
    const cases = [
      { releases: [{ ...draft, draft: false }], error: /published release/ },
      { releases: [draft, { ...draft, id: 43 }], error: /Multiple draft releases.*42, 43/ },
      { releases: [draft, { ...draft, id: 43, draft: false }], error: /published release/ },
    ];
    for (const { releases, error } of cases) {
      const github = { paginate: async () => releases, rest: { repos: {
        listReleases() {},
        createRelease: async () => assert.fail('Ambiguous or published versions must not create a release'),
        updateRelease: async () => assert.fail('Ambiguous or published versions must not be updated'),
      } } };
      await assert.rejects(prepareRelease({ github, context, core }), error);
    }
  });
});

test('release listing failures propagate without creating another draft', async () => {
  await withReleaseEnvironment(async () => {
    const failure = Object.assign(new Error('Cannot list releases'), { status: 404 });
    const github = { paginate: async () => { throw failure; }, rest: { repos: {
      listReleases() {},
      createRelease: async () => assert.fail('A failed lookup must not be treated as an absent draft'),
    } } };
    await assert.rejects(prepareRelease({ github, context, core }), failure);
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
