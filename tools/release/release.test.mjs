import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  GitHubApi, ReleaseError, decodeBody, encodeBody, hashBytes,
  parseStableTag, publishRelease, readTagAnnotation, validateBuildArtifacts,
  validateCommitIdentity, validateProjectVersions, validateTagRef, verifyAssets,
} from './release-lib.mjs';

const sha = 'a'.repeat(40);
const version = '1.2.3';
const tag = `v${version}`;
const body = 'FluxReader v1.2.3\n\n修复：测试 $(不执行) `反引号` 和换行。\n';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'fluxreader-release-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src-tauri'));
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version }));
  writeFileSync(join(root, 'package-lock.json'), JSON.stringify({ version, packages: { '': { version } } }));
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), JSON.stringify({ version }));
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), `[package]\nname = "app"\nversion = "${version}"\n`);
  writeFileSync(join(root, 'src-tauri/Cargo.lock'), `version = 3\n\n[[package]]\nname = "app"\nversion = "${version}"\n`);
  const paths = [join(root, `FluxReader_${version}_x64-setup.exe`), join(root, `FluxReader_${version}_x64_en-US.msi`)];
  paths.forEach((path, i) => writeFileSync(path, `installer-${i}`));
  return { root, paths };
}

function artifacts(f) {
  return validateBuildArtifacts({ artifactPaths: f.paths, appVersion: version, expectedVersion: version });
}

function tagClient(overrides = {}) {
  return {
    getTagRef: async () => ({ object: { type: 'tag', sha: 'b'.repeat(40) } }),
    getTagObject: async () => ({ object: { type: 'commit', sha }, message: body }),
    compare: async () => ({ status: 'behind' }),
    ...overrides,
  };
}

function releaseClient({ draft, published, digest = true, uploadError, assetMutation } = {}) {
  const calls = [];
  const writes = [];
  let state = draft ? { ...draft } : null;
  let publicRelease = published || null;
  let assets = draft?.assets ? structuredClone(draft.assets) : [];
  let nextAssetId = 50;
  const data = new Map();
  return {
    calls,
    writes,
    get state() { return state; },
    get assets() { return assets; },
    getPublishedRelease: async () => { calls.push('get-published'); return publicRelease; },
    listReleases: async () => { calls.push('list-releases'); return state ? [{ ...state }] : []; },
    getRelease: async (_repo, id) => { calls.push(`get-release-${id}`); return state ? { ...state } : null; },
    createRelease: async (_repo, payload) => {
      calls.push('create-draft');
      state = { ...payload, id: 1, upload_url: 'https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}' };
      return { ...state };
    },
    updateRelease: async (_repo, _id, payload) => {
      calls.push(payload.draft === false ? 'publish' : 'update-draft');
      writes.push({ operation: 'update', publicBeforeWrite: state?.draft !== true, payload });
      state = { ...state, ...payload };
      if (payload.draft === false) publicRelease = { ...state };
      return { ...state };
    },
    listAssets: async () => { calls.push('list-assets'); return assets.map((asset) => ({ ...asset })); },
    deleteAsset: async (_repo, id) => {
      calls.push(`delete-${id}`);
      writes.push({ operation: 'delete', publicBeforeWrite: state?.draft !== true, id });
      assets = assets.filter((asset) => asset.id !== id);
    },
    uploadAsset: async (_url, name, bytes) => {
      calls.push(`upload-${name}`);
      writes.push({ operation: 'upload', publicBeforeWrite: state?.draft !== true, name });
      if (uploadError) throw uploadError;
      const id = nextAssetId++;
      data.set(id, bytes);
      let asset = { id, name, size: bytes.length, state: 'uploaded', digest: digest ? `sha256:${hashBytes(bytes)}` : null };
      if (assetMutation) asset = assetMutation(asset);
      assets.push(asset);
      return asset;
    },
    downloadAsset: async (_repo, id) => { calls.push(`download-${id}`); return data.get(id); },
  };
}

test('stable tags and tag refs reject branches, prereleases and ambiguous versions', () => {
  assert.equal(parseStableTag(tag), version);
  assert.equal(validateTagRef({ ref: `refs/tags/${tag}`, tag, sha }), version);
  for (const badTag of ['main', 'v1.2', 'v01.2.3', 'v1.2.3-rc.1', 'v1.2.3+build', 'v1.2.3\n']) {
    assert.throws(() => parseStableTag(badTag), /stable vX.Y.Z/);
  }
  assert.throws(() => validateTagRef({ ref: 'refs/heads/main', tag: 'main', sha }), /tag ref/);
  assert.throws(() => validateTagRef({ ref: 'refs/tags/v1.2.4', tag, sha }), /disagree/);
  assert.throws(() => validateTagRef({ ref: `refs/tags/${tag}`, tag, sha: 'bad' }), /SHA/);
  assert.equal(decodeBody(encodeBody(body)), body);
});

