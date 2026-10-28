/*
|--------------------------------------------------------------------------
| Routes of the benchmark app (BENCH3)
|--------------------------------------------------------------------------
|
| The same routes, statements and answers as the Laravel app's
| BenchSqlController/BenchLogicController and OpenGrid's benchsql module
| (bench/framework/laravel/overlay, examples/test-app/modules/benchsql).
|
*/

import router from '@adonisjs/core/services/router'

const BenchSqlController = () => import('#controllers/bench_sql_controller')
const BenchLogicController = () => import('#controllers/bench_logic_controller')
const BenchToken = () => import('#middleware/bench_token_middleware')

router.get('/health', () => ({ status: 'ok' }))

router
  .group(() => {
    router.get('/health', [BenchSqlController, 'health'])
    router.get('/sqlite', [BenchSqlController, 'sqlite'])
    router.post('/login', [BenchSqlController, 'login'])
  })
  .prefix('/api/bench/sql')

router
  .group(() => {
    router.get('', [BenchSqlController, 'index'])
    router.post('', [BenchSqlController, 'store'])
    router.get('/:id', [BenchSqlController, 'show'])
  })
  .prefix('/api/bench/sql/notes')
  .use(async (ctx, next) => {
    const { default: Middleware } = await BenchToken()
    return new Middleware().handle(ctx, next)
  })

router
  .group(() => {
    router.get('/hydrate', [BenchLogicController, 'hydrate'])
    router.get('/collect', [BenchLogicController, 'collect'])
    router.post('/validate', [BenchLogicController, 'validate'])
    router.get('/json', [BenchLogicController, 'json'])
  })
  .prefix('/api/bench/logic')
  .use(async (ctx, next) => {
    const { default: Middleware } = await BenchToken()
    return new Middleware().handle(ctx, next)
  })
