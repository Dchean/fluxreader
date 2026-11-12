import { appendFileSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { basename, dirname, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';

const DEFAULT_API_URL = 'https://api.github.com';
const RELEASE_TAG_RE = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const APP_NAME = 'FluxReader';

// Note: tag 来源、精确提交门禁及草稿验包后公开的边界，见
// .agents/notes/implemented/process/2026-09-29-发布说明取自-tag-附注.md

export class ReleaseError extends Error {
  constructor(message, { status, url, details } = {}) {
    super(message);
    this.name = 'ReleaseError';
    this.status = status;
    this.url = url;
    this.details = details;
  }
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function parseStableTag(tag) {
  const match = RELEASE_TAG_RE.exec(tag ?? '');
  if (!match || match[0] !== tag) {
    throw new ReleaseError(`Tag must be a stable vX.Y.Z version: ${tag || '(empty)'}`);
  }
  return match.slice(1).join('.');
}

export function validateTagRef({ ref, tag, sha }) {
  if (!ref || !ref.startsWith('refs/tags/')) {
    throw new ReleaseError(`Release must run on a tag ref; got ${ref || '(empty)'}`);
  }
  if (ref !== `refs/tags/${tag}`) {
    throw new ReleaseError(`Tag ref and tag name disagree: ${ref} vs ${tag}`);
  }
  if (!sha || !/^[0-9a-f]{40}$/i.test(sha)) {
    throw new ReleaseError(`Invalid tagged commit SHA: ${sha || '(empty)'}`);
  }
  return parseStableTag(tag);
}

export function validateCommitIdentity({ expectedSha, workflowSha, checkedOutSha }) {
  if (expectedSha !== workflowSha || checkedOutSha !== expectedSha) {
    throw new ReleaseError(`Commit identity mismatch: expected=${expectedSha}, workflow=${workflowSha}, checkout=${checkedOutSha}`);
  }
}

function readJson(root, relativePath) {
  return JSON.parse(readFileSync(resolve(root, relativePath), 'utf8'));
}

function readCargoPackageVersion(root) {
  const cargoToml = readFileSync(resolve(root, 'src-tauri/Cargo.toml'), 'utf8');
  const match = /^version\s*=\s*"([^"]+)"/m.exec(cargoToml);
  if (!match) throw new ReleaseError('src-tauri/Cargo.toml has no package version');
  return match[1];
}

function readCargoLockVersion(root) {
  const cargoLock = readFileSync(resolve(root, 'src-tauri/Cargo.lock'), 'utf8');
  const packageBlocks = cargoLock.split(/^\[\[package\]\]\s*$/m).slice(1);
  const app = packageBlocks.find((block) => /^name\s*=\s*"app"\s*$/m.test(block));
  const match = app && /^version\s*=\s*"([^"]+)"/m.exec(app);
  if (!match) throw new ReleaseError('src-tauri/Cargo.lock has no app package version');
  return match[1];
}

export function readProjectVersions(root) {
  const packageJson = readJson(root, 'package.json');
  const packageLock = readJson(root, 'package-lock.json');
  const tauri = readJson(root, 'src-tauri/tauri.conf.json');
  return {
    'package.json': packageJson.version,
    'package-lock.json': packageLock.version,
    'package-lock.json#packages[""]': packageLock.packages?.['']?.version,
    'src-tauri/tauri.conf.json': tauri.version,
    'src-tauri/Cargo.toml': readCargoPackageVersion(root),
    'src-tauri/Cargo.lock#app': readCargoLockVersion(root),
  };
}

export function validateProjectVersions(root, expectedVersion) {
  const versions = readProjectVersions(root);
  const mismatches = Object.entries(versions)
    .filter(([, value]) => value !== expectedVersion)
    .map(([file, value]) => `${file}=${value ?? '(missing)'}`);
  if (mismatches.length) {
    throw new ReleaseError(
      `Version ${expectedVersion} does not match: ${mismatches.join(', ')}`,
    );
  }
  return versions;
}

function artifactNamePatterns(version, appName) {
  const app = escapeRegExp(appName);
  const ver = escapeRegExp(version);
  return {
    exe: new RegExp(`^${app}_${ver}_x64-setup\\.exe$`),
    msi: new RegExp(`^${app}_${ver}_x64(?:_[A-Za-z0-9.-]+)?\\.msi$`),
  };
}