test('all package and lock versions must match the tag', (t) => {
  const f = fixture(t);
  assert.equal(Object.keys(validateProjectVersions(f.root, version)).length, 6);
  writeFileSync(join(f.root, 'package-lock.json'), JSON.stringify({ version, packages: { '': { version: '1.2.2' } } }));
  assert.throws(() => validateProjectVersions(f.root, version), /package-lock.json#packages/);
  writeFileSync(join(f.root, 'package-lock.json'), JSON.stringify({ version, packages: { '': { version } } }));
  writeFileSync(join(f.root, 'src-tauri/Cargo.lock'), 'version = 3\n\n[[package]]\nname = "app"\nversion = "1.2.2"\n');
  assert.throws(() => validateProjectVersions(f.root, version), /Cargo.lock#app=1.2.2/);
});

test('release checkout must remain pinned to both workflow and preflight SHA', () => {
  validateCommitIdentity({ expectedSha: sha, workflowSha: sha, checkedOutSha: sha });
  assert.throws(() => validateCommitIdentity({ expectedSha: sha, workflowSha: sha, checkedOutSha: 'b'.repeat(40) }), /Commit identity mismatch/);
  assert.throws(() => validateCommitIdentity({ expectedSha: sha, workflowSha: 'b'.repeat(40), checkedOutSha: sha }), /Commit identity mismatch/);
});

test('annotation checks preserve exact body and reject lightweight, empty, wrong SHA and non-main commits', async () => {
  assert.equal((await readTagAnnotation(tagClient(), { repo: 'o/r', tag, sha })).message, body);
  await assert.rejects(readTagAnnotation(tagClient({ getTagRef: async () => ({ object: { type: 'commit' } }) }), { repo: 'o/r', tag, sha }), /lightweight/);
  await assert.rejects(readTagAnnotation(tagClient({ getTagObject: async () => ({ object: { type: 'commit', sha }, message: ' \n' }) }), { repo: 'o/r', tag, sha }), /empty annotation/);
  await assert.rejects(readTagAnnotation(tagClient({ getTagObject: async () => ({ object: { type: 'commit', sha: 'c'.repeat(40) }, message: body }) }), { repo: 'o/r', tag, sha }), /workflow SHA/);
  await assert.rejects(readTagAnnotation(tagClient({ compare: async () => ({ status: 'ahead' }) }), { repo: 'o/r', tag, sha }), /not reachable from main/);
});

test('installer validation requires one nonempty current-version x64 exe and msi', (t) => {
  const f = fixture(t);
  assert.deepEqual(artifacts(f).map((asset) => asset.kind), ['exe', 'msi']);
  assert.throws(() => validateBuildArtifacts({ artifactPaths: [f.paths[0]], appVersion: version, expectedVersion: version }), /one x64 msi/);
  assert.throws(() => validateBuildArtifacts({ artifactPaths: f.paths, appVersion: '1.2.2', expectedVersion: version }), /appVersion/);
  assert.throws(() => validateBuildArtifacts({ artifactPaths: [...f.paths, f.paths[0]], appVersion: version, expectedVersion: version }), /one x64 exe/);
  writeFileSync(f.paths[0], '');
  assert.throws(() => artifacts(f), /empty/);
});

test('new release remains draft until both uploaded installers pass hash checks', async (t) => {
  const f = fixture(t);
  const client = releaseClient();
  const release = await publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) });
  assert.equal(release.draft, false);
  assert.equal(release.body, body);
  assert.ok(client.calls.indexOf('publish') > client.calls.findLastIndex((call) => call.startsWith('upload-')));
  assert.ok(client.calls.slice(0, client.calls.indexOf('publish')).filter((call) => call === 'list-assets').length >= 2);
});

test('draft recovery deletes only matching or starter assets and preserves unrelated uploaded files', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ draft: {
    id: 1, tag_name: tag, draft: true, upload_url: 'https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}',
    assets: [
      { id: 1, name: `FluxReader_${version}_x64-setup.exe`, state: 'uploaded' },
      { id: 2, name: 'partial', state: 'starter' },
      { id: 3, name: 'custom.txt', state: 'uploaded' },
    ],
  } });
  await publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) });
  assert.ok(client.calls.includes('delete-1'));
  assert.ok(client.calls.includes('delete-2'));
  assert.ok(!client.calls.includes('delete-3'));
  assert.ok(!client.calls.includes('create-draft'));
  assert.equal(client.state.name, `FluxReader ${tag}`);
  assert.equal(client.state.prerelease, false);
  assert.equal(Object.hasOwn(client.writes.find((write) => write.operation === 'update').payload, 'draft'), false);
});

