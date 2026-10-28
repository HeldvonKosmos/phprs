import { CanActivate, ExecutionContext } from '@nestjs/common';
import { DbService } from './db.service.js';
export declare class BenchTokenGuard implements CanActivate {
    private readonly db;
    constructor(db: DbService);
    canActivate(context: ExecutionContext): boolean;
}
