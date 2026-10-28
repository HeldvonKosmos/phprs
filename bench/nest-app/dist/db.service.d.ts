import Database from 'better-sqlite3';
export declare class DbService {
    readonly db: Database.Database;
    constructor();
    select(sql: string, params?: unknown[]): any[];
    run(sql: string, params?: unknown[]): void;
}