test('AR-01: public transition after draft list stops before recovery metadata writes', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ draft: { id: 1, tag_name: tag, draft: true } });
  const listReleases = client.listReleases;
  client.listReleases = async (...args) => {
    const snapshot = await listReleases(...args);
    client.state.draft = false;
    return snapshot;
  };
  await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /no longer a draft/);
  assert.equal(client.state.draft, false);
  assert.deepEqual(client.writes, []);
});

test('AR-01: public transition after first upload blocks the second upload and publish', async (t) => {
  const f = fixture(t);
  const client = releaseClient();
  const upload = client.uploadAsset;
  client.uploadAsset = async (...args) => {
    const result = await upload(...args);
    client.state.draft = false;
    return result;
  };
  await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /no longer a draft/);
  assert.equal(client.writes.filter((write) => write.operation === 'upload').length, 1);
  assert.ok(client.writes.every((write) => !write.publicBeforeWrite));
  assert.ok(!client.calls.includes('publish'));
});

test('draft state and identity failures stop before any recovery write', async (t) => {
  const f = fixture(t);
  const invalidReads = [
    async () => null,
    async () => ({ id: 2, tag_name: tag, draft: true }),
    async () => ({ id: 1, tag_name: 'v1.2.4', draft: true }),
    async () => ({ id: 1, tag_name: tag }),
    async () => { throw new ReleaseError('not found', { status: 404 }); },
    async () => { throw new ReleaseError('forbidden', { status: 403 }); },
    async () => { throw new Error('network failure'); },
  ];
  for (const getRelease of invalidReads) {
    const client = releaseClient({ draft: { id: 1, tag_name: tag, draft: true } });
    client.getRelease = getRelease;
    await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }));
    assert.deepEqual(client.writes, []);
  }
});

test('public transition between deletes blocks remaining deletion and uploads', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ draft: {
    id: 1, tag_name: tag, draft: true,
    assets: artifacts(f).map((artifact, index) => ({ id: index + 10, name: artifact.name, state: 'uploaded' })),
  } });
  const remove = client.deleteAsset;
  client.deleteAsset = async (...args) => {
    await remove(...args);
    client.state.draft = false;
  };
  await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /no longer a draft/);
  assert.equal(client.writes.filter((write) => write.operation === 'delete').length, 1);
  assert.equal(client.writes.filter((write) => write.operation === 'upload').length, 0);
  assert.ok(client.writes.every((write) => !write.publicBeforeWrite));
});

test('final draft check runs after the tag check and blocks publication after an external transition', async (t) => {
  const f = fixture(t);
  const client = releaseClient();
  await assert.rejects(publishRelease({
    client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f),
    beforePublish: async () => { client.state.draft = false; },
  }), /no longer a draft/);
  assert.equal(client.writes.filter((write) => write.operation === 'upload').length, 2);
  assert.ok(!client.calls.includes('publish'));
});

test('unexpected draft installers are retained but block publication', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ draft: {
    id: 1, tag_name: tag, draft: true, upload_url: 'https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}',
    assets: [{ id: 7, name: 'FluxReader_1.2.2_arm64-setup.exe', state: 'uploaded' }],
  } });
  await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /unexpected installers/);
  assert.equal(client.state.draft, true);
  assert.ok(!client.calls.includes('delete-7'));
  assert.ok(!client.calls.includes('publish'));
});

test('tag retargeted after upload is refused before publication', async (t) => {
  const f = fixture(t);
  const client = releaseClient();
  const retargeted = tagClient({ getTagObject: async () => ({ object: { type: 'commit', sha: 'c'.repeat(40) }, message: body }) });
  await assert.rejects(publishRelease({
    client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f),
    beforePublish: () => readTagAnnotation(retargeted, { repo: 'o/r', tag, sha }),
  }), /workflow SHA/);
  assert.equal(client.state.draft, true);
  assert.equal(client.calls.filter((call) => call.startsWith('upload-')).length, 2);
  assert.ok(!client.calls.includes('publish'));
});

test('already-published releases are refused before any write', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ published: { id: 1, tag_name: tag, draft: false } });
  await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /already published/);
  assert.deepEqual(client.calls, ['get-published']);
});

