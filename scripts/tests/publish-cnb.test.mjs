import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { publishCnb } from '../publish-cnb.mjs';

const token = 'test-only-cnb-token';
const repo = 'example/Glide-Dock';
const tag = 'v0.4.2';
const executableName = 'Glide-Dock-v0.4.2-Setup.exe';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });

async function fixture(t, options = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'glide-cnb-publish-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(path.join(directory, 'assets'));
  const bytes = Buffer.from('release installer fixture');
  await writeFile(path.join(directory, 'assets', executableName), bytes);
  const checksum = hash(bytes) + '  ' + executableName + '\n';
  await writeFile(path.join(directory, 'assets', executableName + '.sha256'), checksum);
  await writeFile(path.join(directory, 'notes.md'), 'Glide-Dock release notes');
  const files = new Map([[executableName, bytes], [executableName + '.sha256', Buffer.from(checksum)]]);
  const calls = [];
  let release = options.existing ? { id: 'release-id', tag_name: tag, draft: false, assets: [...files].map(([name, bytes]) => ({ name, size: bytes.length, hash_algo: 'sha256', hash_value: hash(bytes) })) } : null;
  const fetchImpl = async (input, init = {}) => {
    const url = new URL(input);
    calls.push({ url, ...init });
    if (url.hostname === 'objects.example.test') {
      assert.equal(init.headers.Authorization, undefined, 'Bearer credentials must not reach object storage');
      assert.equal(init.method, 'PUT');
      assert.equal(init.redirect, 'error');
      const name = decodeURIComponent(url.pathname.slice(1));
      assert.deepEqual(Buffer.from(init.body), files.get(name));
      return new Response('', { status: options.failUpload ? 503 : 200 });
    }
    assert.equal(url.origin, 'https://api.cnb.cool');
    assert.equal(init.headers.Authorization, 'Bearer ' + token);
    assert.equal(init.redirect, 'error');
    if (options.forbidden) return json({ errmsg: 'Missing required scopes: repo-release:rw' }, 403);
    if (url.pathname === '/' + repo) return json({ path: repo });
    if (url.pathname.endsWith('/releases/tags/' + tag)) return release ? json(release) : json({ errmsg: 'Not found' }, 404);
    if (url.pathname === '/' + repo + '/-/releases') {
      const body = JSON.parse(init.body);
      assert.equal(body.draft, true); assert.equal(body.target_commitish, 'commit-123');
      release = { id: 'release-id', tag_name: tag, draft: true, assets: [] };
      return json(release, 201);
    }
    if (url.pathname.endsWith('/asset-upload-url')) {
      const body = JSON.parse(init.body);
      assert.equal(body.overwrite, false); assert.equal(body.ttl, 0);
      assert.equal(body.size, files.get(body.asset_name).length);
      return json({ upload_url: 'https://objects.example.test/' + encodeURIComponent(body.asset_name) + '?signature=private',
        verify_url: options.untrusted ? 'https://attacker.example.test/confirm' : 'https://api.cnb.cool/' + repo + '/-/releases/release-id/asset-upload-confirmation/upload-token/' + encodeURIComponent(body.asset_name) }, 201);
    }
    if (url.pathname.includes('/asset-upload-confirmation/')) {
      const name = decodeURIComponent(url.pathname.split('/').at(-1));
      release.assets.push({ name, size: files.get(name).length, hash_algo: 'sha256', hash_value: hash(files.get(name)) });
      return json({});
    }
    if (url.pathname === '/' + repo + '/-/releases/release-id') {
      if (init.method === 'PATCH') release = { ...release, ...JSON.parse(init.body) };
      return json(release);
    }
    throw new Error('Unexpected request: ' + url.pathname);
  };
  const gitCalls = [];
  const runGit = args => {
    gitCalls.push(args);
    if (args[0] === 'rev-parse') return options.tagMismatch && args[1] !== 'HEAD' ? 'other-commit' : 'commit-123';
    return '';
  };
  const config = { repository: repo, tag, token, assetsDirectory: 'assets', notesFile: 'notes.md', cwd: directory, fetchImpl, runGit, log() {} };
  return { config, calls, gitCalls, directory, getRelease: () => release };
}

test('syncs only the requested branch and tag, confirms both uploads, then publishes', async t => {
  const f = await fixture(t);
  const result = await publishCnb(f.config);
  assert.equal(result.tag, tag); assert.equal(result.commit, 'commit-123');
  assert.deepEqual(f.gitCalls.find(args => args[0] === 'push'), ['push', '--atomic', 'https://cnb.cool/example/Glide-Dock.git', 'HEAD:refs/heads/main', 'refs/tags/v0.4.2:refs/tags/v0.4.2']);
  assert.equal(f.calls.filter(call => call.method === 'PUT').length, 2);
  const patch = f.calls.find(call => call.method === 'PATCH');
  assert.equal(JSON.parse(patch.body).draft, false);
  assert.equal(f.calls.at(-1), patch);
  assert.equal(f.getRelease().assets.length, 2);
});

test('checksum mismatch fails before network or Git side effects', async t => {
  const f = await fixture(t);
  await writeFile(path.join(f.directory, 'assets', executableName), 'modified installer');
  await assert.rejects(publishCnb(f.config), /SHA-256/);
  assert.equal(f.calls.length, 0); assert.equal(f.gitCalls.length, 0);
});

test('a tag pointing to different source cannot be released', async t => {
  const f = await fixture(t, { tagMismatch: true });
  await assert.rejects(publishCnb(f.config), /checked-out commit/);
  assert.equal(f.calls.length, 0); assert.equal(f.gitCalls.some(args => args[0] === 'push'), false);
});

test('API scope errors fail before source is pushed', async t => {
  const f = await fixture(t, { forbidden: true });
  await assert.rejects(publishCnb(f.config), /HTTP 403.*repo-release:rw/);
  assert.equal(f.gitCalls.some(args => args[0] === 'push'), false);
});

test('an object-store failure leaves the release in draft', async t => {
  const f = await fixture(t, { failUpload: true });
  await assert.rejects(publishCnb(f.config), /HTTP 503/);
  assert.equal(f.calls.some(call => call.method === 'PATCH'), false);
  assert.equal(f.getRelease().draft, true);
});

test('rejects an off-domain confirmation URL before uploading', async t => {
  const f = await fixture(t, { untrusted: true });
  await assert.rejects(publishCnb(f.config), /untrusted/);
  assert.equal(f.calls.some(call => call.method === 'PUT'), false);
});

test('retry verifies matching versioned assets without replacing them', async t => {
  const f = await fixture(t, { existing: true });
  await publishCnb(f.config);
  assert.equal(f.calls.some(call => call.method === 'PUT'), false);
  assert.equal(f.calls.some(call => call.url.pathname.endsWith('/asset-upload-url')), false);
});

test('retry refuses to overwrite differing versioned assets', async t => {
  const f = await fixture(t, { existing: true });
  f.getRelease().assets[0].hash_value = '0'.repeat(64);
  await assert.rejects(publishCnb(f.config), /cannot be verified as identical/);
  assert.equal(f.calls.some(call => call.method === 'PUT'), false);
  assert.equal(f.calls.some(call => call.method === 'PATCH'), false);
});

test('missing token and malformed target fail without side effects', async t => {
  const f = await fixture(t);
  await assert.rejects(publishCnb({ ...f.config, token: '' }), /CNB_TOKEN/);
  await assert.rejects(publishCnb({ ...f.config, repository: 'https://wrong.example/repo' }), /organization\/repository/);
  assert.equal(f.calls.length, 0); assert.equal(f.gitCalls.length, 0);
});
