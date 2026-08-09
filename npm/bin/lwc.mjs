#!/usr/bin/env node

import { spawn } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const binary = join(packageRoot, 'vendor', process.platform === 'win32' ? 'lwc.exe' : 'lwc')
const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' })

child.on('error', (error) => {
  console.error(`cannot start lwc: ${error.message}`)
  process.exitCode = 1
})
child.on('exit', (code, signal) => {
  if (signal)
    process.kill(process.pid, signal)
  else
    process.exitCode = code ?? 1
})
