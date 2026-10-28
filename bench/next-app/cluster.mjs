// Production start of the benchmark app (BENCH3): `next start` (after
// `next build`) in N worker processes through node:cluster; `next start`
// itself runs one process. The primary only forks and restarts nothing.
import cluster from 'node:cluster'
import { fileURLToPath } from 'node:url'

const workers = Number(process.env.WORKERS ?? 4)
cluster.setupPrimary({
  exec: fileURLToPath(new URL('./node_modules/next/dist/bin/next', import.meta.url)),
  args: ['start', '--hostname', '127.0.0.1', '--port', String(process.env.PORT ?? 3000)],
  cwd: fileURLToPath(new URL('./', import.meta.url)),
})
for (let i = 0; i < workers; i++) cluster.fork()
cluster.on('exit', (worker, code) => {
  console.error(`worker ${worker.process.pid} exited with ${code}`)
  process.exit(code || 1)
})
