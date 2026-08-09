import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmod, copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const packageRoot = dirname(fileURLToPath(import.meta.url))
const packageJson = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))

export function targetFor(platform, arch) {
  const target = {
    'darwin:x64': 'x86_64-apple-darwin',
    'darwin:arm64': 'aarch64-apple-darwin',
    'linux:x64': 'x86_64-unknown-linux-gnu',
    'linux:arm64': 'aarch64-unknown-linux-gnu',
    'win32:x64': 'x86_64-pc-windows-msvc',
    'win32:arm64': 'aarch64-pc-windows-msvc',
  }[`${platform}:${arch}`]
  if (!target)
    throw new Error(`unsupported platform: ${platform}/${arch}`)
  return target
}

export function archiveFor(version, platform, arch) {
  const extension = platform === 'win32' ? 'zip' : 'tar.gz'
  return `lwc-${version}-${targetFor(platform, arch)}.${extension}`
}

export function checksumFor(contents, archive) {
  for (const line of contents.split(/\r?\n/)) {
    const match = /^([0-9a-fA-F]{64})[ \t]+\*?(\S+)$/.exec(line)
    if (match?.[2] === archive)
      return match[1].toLowerCase()
  }
  throw new Error(`checksum not found for ${archive}`)
}

async function download(url, destination, fetchImpl) {
  const response = await fetchImpl(url)
  if (!response.ok)
    throw new Error(`download failed (${response.status}): ${url}`)
  await writeFile(destination, new Uint8Array(await response.arrayBuffer()))
}

export async function install({
  platform = process.platform,
  arch = process.arch,
  version = packageJson.version,
  fetchImpl = fetch,
  tarCommand = 'tar',
  destinationRoot = packageRoot,
} = {}) {
  const archive = archiveFor(version, platform, arch)
  const stageName = archive.replace(/\.(?:tar\.gz|zip)$/, '')
  const release = `https://github.com/JanYork/llm-wiki-cli/releases/download/v${version}`
  const work = await mkdtemp(join(tmpdir(), 'lwc-npm-'))
  try {
    const archivePath = join(work, archive)
    const sumsPath = join(work, 'SHA256SUMS')
    await Promise.all([
      download(`${release}/${archive}`, archivePath, fetchImpl),
      download(`${release}/SHA256SUMS`, sumsPath, fetchImpl),
    ])

    const expected = checksumFor(await readFile(sumsPath, 'utf8'), archive)
    const actual = createHash('sha256').update(await readFile(archivePath)).digest('hex')
    if (actual !== expected)
      throw new Error(`checksum verification failed for ${archive}`)

    const extracted = spawnSync(tarCommand, ['-xf', archivePath, '-C', work], {
      encoding: 'utf8',
    })
    if (extracted.error)
      throw new Error(`tar is required to install @i-xor/lwc: ${extracted.error.message}`)
    if (extracted.status !== 0)
      throw new Error(`cannot extract ${archive}: ${extracted.stderr.trim()}`)

    const binaryName = platform === 'win32' ? 'lwc.exe' : 'lwc'
    const source = join(work, stageName, binaryName)
    if (platform !== 'win32')
      await chmod(source, 0o755)
    const verified = spawnSync(source, ['--version'], { encoding: 'utf8' })
    if (verified.status !== 0 || verified.stdout.trim() !== `lwc ${version}`)
      throw new Error(`downloaded binary failed version check for lwc ${version}`)

    const vendor = join(destinationRoot, 'vendor')
    const destination = join(vendor, binaryName)
    const staged = join(vendor, `.${binaryName}.${process.pid}`)
    await mkdir(vendor, { recursive: true })
    await copyFile(source, staged)
    if (platform !== 'win32')
      await chmod(staged, 0o755)
    await rm(destination, { force: true })
    await rename(staged, destination)
    return destination
  }
  finally {
    await rm(work, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  install().catch((error) => {
    console.error(`@i-xor/lwc install failed: ${error.message}`)
    process.exitCode = 1
  })
}
