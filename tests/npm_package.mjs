import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { chmod, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

import { archiveFor, checksumFor, download, install, targetFor } from '../npm/install.mjs'

const cargoToml = readFileSync('Cargo.toml', 'utf8')
const cargoVersion = /^version = "([^"]+)"/m.exec(cargoToml)[1]
const cargoDescription = /^description = "([^"]+)"/m.exec(cargoToml)[1]
const cargoKeywords = JSON.parse(`[${/^keywords = \[(.+)\]$/m.exec(cargoToml)[1]}]`)
const packageJson = JSON.parse(readFileSync('npm/package.json', 'utf8'))

test('npm and Cargo versions match', () => {
  assert.equal(packageJson.name, '@i-xor/lwc')
  assert.equal(packageJson.version, cargoVersion)
  assert.equal(packageJson.bin.lwc, 'bin/lwc.mjs')
})

test('npm, Cargo, and README discovery metadata stay aligned', () => {
  const description =
    'Agent-driven proactive memory CLI for AI agents — autonomously recall, maintain, and evolve persistent, source-grounded knowledge across sessions.'
  const compatibility =
    'Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity, GitHub Copilot in VS Code, Copilot CLI, Copilot for JetBrains, and pi'

  assert.equal(cargoDescription, description)
  assert.equal(packageJson.description, description)
  assert.deepEqual(packageJson.keywords.slice(0, cargoKeywords.length), cargoKeywords)

  for (const readme of ['README.md', 'npm/README.md']) {
    const contents = readFileSync(readme, 'utf8')
    assert.match(contents, /agent-driven proactive memory CLI for AI agents/i)
    const advertised = /\*\*Works with ([\s\S]*?)\.\*\*/.exec(contents)?.[1]
    assert.equal(advertised?.replace(/\s+/g, ' ').trim(), compatibility)
  }
})

test('both READMEs advertise the npm package and runtime contract', () => {
  for (const readme of ['README.md', 'README.zh-CN.md']) {
    const contents = readFileSync(readme, 'utf8')
    assert.match(contents, /npm-%40i--xor%2Flwc/)
    assert.match(contents, /alt="Node\.js 22 or newer"/)
    assert.match(contents, /node-%3E%3D22/)
    assert.match(contents, /npm install --global @i-xor\/lwc/)
  }
})

test('all release targets map exactly', () => {
  assert.equal(targetFor('darwin', 'x64'), 'x86_64-apple-darwin')
  assert.equal(targetFor('darwin', 'arm64'), 'aarch64-apple-darwin')
  assert.equal(targetFor('linux', 'x64'), 'x86_64-unknown-linux-gnu')
  assert.equal(targetFor('linux', 'arm64'), 'aarch64-unknown-linux-gnu')
  assert.equal(targetFor('win32', 'x64'), 'x86_64-pc-windows-msvc')
  assert.equal(targetFor('win32', 'arm64'), 'aarch64-pc-windows-msvc')
  assert.throws(() => targetFor('freebsd', 'x64'), /unsupported platform/)
})

test('archive and checksum selection are exact', () => {
  assert.equal(
    archiveFor('0.12.0', 'linux', 'x64'),
    'lwc-0.12.0-x86_64-unknown-linux-gnu.tar.gz',
  )
  assert.equal(
    archiveFor('0.12.0', 'win32', 'arm64'),
    'lwc-0.12.0-aarch64-pc-windows-msvc.zip',
  )
  const hash = 'a'.repeat(64)
  assert.equal(checksumFor(`${hash}  wanted.tar.gz\n`, 'wanted.tar.gz'), hash)
  assert.throws(() => checksumFor(`${hash}  other.tar.gz\n`, 'wanted.tar.gz'), /checksum/)
})

