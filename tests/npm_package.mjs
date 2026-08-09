import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

import { archiveFor, checksumFor, targetFor } from '../npm/install.mjs'

const cargoVersion = /^version = "([^"]+)"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1]
const packageJson = JSON.parse(readFileSync('npm/package.json', 'utf8'))

test('npm and Cargo versions match', () => {
  assert.equal(packageJson.name, '@i-xor/lwc')
  assert.equal(packageJson.version, cargoVersion)
  assert.equal(packageJson.bin.lwc, 'bin/lwc.mjs')
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