export function parseArtifactPaths(value) {
  let paths;
  try {
    paths = typeof value === 'string' ? JSON.parse(value) : value;
  } catch (error) {
    throw new ReleaseError(`tauri-action artifactPaths is not valid JSON: ${error.message}`);
  }
  if (!Array.isArray(paths)) throw new ReleaseError('tauri-action artifactPaths must be an array');
  return paths;
}

export function validateBuildArtifacts({ artifactPaths, appVersion, expectedVersion, appName = APP_NAME }) {
  if (appVersion !== expectedVersion) {
    throw new ReleaseError(`tauri-action appVersion ${appVersion} does not match tag version ${expectedVersion}`);
  }
  const paths = parseArtifactPaths(artifactPaths);
  const patterns = artifactNamePatterns(expectedVersion, appName);
  const matching = { exe: [], msi: [] };
  for (const path of paths) {
    if (typeof path !== 'string') continue;
    const name = basename(path);
    if (patterns.exe.test(name)) matching.exe.push({ path, name });
    if (patterns.msi.test(name)) matching.msi.push({ path, name });
  }
  for (const kind of ['exe', 'msi']) {
    if (matching[kind].length !== 1) {
      throw new ReleaseError(`Expected exactly one x64 ${kind} artifact, found ${matching[kind].length}`);
    }
  }
  const result = [];
  for (const kind of ['exe', 'msi']) {
    const artifact = matching[kind][0];
    let info;
    try {
      info = statSync(artifact.path);
    } catch (error) {
      throw new ReleaseError(`Artifact ${artifact.path} is unavailable: ${error.message}`);
    }
    if (!info.isFile() || info.size <= 0) throw new ReleaseError(`Artifact ${artifact.path} is empty`);
    result.push({ ...artifact, kind, size: info.size, sha256: hashFile(artifact.path) });
  }
  return result;
}

