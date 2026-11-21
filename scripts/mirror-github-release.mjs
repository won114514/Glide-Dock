import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { publishCnb } from './publish-cnb.mjs';

const git = (args, cwd) => execFileSync('git', args, { cwd, encoding: 'utf8', timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

export async function mirrorGithubRelease({
  tag, repository, githubRepository = 'won114514/Glide-Dock', token, cwd = process.cwd(),
  fetchImpl = fetch, runGit = args => git(args, cwd), publish = publishCnb,
  sleep = pause, attempts = 40, log = console.log, temporaryRoot = os.tmpdir(),
}) {
  if (!token) throw new Error('CNB pipeline token is required. Run this script inside a trusted CNB pipeline.');
  if (!/^[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)+$/.test(repository ?? '')) throw new Error('Invalid CNB repository.');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(githubRepository)) throw new Error('Invalid GitHub repository.');
  if (!/^v\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(tag ?? '')) throw new Error('Invalid version tag.');
  async function request(route, allowMissing = false) {
    const response = await fetchImpl('https://api.github.com/repos/' + githubRepository + route, {
      headers: { Accept: 'application/vnd.github+json', 'User-Agent': 'Glide-Dock-CNB-release-mirror' },
      redirect: 'error', signal: AbortSignal.timeout(30000),
    });
    if (allowMissing && response.status === 404) return null;
    if (!response.ok) throw new Error('GitHub release lookup failed (HTTP ' + response.status + ').');
    return response.json();
  }
  let release;
  const executableName = 'Glide-Dock-' + tag + '-Setup.exe';
  const names = [executableName, executableName + '.sha256'];
  for (let attempt = 0; attempt < attempts; attempt++) {
    release = await request('/releases/tags/' + encodeURIComponent(tag), true);
    if (release && !release.draft && names.every(name => release.assets?.some(asset => asset.name === name))) break;
    release = null;
    if (attempt + 1 < attempts) {
      log('Waiting for GitHub installer assets for ' + tag + '...');
      await sleep(30000);
    }
  }
  if (!release || release.tag_name !== tag) throw new Error('GitHub release assets are not ready for ' + tag + '.');
  let remoteObject = (await request('/git/ref/tags/' + encodeURIComponent(tag))).object;
  for (let depth = 0; remoteObject?.type === 'tag' && depth < 4; depth++) {
    remoteObject = (await request('/git/tags/' + remoteObject.sha)).object;
  }
  if (remoteObject?.type !== 'commit' || !/^[a-f0-9]{40}$/.test(remoteObject.sha)) throw new Error('GitHub tag did not resolve to a commit.');
  // Fetch from CNB: its immutable tag must match the GitHub source used to build the installer.
  runGit(['fetch', 'origin', 'refs/tags/' + tag + ':refs/tags/' + tag]);
  const commit = runGit(['rev-parse', 'refs/tags/' + tag + '^{commit}']);
  if (commit !== remoteObject.sha) throw new Error('GitHub and CNB version tags point to different commits.');
  const scratch = await mkdtemp(path.join(temporaryRoot, 'glide-release-mirror-'));
  const assetsDirectory = path.join(scratch, 'assets');
  await mkdir(assetsDirectory);
  const files = new Map();
  for (const name of names) {
    const asset = release.assets.find(asset => asset.name === name);
    const url = new URL(asset.browser_download_url);
    if (url.origin !== 'https://github.com' || url.pathname !== '/' + githubRepository + '/releases/download/' + tag + '/' + name) {
      throw new Error('GitHub returned an unexpected installer download URL.');
    }
    // GitHub downloads may redirect to its signed asset storage. No CNB credential is sent.
    const response = await fetchImpl(url.href, { signal: AbortSignal.timeout(180000) });
    if (!response.ok) throw new Error('Installer download failed (HTTP ' + response.status + '): ' + name);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (bytes.length !== Number(asset.size)) throw new Error('Downloaded asset size differs from GitHub: ' + name);
    if (asset.digest?.startsWith('sha256:') && asset.digest.slice(7) !== hash(bytes)) throw new Error('GitHub asset digest mismatch: ' + name);
    files.set(name, bytes);
    await writeFile(path.join(assetsDirectory, name), bytes);
  }
  const checksum = files.get(names[1]).toString('utf8').trim().match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
  if (!checksum || checksum[1].toLowerCase() !== hash(files.get(executableName)) || checksum[2] !== executableName) {
    throw new Error('Downloaded installer does not match its SHA-256 file.');
  }
  const sourceDirectory = path.join(scratch, 'source');
  runGit(['worktree', 'add', '--detach', sourceDirectory, commit]);
  log('Verified GitHub installer SHA-256: ' + hash(files.get(executableName)));
  // The tag is already in CNB. Do not move main backward when mirroring an older release.
  return publish({ repository, tag, token, cwd: sourceDirectory, assetsDirectory, syncSource: false, fetchImpl, log });
}

async function main() {
  const { values } = parseArgs({ options: { tag: { type: 'string' } } });
  let tag = values.tag;
  if (!tag && process.env.CNB_IS_TAG === 'true') tag = process.env.CNB_BRANCH;
  if (!tag) {
    const cargo = await readFile('Cargo.toml', 'utf8');
    const version = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    if (!version) throw new Error('Cannot read workspace release version.');
    tag = 'v' + version;
  }
  await mirrorGithubRelease({ tag, repository: process.env.CNB_REPO_SLUG ?? process.env.CNB_REPOSITORY,
    githubRepository: process.env.GITHUB_REPOSITORY ?? 'won114514/Glide-Dock', token: process.env.CNB_TOKEN });
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { await main(); }
  catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(process.env.CNB_TOKEN ? message.replaceAll(process.env.CNB_TOKEN, '[redacted]') : message);
    process.exitCode = 1;
  }
}
