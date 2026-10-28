var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};
var __param = (this && this.__param) || function (paramIndex, decorator) {
    return function (target, key) { decorator(target, key, paramIndex); }
};
import { createHash, randomBytes } from 'node:crypto';
import { Body, Controller, Get, HttpCode, HttpException, Param, Post, Query, Req, UseGuards } from '@nestjs/common';
import bcrypt from 'bcrypt';
import { BenchTokenGuard } from './bench-token.guard.js';
import { DbService } from './db.service.js';
const now = () => new Date().toISOString().slice(0, 19).replace('T', ' ');
const note = (r) => ({ id: r.id, title: r.title, body: r.body, done: r.done === 1, priority: r.priority, created_at: r.created_at, updated_at: r.updated_at });
let BenchSqlController = class BenchSqlController {
    db;
    constructor(db) {
        this.db = db;
    }
    health() {
        return { status: 'ok' };
    }
    async login(body) {
        const { email, password } = body ?? {};
        if (typeof email !== 'string' || typeof password !== 'string')
            throw new HttpException({ message: 'The email and password fields are required.' }, 422);
        const rows = this.db.select('SELECT id, password FROM users WHERE email = ? LIMIT 1', [email]);
        if (rows.length === 0 || !(await bcrypt.compare(password, rows[0].password)))
            throw new HttpException({ message: 'Invalid credentials.' }, 401);
        const token = randomBytes(32).toString('hex');
        this.db.run('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)', [rows[0].id, createHash('sha256').update(token).digest('hex'), Math.floor(Date.now() / 1000), now()]);
        return { token };
    }
    index(req, q) {
        const done = (q.done ?? 'false') === 'true' ? 1 : 0;
        const perPage = Number.parseInt(q.per_page ?? '15', 10) || 0;
        const page = Number.parseInt(q.page ?? '1', 10) || 0;
        const total = this.db.select('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [req.benchUser.id, done])[0].total;
        const rows = this.db.select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [req.benchUser.id, done, perPage, (page - 1) * perPage]);
        return { data: rows.map(note), total, per_page: perPage, current_page: page };
    }
    show(req, id) {
        const rows = this.db.select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [Number.parseInt(id, 10) || 0, req.benchUser.id]);
        if (rows.length === 0)
            throw new HttpException({ message: 'Not found.' }, 404);
        return { data: note(rows[0]) };
    }
    store(req, body) {
        const title = body?.title;
        if (typeof title !== 'string' || title === '')
            throw new HttpException({ message: 'The title field is required.' }, 422);
        const text = typeof body.body === 'string' ? body.body : null;
        const priority = Number.isInteger(body.priority ?? 3) ? (body.priority ?? 3) : 3;
        const t = now();
        const id = this.db.select('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [req.benchUser.id, title, text, priority, t, t])[0].id;
        return { data: { id, title, body: text, done: false, priority, created_at: t, updated_at: t } };
    }
};
__decorate([
    Get('health'),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", []),
    __metadata("design:returntype", void 0)
], BenchSqlController.prototype, "health", null);
__decorate([
    Post('login'),
    HttpCode(200),
    __param(0, Body()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object]),
    __metadata("design:returntype", Promise)
], BenchSqlController.prototype, "login", null);
__decorate([
    Get('notes'),
    UseGuards(BenchTokenGuard),
    __param(0, Req()),
    __param(1, Query()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object, Object]),
    __metadata("design:returntype", void 0)
], BenchSqlController.prototype, "index", null);
__decorate([
    Get('notes/:id'),
    UseGuards(BenchTokenGuard),
    __param(0, Req()),
    __param(1, Param('id')),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object, String]),
    __metadata("design:returntype", void 0)
], BenchSqlController.prototype, "show", null);
__decorate([
    Post('notes'),
    UseGuards(BenchTokenGuard),
    __param(0, Req()),
    __param(1, Body()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object, Object]),
    __metadata("design:returntype", void 0)
], BenchSqlController.prototype, "store", null);
BenchSqlController = __decorate([
    Controller('api/bench/sql'),
    __metadata("design:paramtypes", [DbService])
], BenchSqlController);
export { BenchSqlController };
//# sourceMappingURL=bench-sql.controller.js.map