test('GET by tag returning a draft still permits draft recovery', async (t) => {
  const f = fixture(t);
  const draft = { id: 1, tag_name: tag, draft: true, upload_url: 'https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}' };
  const client = releaseClient({ draft, published: draft });
  await publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) });
  assert.ok(client.calls.includes('update-draft'));
  assert.ok(client.calls.includes('publish'));
});

test('upload failure and wrong size/state/hash never publish the draft', async (t) => {
  const f = fixture(t);
  for (const scenario of [
    { uploadError: new Error('upload failed') },
    { assetMutation: (asset) => ({ ...asset, size: asset.size + 1 }) },
    { assetMutation: (asset) => ({ ...asset, state: 'starter' }) },
    { assetMutation: (asset) => ({ ...asset, digest: `sha256:${'0'.repeat(64)}` }) },
  ]) {
    const client = releaseClient(scenario);
    await assert.rejects(publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }));
    assert.equal(client.state.draft, true);
    assert.ok(!client.calls.includes('publish'));
  }
});

test('missing remote installer is rejected', async (t) => {
  const f = fixture(t);
  await assert.rejects(verifyAssets({ listAssets: async () => [] }, { repo: 'o/r', releaseId: 1, artifacts: artifacts(f) }), /Expected one release asset/);
});

test('null digest downloads bytes and checks their hash', async (t) => {
  const f = fixture(t);
  const client = releaseClient({ digest: false });
  await publishRelease({ client, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) });
  assert.equal(client.calls.filter((call) => call.startsWith('download-')).length, 4);
  const bad = releaseClient({ digest: false });
  bad.downloadAsset = async () => Buffer.from('corrupt');
  await assert.rejects(publishRelease({ client: bad, repo: 'o/r', tag, sha, body, artifacts: artifacts(f) }), /hash mismatch/);
  assert.equal(bad.state.draft, true);
});

test('API absence is only a 404; forbidden and network errors propagate', async () => {
  for (const status of [403, 429, 500]) {
    const api = new GitHubApi({ fetchImpl: async () => new Response('error', { status }) });
    await assert.rejects(api.getPublishedRelease('o/r', tag), (error) => error instanceof ReleaseError && error.status === status);
  }
  const absent = new GitHubApi({ fetchImpl: async () => new Response('missing', { status: 404 }) });
  assert.equal(await absent.getPublishedRelease('o/r', tag), null);
  await assert.rejects(absent.getRelease('o/r', 1), (error) => error instanceof ReleaseError && error.status === 404);
  const offline = new GitHubApi({ fetchImpl: async () => { throw new Error('offline'); } });
  await assert.rejects(offline.getPublishedRelease('o/r', tag), /offline/);
});

test('release listing paginates and upload uses the release upload_url', async () => {
  const urls = [];
  const api = new GitHubApi({ token: 'test-only', fetchImpl: async (url, init) => {
    urls.push(url);
    if (init.method === 'POST') return Response.json({ id: 1 });
    return Response.json(url.endsWith('page=1') ? Array.from({ length: 100 }, (_, id) => ({ id })) : [{ id: 101 }]);
  } });
  assert.equal((await api.listReleases('o/r')).length, 101);
  await api.uploadAsset('https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}', 'a file.exe', Buffer.from('a'));
  assert.match(urls.at(-1), /^https:\/\/uploads\.github\.com\/repos\/o\/r\/releases\/1\/assets\?name=a\+file\.exe$/);
  await assert.rejects(api.uploadAsset('https://example.com/assets{?name}', 'a', Buffer.from('a')), /Unexpected release upload URL/);
});

test('workflow wiring keeps complete CI before build and never passes release inputs to tauri-action', () => {
  const release = readFileSync(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8');
  const ci = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
  assert.match(ci, /\n  workflow_call:/);
  assert.doesNotMatch(ci, /\n\s+tags:/);
  assert.match(release, /needs: \[preflight, ci\]/);
  assert.match(release, /uses: \.\/\.github\/workflows\/ci\.yml/);
  assert.doesNotMatch(release, /^\s+(tagName|releaseName|releaseId|releaseDraft):/m);
  for (const command of ['npm run lint', 'npm run build', 'npm run test:frontend', 'npm run test:components', 'npm run test:ui-layout', 'cargo fmt', 'cargo check', 'cargo clippy', 'cargo test']) assert.ok(ci.includes(command), command);
  assert.equal(readFileSync(new URL('../../.node-version', import.meta.url), 'utf8').trim(), '22');
});
