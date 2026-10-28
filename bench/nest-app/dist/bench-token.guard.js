var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};
import { createHash } from 'node:crypto';
import { HttpException, Injectable } from '@nestjs/common';
import { DbService } from './db.service.js';
let BenchTokenGuard = class BenchTokenGuard {
    db;
    constructor(db) {
        this.db = db;
    }
    canActivate(context) {
        const request = context.switchToHttp().getRequest();
        const header = request.headers['authorization'] ?? '';
        const token = header.startsWith('Bearer ') ? header.slice(7) : '';
        if (token === '')
            throw new HttpException({ message: 'Unauthenticated.' }, 401);
        const rows = this.db.select('SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1', [createHash('sha256').update(token).digest('hex')]);
        if (rows.length === 0)
            throw new HttpException({ message: 'Unauthenticated.' }, 401);
        const row = rows[0];
        const now = Math.floor(Date.now() / 1000);
        if (now - row.last_used_at >= 1)
            this.db.run('UPDATE tokens SET last_used_at = ? WHERE id = ?', [now, row.token_id]);
        request.benchUser = { id: row.id, name: row.name, email: row.email };
        return true;
    }
};
BenchTokenGuard = __decorate([
    Injectable(),
    __metadata("design:paramtypes", [DbService])
], BenchTokenGuard);
export { BenchTokenGuard };
//# sourceMappingURL=bench-token.guard.js.map