// Production start of the benchmark app (BENCH3, on Bun): the built app
// (`node ace build` -> build/) in N worker processes through node:cluster,
// as AdonisJS documents for multi-core production use; the primary only
// forks and restarts nothing (a dying worker ends the run).
import cluster from 'node:cluster'
import { fileURLToPath } from 'node:url'

const workers = Number(process.env.WORKERS ?? 4)
cluster.setupPrimary({ exec: fileURLToPath(new URL('./build/bin/server.js', import.meta.url)), cwd: fileURLToPath(new URL('./build/', import.meta.url)) })
for (let i = 0; i < workers; i++) cluster.fork()
cluster.on('exit', (worker, code) => {
  console.error(`worker ${worker.process.pid} exited with ${code}`)
  process.exit(code || 1)
})