export function hashFile(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

export function hashBytes(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

export function encodeBody(body) {
  return Buffer.from(body, 'utf8').toString('base64');
}

export function decodeBody(encoded) {
  return Buffer.from(encoded, 'base64').toString('utf8');
}

export class GitHubApi {
  constructor({ token, baseUrl = DEFAULT_API_URL, fetchImpl = globalThis.fetch } = {}) {
    if (typeof fetchImpl !== 'function') throw new ReleaseError('Fetch API is unavailable');
    this.token = token;
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.fetchImpl = fetchImpl;
  }

  url(path) {
    return path.startsWith('http') ? path : `${this.baseUrl}${path}`;
  }

  async request(method, path, { body, headers = {}, raw = false } = {}) {
    const url = this.url(path);
    const requestHeaders = {
      accept: 'application/vnd.github+json',
      'x-github-api-version': '2022-11-28',
      ...(this.token ? { authorization: `Bearer ${this.token}` } : {}),
      ...headers,
    };
    const response = await this.fetchImpl(url, {
      method,
      headers: requestHeaders,
      signal: AbortSignal.timeout(60_000),
      ...(body === undefined ? {} : { body }),
    });
    if (!response.ok) {
      let details = '';
      try { details = await response.text(); } catch { /* preserve status */ }
      throw new ReleaseError(`GitHub API ${method} ${path} failed (${response.status})`, {
        status: response.status,
        url,
        details,
      });
    }
    if (raw) return response;
    if (response.status === 204) return null;
    return response.json();
  }

  async get(path) { return this.request('GET', path); }

  async getTagRef(repo, tag) {
    return this.get(`/repos/${repo}/git/ref/tags/${encodeURIComponent(tag)}`);
  }

  async getTagObject(repo, sha) {
    return this.get(`/repos/${repo}/git/tags/${sha}`);
  }

  async compare(repo, base, head) {
    return this.get(`/repos/${repo}/compare/${encodeURIComponent(base)}...${head}`);
  }

  async getPublishedRelease(repo, tag) {
    try {
      return await this.get(`/repos/${repo}/releases/tags/${encodeURIComponent(tag)}`);
    } catch (error) {
      if (error instanceof ReleaseError && error.status === 404) return null;
      throw error;
    }
  }

  async getRelease(repo, releaseId) {
    return this.get(`/repos/${repo}/releases/${releaseId}`);
  }

  async listReleases(repo) {
    const releases = [];
    for (let page = 1; ; page += 1) {
      const batch = await this.get(`/repos/${repo}/releases?per_page=100&page=${page}`);
      if (!Array.isArray(batch)) throw new ReleaseError('GitHub releases response is not an array');
      releases.push(...batch);
      if (batch.length < 100) return releases;
    }
  }

  async listAssets(repo, releaseId) {
    const assets = [];
    for (let page = 1; ; page += 1) {
      const batch = await this.get(`/repos/${repo}/releases/${releaseId}/assets?per_page=100&page=${page}`);
      if (!Array.isArray(batch)) throw new ReleaseError('GitHub assets response is not an array');
      assets.push(...batch);
      if (batch.length < 100) return assets;
    }
  }

  async createRelease(repo, payload) {
    return this.request('POST', `/repos/${repo}/releases`, {
      body: JSON.stringify(payload),
      headers: { 'content-type': 'application/json' },
    });
  }

  async updateRelease(repo, releaseId, payload) {
    return this.request('PATCH', `/repos/${repo}/releases/${releaseId}`, {
      body: JSON.stringify(payload),
      headers: { 'content-type': 'application/json' },
    });
  }

  async deleteAsset(repo, assetId) {
    return this.request('DELETE', `/repos/${repo}/releases/assets/${assetId}`);
  }

  async uploadAsset(uploadUrl, name, bytes) {
    const url = new URL(uploadUrl.replace(/\{.*$/, ''));
    if (url.protocol !== 'https:' || url.hostname !== 'uploads.github.com') {
      throw new ReleaseError(`Unexpected release upload URL: ${url.origin}`);
    }
    url.searchParams.set('name', name);
    return this.request('POST', url.toString(), {
      body: bytes,
      headers: {
        'content-type': 'application/octet-stream',
        'content-length': String(bytes.byteLength),
      },
    });
  }

  async downloadAsset(repo, assetId) {
    const response = await this.request('GET', `/repos/${repo}/releases/assets/${assetId}`, {
      raw: true,
      headers: { accept: 'application/octet-stream' },
    });
    return Buffer.from(await response.arrayBuffer());
  }
}

export async function readTagAnnotation(client, { repo, tag, sha }) {
  const ref = await client.getTagRef(repo, tag);
  if (ref?.object?.type !== 'tag') throw new ReleaseError(`Tag ${tag} is lightweight; an annotated tag is required`);
  const tagObject = await client.getTagObject(repo, ref.object.sha);
  if (tagObject?.object?.type !== 'commit' || tagObject.object.sha !== sha) {
    throw new ReleaseError(`Tag ${tag} does not point to the workflow SHA ${sha}`);
  }
  const message = typeof tagObject.message === 'string' ? tagObject.message : '';
  if (!message.trim()) throw new ReleaseError(`Tag ${tag} has an empty annotation`);
  const compare = await client.compare(repo, 'main', sha);
  if (!['behind', 'identical'].includes(compare?.status)) {
    throw new ReleaseError(`Tagged commit ${sha} is not reachable from main (status ${compare?.status || 'unknown'})`);
  }
  return { message, tagObject };
}

function expectedAssetNames(artifacts) {
  return new Set(artifacts.map((artifact) => artifact.name));
}

function validateDraftRelease(release, { releaseId, tag }) {
  if (!Number.isSafeInteger(releaseId) || releaseId <= 0
      || !release || release.id !== releaseId || release.tag_name !== tag) {
    throw new ReleaseError(`Release ${tag} identity changed or is missing (expected ID ${releaseId})`);
  }
  if (release.draft !== true) {
    throw new ReleaseError(`Release ${tag} is no longer a draft; refusing further writes`);
  }
  return release;
}

async function requireCurrentDraft(client, { repo, releaseId, tag }) {
  return validateDraftRelease(await client.getRelease(repo, releaseId), { releaseId, tag });
}

export async function verifyAssets(client, { repo, releaseId, artifacts }) {
  const assets = await client.listAssets(repo, releaseId);
  const expected = expectedAssetNames(artifacts);
  const unexpectedInstallers = assets.filter((asset) => /\.(exe|msi)$/i.test(asset.name) && !expected.has(asset.name));
  if (unexpectedInstallers.length) {
    throw new ReleaseError(`Draft contains unexpected installers: ${unexpectedInstallers.map((asset) => asset.name).join(', ')}`);
  }
  const verified = [];
  for (const artifact of artifacts) {
    const matches = assets.filter((asset) => asset.name === artifact.name);
    if (matches.length !== 1) throw new ReleaseError(`Expected one release asset named ${artifact.name}, found ${matches.length}`);
    const remote = matches[0];
    if (remote.state !== 'uploaded') throw new ReleaseError(`Release asset ${artifact.name} is not uploaded (state ${remote.state || 'unknown'})`);
    if (Number(remote.size) !== artifact.size) throw new ReleaseError(`Release asset ${artifact.name} has size ${remote.size}, expected ${artifact.size}`);
    let remoteHash;
    if (typeof remote.digest === 'string' && /^sha256:[0-9a-f]{64}$/.test(remote.digest)) {
      remoteHash = remote.digest.slice('sha256:'.length);
    } else if (remote.digest == null) {
      remoteHash = hashBytes(await client.downloadAsset(repo, remote.id));
    } else {
      throw new ReleaseError(`Release asset ${artifact.name} has an invalid digest`);
    }
    if (remoteHash !== artifact.sha256) throw new ReleaseError(`Release asset ${artifact.name} hash mismatch`);
    verified.push({ ...remote, sha256: remoteHash });
  }
  return verified;
}

export async function publishRelease({ client, repo, tag, sha, body, artifacts, appName = APP_NAME, beforePublish = async () => {} }) {
  if (!Array.isArray(artifacts) || artifacts.length !== 2) throw new ReleaseError('Exactly the validated exe and msi artifacts are required');
  const published = await client.getPublishedRelease(repo, tag);
  if (published && published.draft !== true) throw new ReleaseError(`Release ${tag} is already published; refusing to rewrite it`);

  const releases = await client.listReleases(repo);
  if (releases.some((release) => release.tag_name === tag && release.draft !== true)) {
    throw new ReleaseError(`Release ${tag} is already published; refusing to rewrite it`);
  }
  const drafts = releases.filter((release) => release.tag_name === tag && release.draft === true);
  if (drafts.length > 1) throw new ReleaseError(`Multiple draft releases found for ${tag}`);
  let release = drafts[0];
  if (!release) {
    release = await client.createRelease(repo, {
      tag_name: tag,
      target_commitish: sha,
      name: `${appName} ${tag}`,
      body,
      draft: true,
      prerelease: false,
    });
    validateDraftRelease(release, { releaseId: release?.id, tag });
  } else {
    const releaseId = release.id;
    await requireCurrentDraft(client, { repo, releaseId, tag });
    // A recovery write must never turn a release published by another client back into a draft.
    release = await client.updateRelease(repo, releaseId, { name: `${appName} ${tag}`, body, prerelease: false });
    validateDraftRelease(release, { releaseId, tag });
  }

  const releaseId = release.id;
  const names = expectedAssetNames(artifacts);
  const existingAssets = await client.listAssets(repo, releaseId);
  const unexpectedInstallers = existingAssets.filter((asset) => /\.(exe|msi)$/i.test(asset.name) && !names.has(asset.name) && asset.state !== 'starter');
  if (unexpectedInstallers.length) {
    throw new ReleaseError(`Draft contains unexpected installers: ${unexpectedInstallers.map((asset) => asset.name).join(', ')}`);
  }
  for (const asset of existingAssets) {
    if (names.has(asset.name) || asset.state === 'starter') {
      await requireCurrentDraft(client, { repo, releaseId, tag });
      await client.deleteAsset(repo, asset.id);
    }
  }
  for (const artifact of artifacts) {
    const bytes = readFileSync(artifact.path);
    const current = await requireCurrentDraft(client, { repo, releaseId, tag });
    await client.uploadAsset(current.upload_url, artifact.name, bytes);
  }
  await verifyAssets(client, { repo, releaseId, artifacts });
  await beforePublish();
  // GET and the following write are not atomic; callers must not edit this tag's release concurrently.
  await requireCurrentDraft(client, { repo, releaseId, tag });

  try {
    await client.updateRelease(repo, releaseId, { name: `${appName} ${tag}`, body, draft: false, prerelease: false });
  } catch (error) {
    throw new ReleaseError(`Publishing ${tag} did not return success; inspect its current state before retrying. ${error.message}`);
  }
  const finalRelease = await client.getPublishedRelease(repo, tag);
  if (!finalRelease || finalRelease.id !== releaseId || finalRelease.draft !== false
      || finalRelease.prerelease !== false || finalRelease.tag_name !== tag || finalRelease.name !== `${appName} ${tag}` || finalRelease.body !== body) {
    throw new ReleaseError(`Published release ${tag} failed final verification`);
  }
  await verifyAssets(client, { repo, releaseId: finalRelease.id, artifacts });
  return finalRelease;
}

function output(name, value) {
  const target = process.env.GITHUB_OUTPUT;
  if (!target) return;
  appendFileSync(target, `${name}=${value}\n`);
}

function env(name) {
  const value = process.env[name];
  if (!value) throw new ReleaseError(`Required environment variable ${name} is missing`);
  return value;
}

async function commandPreflight() {
  const tag = env('GITHUB_REF_NAME');
  const sha = env('GITHUB_SHA').toLowerCase();
  const version = validateTagRef({ ref: env('GITHUB_REF'), tag, sha });
  const root = process.env.GITHUB_WORKSPACE || process.cwd();
  const checkedOutSha = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
  validateCommitIdentity({ expectedSha: sha, workflowSha: sha, checkedOutSha });
  validateProjectVersions(root, version);
  const client = new GitHubApi({ token: env('GITHUB_TOKEN'), baseUrl: process.env.GITHUB_API_URL || DEFAULT_API_URL });
  const { message } = await readTagAnnotation(client, { repo: env('GITHUB_REPOSITORY'), tag, sha });
  output('tag', tag);
  output('version', version);
  output('sha', sha);
  output('body_b64', encodeBody(message));
  console.log(`Preflight passed for ${tag} (${sha})`);
}

async function validateReleaseIdentity(client) {
  const tag = env('RELEASE_TAG');
  const sha = env('RELEASE_SHA');
  const repo = env('GITHUB_REPOSITORY');
  const root = process.env.GITHUB_WORKSPACE || process.cwd();
  const checkedOutSha = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
  validateCommitIdentity({ expectedSha: sha, workflowSha: env('GITHUB_SHA'), checkedOutSha });
  const version = validateTagRef({ ref: env('GITHUB_REF'), tag, sha });
  validateProjectVersions(root, version);
  const { message } = await readTagAnnotation(client, { repo, tag, sha });
  if (message !== decodeBody(env('RELEASE_BODY_B64'))) throw new ReleaseError('Tag annotation changed after preflight');
  return { tag, sha, repo, version, body: message };
}

async function commandValidateCheckout() {
  const client = new GitHubApi({ token: env('GITHUB_TOKEN'), baseUrl: process.env.GITHUB_API_URL || DEFAULT_API_URL });
  const { tag, sha, repo } = await validateReleaseIdentity(client);
  const existing = await client.getPublishedRelease(repo, tag);
  if (existing && existing.draft !== true) throw new ReleaseError(`Release ${tag} is already published; refusing to rewrite it`);
  console.log(`Release checkout fixed at ${tag} (${sha})`);
}

function commandValidateArtifacts() {
  const expectedVersion = env('EXPECTED_VERSION');
  const artifacts = validateBuildArtifacts({
    artifactPaths: env('TAURI_ARTIFACT_PATHS'),
    appVersion: env('TAURI_APP_VERSION'),
    expectedVersion,
  });
  const target = env('ARTIFACT_MANIFEST_PATH');
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, JSON.stringify(artifacts, null, 2));
  console.log(`Validated ${artifacts.map((artifact) => artifact.name).join(', ')}`);
}

async function commandPublish() {
  const manifest = JSON.parse(readFileSync(env('ARTIFACT_MANIFEST_PATH'), 'utf8'));
  const client = new GitHubApi({ token: env('GITHUB_TOKEN'), baseUrl: process.env.GITHUB_API_URL || DEFAULT_API_URL });
  const { tag, sha, repo, version, body } = await validateReleaseIdentity(client);
  const artifacts = validateBuildArtifacts({
    artifactPaths: manifest.map((artifact) => artifact.path),
    appVersion: env('RELEASE_VERSION'),
    expectedVersion: version,
  });
  const release = await publishRelease({
    client,
    repo,
    tag,
    sha,
    body,
    artifacts,
    beforePublish: () => validateReleaseIdentity(client),
  });
  console.log(`Published ${release.html_url || release.tag_name}`);
}

const command = process.argv[2];
if (command === 'preflight') await commandPreflight();
else if (command === 'validate-checkout') await commandValidateCheckout();
else if (command === 'validate-artifacts') commandValidateArtifacts();
else if (command === 'publish') await commandPublish();
