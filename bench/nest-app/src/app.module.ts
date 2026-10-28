import { Controller, Get, Module } from '@nestjs/common'
import { BenchLogicController } from './bench-logic.controller.js'
import { BenchSqlController } from './bench-sql.controller.js'
import { BenchTokenGuard } from './bench-token.guard.js'
import { DbService } from './db.service.js'

@Controller()
class HealthController {
  @Get('health')
  health() {
    return { status: 'ok' }
  }
}

@Module({
  controllers: [HealthController, BenchSqlController, BenchLogicController],
  providers: [DbService, BenchTokenGuard],
})
export class AppModule {}
