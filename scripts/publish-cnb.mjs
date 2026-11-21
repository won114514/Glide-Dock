import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const apiOrigin = 'https://api.cnb.cool';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

function git(args, token, cwd) {
  // Git invokes this fixed helper through its shell; the token stays in the environment.
  const helper = '!f() { if test "$1" = get; then printf "%s\\n" "username=cnb" "password=$CNB_TOKEN"; fi; }; f';
  return execFileSync('git', ['-c', 'credential.helper=', '-c', 'credential.helper=' + helper, ...args], {
    cwd, encoding: 'utf8', timeout: 120000,
    env: { ...process.env, CNB_TOKEN: token, GIT_TERMINAL_PROMPT: '0', GCM_INTERACTIVE: 'never' },
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
}

export async function publishCnb({
  repository, tag, assetsDirectory, notesFile, token, branch = 'main', cwd = process.cwd(),
  fetchImpl = fetch, runGit = args => git(args, token, cwd), log = console.log, syncSource = true,
}) {
  if (!token) throw new Error('CNB_TOKEN is required; configure it as a repository Actions secret.');
  if (!/^[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)+$/.test(repository ?? '')) throw new Error('CNB repository must be an organization/repository path.');
  if (!/^v\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(tag ?? '')) throw new Error('A version tag such as v0.4.2 is required.');
  if (!/^[A-Za-z0-9_.\/-]+$/.test(branch) || branch.includes('..')) throw new Error('Invalid release branch.');
  const executableName = 'Glide-Dock-' + tag + '-Setup.exe';
  const checksumName = executableName + '.sha256';
  const executable = await readFile(path.resolve(cwd, assetsDirectory, executableName));
  const checksum = await readFile(path.resolve(cwd, assetsDirectory, checksumName));
  const expectedHash = sha256(executable);
  const checksumMatch = checksum.toString('utf8').trim().match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
  if (!checksumMatch || checksumMatch[1].toLowerCase() !== expectedHash || checksumMatch[2] !== executableName) {
    throw new Error('Installer SHA-256 does not match its checksum file.');
  }
  const notes = await readFile(path.resolve(cwd, notesFile ?? ('docs/releases/' + tag + '.md')), 'utf8');
  if (!notes.trim()) throw new Error('Release notes must not be empty.');
  const commit = runGit(['rev-parse', 'HEAD']);
  const taggedCommit = runGit(['rev-parse', 'refs/tags/' + tag + '^{commit}']);
  if (commit !== taggedCommit) throw new Error('The release tag must point to the checked-out commit.');
  runGit(['diff', '--quiet', 'HEAD']);

  const repoPath = '/' + repository.split('/').map(encodeURIComponent).join('/');
  function safeMessage(value) { return String(value).replaceAll(token, '[redacted]').replace(/https?:\/\/\S+/g, '[URL]'); }
  async function request(route, { method = 'GET', body, allowMissing = false } = {}) {
    let response;
    try {
      response = await fetchImpl(apiOrigin + route, {
        method, redirect: 'error', signal: AbortSignal.timeout(60000),
        headers: { Accept: 'application/vnd.cnb.api+json', Authorization: 'Bearer ' + token, ...(body ? { 'Content-Type': 'application/json' } : {}) },
        body: body ? JSON.stringify(body) : undefined,
      });
    } catch { throw new Error('CNB API request could not complete: ' + method + ' ' + route); }
    if (allowMissing && response.status === 404) return null;
    const text = await response.text();
    let result;
    try { result = text ? JSON.parse(text) : null; }
    catch { throw new Error('CNB API returned an invalid response (HTTP ' + response.status + ').'); }
    if (!response.ok) {
      const detail = result?.errmsg ?? result?.message ?? 'Request rejected';
      throw new Error('CNB API HTTP ' + response.status + ': ' + safeMessage(detail));
    }
    return result;
  }

  // Check access before pushing source or creating a release.
  await request(repoPath);
  const releaseByTag = repoPath + '/-/releases/tags/' + encodeURIComponent(tag);
  let release = await request(releaseByTag, { allowMissing: true });
  if (syncSource) {
    log('Synchronizing ' + branch + ' and ' + tag + ' to CNB...');
    runGit(['push', '--atomic', 'https://cnb.cool/' + repository + '.git',
      'HEAD:refs/heads/' + branch, 'refs/tags/' + tag + ':refs/tags/' + tag]);
  }
  if (!release) {
    release = await request(repoPath + '/-/releases', { method: 'POST', body: {
      tag_name: tag, target_commitish: commit, name: 'Glide-Dock ' + tag, body: notes,
      draft: true, prerelease: tag.includes('-'), make_latest: 'false',
    } });
  }
  if (!release?.id || release.tag_name !== tag) throw new Error('CNB returned an unexpected release identity.');
  const releasePath = repoPath + '/-/releases/' + encodeURIComponent(release.id);
  const files = [{ name: executableName, bytes: executable }, { name: checksumName, bytes: checksum }];
  for (const file of files) {
    const existing = release.assets?.find(asset => asset.name === file.name);
    if (existing) {
      const assetHash = existing.hash_algo?.toLowerCase() === 'sha256' ? existing.hash_value?.toLowerCase() : null;
      if (Number(existing.size) !== file.bytes.length || !assetHash || assetHash !== sha256(file.bytes)) {
        // Immutable versioned artifacts must not be silently replaced on a retry.
        throw new Error('Existing CNB asset cannot be verified as identical: ' + file.name);
      }
      log('Verified existing asset: ' + file.name);
      continue;
    }
    const upload = await request(releasePath + '/asset-upload-url', { method: 'POST', body: {
      asset_name: file.name, size: file.bytes.length, overwrite: false, ttl: 0,
    } });
    let uploadUrl, verifyUrl;
    try { uploadUrl = new URL(upload.upload_url); verifyUrl = new URL(upload.verify_url, apiOrigin); }
    catch { throw new Error('CNB returned an invalid upload URL.'); }
    if (uploadUrl.protocol !== 'https:' || uploadUrl.username || uploadUrl.password
      || verifyUrl.origin !== apiOrigin || !verifyUrl.pathname.startsWith(releasePath + '/asset-upload-confirmation/')) {
      throw new Error('CNB returned an untrusted upload or confirmation URL.');
    }
    let uploaded;
    try {
      // The object-store URL is already signed. Never forward the CNB bearer token to it.
      uploaded = await fetchImpl(uploadUrl.href, {
        method: 'PUT', body: file.bytes, redirect: 'error', signal: AbortSignal.timeout(120000),
        headers: { 'Content-Type': 'application/octet-stream', 'Content-Length': String(file.bytes.length) },
      });
    } catch { throw new Error('CNB asset upload could not complete: ' + file.name); }
    if (!uploaded.ok) throw new Error('CNB asset upload failed (HTTP ' + uploaded.status + '): ' + file.name);
    await request(verifyUrl.pathname + verifyUrl.search, { method: 'POST' });
    log('Uploaded and confirmed: ' + file.name);
  }
  // Keep a new release in draft until the server has registered both attachments.
  release = await request(releasePath);
  for (const file of files) {
    const asset = release.assets?.find(asset => asset.name === file.name);
    if (!asset || Number(asset.size) !== file.bytes.length) throw new Error('CNB attachment verification failed: ' + file.name);
    if (asset.hash_algo?.toLowerCase() === 'sha256' && asset.hash_value?.toLowerCase() !== sha256(file.bytes)) {
      throw new Error('CNB attachment hash differs from the local file: ' + file.name);
    }
  }
  release = await request(releasePath, { method: 'PATCH', body: {
    name: 'Glide-Dock ' + tag, body: notes, draft: false, prerelease: tag.includes('-'),
    make_latest: tag.includes('-') ? 'false' : 'true',
  } });
  if (release?.draft) throw new Error('CNB release is still a draft.');
  const url = 'https://cnb.cool/' + repository + '/-/releases/tag/' + encodeURIComponent(tag);
  log('CNB release published: ' + url);
  return { url, tag, commit, sha256: expectedHash };
}

async function main() {
  const { values } = parseArgs({ options: {
    repository: { type: 'string', default: process.env.CNB_REPOSITORY }, tag: { type: 'string', default: process.env.GITHUB_REF_NAME },
    assets: { type: 'string', default: 'release-assets' }, notes: { type: 'string' }, branch: { type: 'string', default: 'main' },
  } });
  try {
    await publishCnb({ repository: values.repository, tag: values.tag, assetsDirectory: values.assets,
      notesFile: values.notes, branch: values.branch, token: process.env.CNB_TOKEN });
  } catch (error) {
    const token = process.env.CNB_TOKEN;
    const message = error instanceof Error ? error.message : String(error);
    console.error(token ? message.replaceAll(token, '[redacted]') : message);
    process.exitCode = 1;
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
