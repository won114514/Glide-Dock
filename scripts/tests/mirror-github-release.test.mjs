import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { mirrorGithubRelease } from '../mirror-github-release.mjs';

const tag = 'v0.4.2';
const repository = 'example/Glide-Dock';
const token = 'temporary-pipeline-token';
const commit = 'a'.repeat(40);
const executableName = 'Glide-Dock-' + tag + '-Setup.exe';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const json = (data, status = 200) => new Response(JSON.stringify(data), { status });

async function fixture(t, options = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'glide-mirror-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const executable = Buffer.from('official GitHub build');
  const checksum = (options.badChecksum ? '0'.repeat(64) : hash(executable)) + '  ' + executableName + '\n';
  const files = new Map([[executableName, executable], [executableName + '.sha256', Buffer.from(checksum)]]);
  const release = { tag_name: tag, draft: false, assets: [...files].map(([name, bytes]) => ({
    name, size: bytes.length, digest: 'sha256:' + (options.badDigest ? '0'.repeat(64) : hash(bytes)),
    browser_download_url: (options.badUrl ? 'https://attacker.example' : 'https://github.com') + '/' + repository + '/releases/download/' + tag + '/' + name,
  })) };
  if (options.missingAsset) release.assets.pop();
  const calls = [], gitCalls = [], pauses = [];
  let lookups = 0, publication;
  const fetchImpl = async (input, init = {}) => {
    const url = new URL(input);
    calls.push({ url, ...init });
    const expectedAuthorization = url.hostname === 'api.github.com' && options.githubToken
      ? 'Bearer ' + options.githubToken : undefined;
    assert.equal(init.headers?.Authorization, expectedAuthorization, 'Only the GitHub API receives its own credential');
    if (url.hostname === 'github.com') {
      const bytes = files.get(url.pathname.split('/').at(-1));
      assert.ok(bytes); return new Response(bytes);
    }
    assert.equal(url.hostname, 'api.github.com');
    if (url.pathname.endsWith('/releases/tags/' + tag)) {
      lookups++;
      if (options.unpublished || options.waitFirst && lookups === 1) return json({}, 404);
      return json(release);
    }
    if (url.pathname.endsWith('/git/ref/tags/' + tag)) return json({ object: { type: 'tag', sha: 'b'.repeat(40) } });
    if (url.pathname.endsWith('/git/tags/' + 'b'.repeat(40))) return json({ object: { type: 'commit', sha: commit } });
    throw new Error('Unexpected GitHub request: ' + url.pathname);
  };
  const runGit = args => {
    gitCalls.push(args);
    if (args[0] === 'rev-parse') return options.differentCommit ? 'c'.repeat(40) : commit;
    return '';
  };
  const config = { tag, repository, githubRepository: repository, token, githubToken: options.githubToken, cwd: directory, temporaryRoot: directory,
    publication: options.publication, fetchImpl, runGit, attempts: 2, sleep: async ms => pauses.push(ms), log() {},
    publish: async value => { publication = value; return { tag, commit }; } };
  return { config, calls, gitCalls, pauses, files, getPublication: () => publication };
}

test('mirrors verified GitHub bytes using the matching CNB tag and provided credential', async t => {
  const f = await fixture(t);
  assert.deepEqual(await mirrorGithubRelease(f.config), { tag, commit });
  const publication = f.getPublication();
  assert.equal(publication.syncSource, false);
  assert.equal(publication.token, token);
  assert.equal(publication.repository, repository);
  assert.deepEqual(f.gitCalls[0], ['fetch', 'https://cnb.cool/' + repository + '.git', 'refs/tags/' + tag + ':refs/tags/' + tag]);
  assert.equal(f.gitCalls.some(args => args[0] === 'push'), false);
  assert.deepEqual(f.gitCalls.at(-1).slice(0, 3), ['worktree', 'add', '--detach']);
  assert.equal(f.gitCalls.at(-1).at(-1), commit);
  for (const [name, bytes] of f.files) assert.deepEqual(await readFile(path.join(publication.assetsDirectory, name)), bytes);
});

test('authenticates GitHub API lookups without forwarding either credential to public downloads', async t => {
  const githubToken = 'temporary-github-workflow-token';
  const f = await fixture(t, { githubToken });
  await mirrorGithubRelease(f.config);
  const apiCalls = f.calls.filter(call => call.url.hostname === 'api.github.com');
  assert.equal(apiCalls.length, 3);
  assert.ok(apiCalls.every(call => call.headers.Authorization === 'Bearer ' + githubToken));
  const downloads = f.calls.filter(call => call.url.hostname === 'github.com');
  assert.equal(downloads.length, 2);
  assert.ok(downloads.every(call => call.headers === undefined));
  assert.equal(f.getPublication().githubToken, undefined);
  assert.equal(f.getPublication().token, token);
});

test('preserves explicit Release publication for the native CNB temporary credential', async t => {
  const f = await fixture(t, { publication: 'release' });
  await mirrorGithubRelease(f.config);
  assert.equal(f.getPublication().publication, 'release');
});

test('rejects invalid publication modes before network or Git side effects', async t => {
  const f = await fixture(t, { publication: 'invalid' });
  await assert.rejects(mirrorGithubRelease(f.config), /publication must be/);
  assert.equal(f.calls.length, 0);
  assert.equal(f.gitCalls.length, 0);
});

test('waits for GitHub publication without forwarding CNB credentials', async t => {
  const f = await fixture(t, { waitFirst: true });
  await mirrorGithubRelease(f.config);
  assert.deepEqual(f.pauses, [30000]);
});

test('fails without publishing or changing Git refs when the release stays unavailable', async t => {
  const f = await fixture(t, { unpublished: true });
  await assert.rejects(mirrorGithubRelease(f.config), /not ready/);
  assert.equal(f.getPublication(), undefined); assert.equal(f.gitCalls.length, 0);
});

test('waits for both expected installer assets before downloading', async t => {
  const f = await fixture(t, { missingAsset: true });
  await assert.rejects(mirrorGithubRelease(f.config), /not ready/);
  assert.equal(f.gitCalls.length, 0);
});

test('rejects different GitHub and CNB tag commits before downloading or publishing', async t => {
  const f = await fixture(t, { differentCommit: true });
  await assert.rejects(mirrorGithubRelease(f.config), /different commits/);
  assert.equal(f.getPublication(), undefined);
  assert.equal(f.calls.some(call => call.url.hostname === 'github.com'), false);
});

test('rejects corrupt checksum files before creating a source worktree or publishing', async t => {
  const f = await fixture(t, { badChecksum: true });
  await assert.rejects(mirrorGithubRelease(f.config), /SHA-256/);
  assert.equal(f.getPublication(), undefined); assert.equal(f.gitCalls.some(args => args[0] === 'worktree'), false);
});

test('rejects a GitHub asset digest mismatch', async t => {
  const f = await fixture(t, { badDigest: true });
  await assert.rejects(mirrorGithubRelease(f.config), /digest mismatch/);
  assert.equal(f.getPublication(), undefined);
});

test('rejects an installer URL outside the expected GitHub repository', async t => {
  const f = await fixture(t, { badUrl: true });
  await assert.rejects(mirrorGithubRelease(f.config), /unexpected installer download URL/);
  assert.equal(f.getPublication(), undefined);
});
