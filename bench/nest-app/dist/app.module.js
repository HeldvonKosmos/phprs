var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};
import { Controller, Get, Module } from '@nestjs/common';
import { BenchLogicController } from './bench-logic.controller.js';
import { BenchSqlController } from './bench-sql.controller.js';
import { BenchTokenGuard } from './bench-token.guard.js';
import { DbService } from './db.service.js';
let HealthController = class HealthController {
    health() {
        return { status: 'ok' };
    }
};
__decorate([
    Get('health'),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", []),
    __metadata("design:returntype", void 0)
], HealthController.prototype, "health", null);
HealthController = __decorate([
    Controller()
], HealthController);
let AppModule = class AppModule {
};
AppModule = __decorate([
    Module({
        controllers: [HealthController, BenchSqlController, BenchLogicController],
        providers: [DbService, BenchTokenGuard],
    })
], AppModule);
export { AppModule };
//# sourceMappingURL=app.module.js.map