test('release downloads retry transient failures with a fresh timeout signal', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lwc-npm-download-'))
  const destination = join(directory, 'asset')
  const signals = []
  let attempts = 0
  const fetchImpl = async (_url, options) => {
    attempts += 1
    signals.push(options.signal)
    if (attempts === 1)
      return { ok: false, status: 503 }
    return { ok: true, arrayBuffer: async () => new TextEncoder().encode('asset') }
  }

  await download('https://github.test/asset', destination, fetchImpl, {
    retries: 2,
    retryDelayMs: 0,
    timeoutMs: 100,
  })

  assert.equal(attempts, 2)
  assert.equal(new Set(signals).size, 2)
  assert.ok(signals.every(signal => signal instanceof AbortSignal))
  assert.equal(await readFile(destination, 'utf8'), 'asset')
})

test('release downloads time out every attempt and stop at the retry limit', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lwc-npm-download-'))
  const destination = join(directory, 'asset')
  const signals = []
  const fetchImpl = async (_url, options) => {
    signals.push(options.signal)
    return new Promise((_resolve, reject) => {
      options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true })
    })
  }

  await assert.rejects(
    download('https://github.test/asset', destination, fetchImpl, {
      retries: 1,
      retryDelayMs: 0,
      timeoutMs: 5,
    }),
    (error) => {
      assert.match(error.message, /download failed after 2 attempts/)
      assert.equal(error.cause?.name, 'TimeoutError')
      return true
    },
  )
  assert.equal(signals.length, 2)
  assert.ok(signals.every(signal => signal.aborted))
})

test('release downloads preserve the final transport cause after finite retries', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lwc-npm-download-'))
  const destination = join(directory, 'asset')
  const causes = [new Error('reset-1'), new Error('reset-2'), new Error('reset-final')]
  let attempts = 0
  const fetchImpl = async () => {
    throw causes[attempts++]
  }

  await assert.rejects(
    download('https://github.test/asset', destination, fetchImpl, {
      retries: 2,
      retryDelayMs: 0,
      timeoutMs: 100,
    }),
    (error) => {
      assert.match(error.message, /download failed after 3 attempts/)
      assert.equal(error.cause, causes[2])
      return true
    },
  )
  assert.equal(attempts, 3)
})

test('installer applies timeout signals to both GitHub release downloads', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lwc-npm-install-'))
  const destinationRoot = join(directory, 'package')
  const tarCommand = join(directory, 'fake-tar')
  const archive = new TextEncoder().encode('archive fixture')
  const archiveHash = createHash('sha256').update(archive).digest('hex')
  const requests = []
  await writeFile(
    tarCommand,
    `#!/bin/sh
stage="$4/lwc-9.8.7-x86_64-unknown-linux-gnu"
mkdir -p "$stage"
printf '#!/bin/sh\\nprintf "lwc 9.8.7\\n"\\n' > "$stage/lwc"
chmod 755 "$stage/lwc"
`,
  )
  await chmod(tarCommand, 0o755)
  const fetchImpl = async (url, options) => {
    requests.push({ url, signal: options.signal })
    const contents = url.endsWith('/SHA256SUMS')
      ? new TextEncoder().encode(`${archiveHash}  lwc-9.8.7-x86_64-unknown-linux-gnu.tar.gz\n`)
      : archive
    return { ok: true, arrayBuffer: async () => contents }
  }

  const installed = await install({
    arch: 'x64',
    destinationRoot,
    fetchImpl,
    platform: 'linux',
    tarCommand,
    version: '9.8.7',
  })

  assert.equal(requests.length, 2)
  assert.deepEqual(
    requests.map(request => request.url.split('/').at(-1)).sort(),
    ['SHA256SUMS', 'lwc-9.8.7-x86_64-unknown-linux-gnu.tar.gz'],
  )
  assert.ok(requests.every(request => request.signal instanceof AbortSignal))
  assert.match(await readFile(installed, 'utf8'), /lwc 9\.8\.7/)
})

test('npm package contains only declared files', () => {
  const packed = spawnSync('npm', ['pack', '--dry-run', '--json'], {
    cwd: 'npm',
    encoding: 'utf8',
  })
  assert.equal(packed.status, 0, packed.stderr)
  const files = JSON.parse(packed.stdout)[0].files.map(file => file.path).sort()
  assert.deepEqual(files, [
    'README.md',
    'bin/lwc.mjs',
    'install.mjs',
    'package.json',
  ])
})
