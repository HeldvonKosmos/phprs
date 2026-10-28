import { NestFactory } from '@nestjs/core'
import { FastifyAdapter } from '@nestjs/platform-fastify'
import { AppModule } from './app.module.js'

// BENCH3: Express (Nest's default) or, as a marked variant, Fastify
// (NEST_ADAPTER=fastify); warnings and errors only
async function bootstrap() {
  const logger = ['warn', 'error'] as const
  const app = process.env.NEST_ADAPTER === 'fastify' ? await NestFactory.create(AppModule, new FastifyAdapter(), { logger: [...logger] }) : await NestFactory.create(AppModule, { logger: [...logger] })
  await app.listen(Number(process.env.PORT ?? 3000), '127.0.0.1')
}
bootstrap()
