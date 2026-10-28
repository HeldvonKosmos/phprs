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
import { Body, Controller, Get, HttpCode, HttpException, Post, Query, Req, UseGuards, ValidationPipe } from '@nestjs/common';
import { Type } from 'class-transformer';
import { ArrayNotEmpty, IsArray, IsInt, IsOptional, IsString, Max, MaxLength, Min, ValidateNested } from 'class-validator';
import { BenchTokenGuard } from './bench-token.guard.js';
import { DbService } from './db.service.js';
class UserView {
    id;
    name;
    email;
    constructor(id, name, email) {
        this.id = id;
        this.name = name;
        this.email = email;
    }
    toArray() {
        return { id: this.id, name: this.name, email: this.email };
    }
}
class NoteView {
    id;
    title;
    body;
    done;
    priority;
    createdAt;
    user;
    constructor(id, title, body, done, priority, createdAt, user) {
        this.id = id;
        this.title = title;
        this.body = body;
        this.done = done;
        this.priority = priority;
        this.createdAt = createdAt;
        this.user = user;
    }
    static fromRow(r) {
        return new NoteView(r.id, r.title, r.body, r.done === 1, r.priority, r.created_at, new UserView(r.user_id, r.user_name, r.user_email));
    }
    toArray() {
        return { id: this.id, title: this.title, body: this.body, done: this.done, priority: this.priority, created_at: this.createdAt, user: this.user.toArray() };
    }
}
class ItemDto {
    title;
    priority;
    tags;
}
__decorate([
    IsString(),
    MaxLength(120),
    __metadata("design:type", String)
], ItemDto.prototype, "title", void 0);
__decorate([
    IsInt(),
    Min(1),
    Max(5),
    __metadata("design:type", Number)
], ItemDto.prototype, "priority", void 0);
__decorate([
    IsOptional(),
    IsArray(),
    IsString({ each: true }),
    MaxLength(20, { each: true }),
    __metadata("design:type", Array)
], ItemDto.prototype, "tags", void 0);
class ItemsDto {
    items;
}
__decorate([
    IsArray(),
    ArrayNotEmpty(),
    ValidateNested({ each: true }),
    Type(() => ItemDto),
    __metadata("design:type", Array)
], ItemsDto.prototype, "items", void 0);
const ORDER = ['items', 'title', 'priority', 'tags', 'tag'];
const missing = (v) => v === undefined || v === null || v === '';
function laravelErrors(errors) {
    const found = [];
    const add = (rank, path, msg) => {
        const key = path.join('.');
        found.push([ORDER.indexOf(rank), path.filter((p) => typeof p === 'number'), key, msg(key)]);
    };
    const walk = (es, path) => {
        for (const e of es) {
            const prop = /^\d+$/.test(e.property) ? Number(e.property) : e.property;
            const here = [...path, prop];
            const c = e.constraints ?? {};
            if (e.children?.length)
                walk(e.children, here);
            if (!Object.keys(c).length)
                continue;
            if (prop === 'items')
                add('items', here, (k) => (missing(e.value) || c.arrayNotEmpty ? `The ${k} field is required.` : `The ${k} field must be an array.`));
            else if (prop === 'title')
                add('title', here, (k) => (missing(e.value) ? `The ${k} field is required.` : c.isString ? `The ${k} field must be a string.` : `The ${k} field must not be greater than 120 characters.`));
            else if (prop === 'priority')
                add('priority', here, (k) => (missing(e.value) ? `The ${k} field is required.` : c.isInt ? `The ${k} field must be an integer.` : `The ${k} field must be between 1 and 5.`));
            else if (prop === 'tags') {
                if (!Array.isArray(e.value))
                    add('tags', here, (k) => `The ${k} field must be an array.`);
                else
                    e.value.forEach((v, j) => {
                        if (typeof v !== 'string')
                            add('tag', [...here, j], (k) => `The ${k} field must be a string.`);
                        else if (v.length > 20)
                            add('tag', [...here, j], (k) => `The ${k} field must not be greater than 20 characters.`);
                    });
            }
        }
    };
    walk(errors, []);
    found.sort((a, b) => a[0] - b[0] || a[1].map((x, i) => x - (b[1][i] ?? -1)).find((d) => d !== 0) || 0);
    const out = {};
    for (const [, , key, msg] of found)
        (out[key] ??= []).push(msg);
    const all = Object.values(out).flat();
    const more = all.length - 1;
    return new HttpException({ message: all[0] + (more > 0 ? ` (and ${more} more ${more === 1 ? 'error' : 'errors'})` : ''), errors: out }, 422);
}
const itemsPipe = new ValidationPipe({ transform: true, exceptionFactory: laravelErrors });
let BenchLogicController = class BenchLogicController {
    db;
    constructor(db) {
        this.db = db;
    }
    n(q) {
        const v = Number.parseInt(q.n ?? '1', 10);
        return Math.min(1000, Math.max(0, Number.isNaN(v) ? 0 : v));
    }
    rows(req, n) {
        return this.db.select('SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?', [req.benchUser.id, n]);
    }
    hydrate(req, q) {
        return { data: this.rows(req, this.n(q)).map((r) => NoteView.fromRow(r).toArray()) };
    }
    collect(req, q) {
        const mapped = this.rows(req, this.n(q))
            .map((r) => ({ title: r.title, priority: r.priority, done: r.done === 1, chars: r.title.length + String(r.body ?? '').length }))
            .filter((r) => !r.done);
        const groups = Map.groupBy(mapped, (r) => r.priority);
        const out = {};
        for (const key of [...groups.keys()].sort((a, b) => a - b)) {
            const g = groups.get(key);
            const first = [...g].sort((a, b) => (a.title < b.title ? -1 : a.title > b.title ? 1 : 0))[0];
            out[key] = { count: g.length, chars: g.reduce((s, r) => s + r.chars, 0), first: first.title };
        }
        return { groups: out };
    }
    validate(data) {
        return { valid: data.items.length };
    }
    json(q) {
        const n = this.n(q);
        const items = [];
        for (let i = 0; i < n; i++)
            items.push({ id: i, name: 'item ' + i, tags: ['a', 'b', 'c'], meta: { created: '2026-10-27', score: i * 1.5, flags: { x: true, y: false } } });
        return { count: n, items };
    }
};
__decorate([
    Get('hydrate'),
    __param(0, Req()),
    __param(1, Query()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object, Object]),
    __metadata("design:returntype", void 0)
], BenchLogicController.prototype, "hydrate", null);
__decorate([
    Get('collect'),
    __param(0, Req()),
    __param(1, Query()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object, Object]),
    __metadata("design:returntype", void 0)
], BenchLogicController.prototype, "collect", null);
__decorate([
    Post('validate'),
    HttpCode(200),
    __param(0, Body(itemsPipe)),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [ItemsDto]),
    __metadata("design:returntype", void 0)
], BenchLogicController.prototype, "validate", null);
__decorate([
    Get('json'),
    __param(0, Query()),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Object]),
    __metadata("design:returntype", void 0)
], BenchLogicController.prototype, "json", null);
BenchLogicController = __decorate([
    Controller('api/bench/logic'),
    UseGuards(BenchTokenGuard),
    __metadata("design:paramtypes", [DbService])
], BenchLogicController);
export { BenchLogicController };
//# sourceMappingURL=bench-logic.controller.js.map