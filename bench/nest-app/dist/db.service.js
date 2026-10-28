var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};
import { Injectable } from '@nestjs/common';
import Database from 'better-sqlite3';
let DbService = class DbService {
    db;
    constructor() {
        this.db = new Database(process.env.BENCH_SQLITE_DATABASE ?? 'bench.sqlite');
        this.db.pragma('journal_mode = WAL');
        this.db.pragma('synchronous = FULL');
        this.db.pragma('busy_timeout = 5000');
    }
    select(sql, params = []) {
        return this.db.prepare(sql).all(...params);
    }
    run(sql, params = []) {
        this.db.prepare(sql).run(...params);
    }
};
DbService = __decorate([
    Injectable(),
    __metadata("design:paramtypes", [])
], DbService);
export { DbService };
//# sourceMappingURL=db.service.js.map