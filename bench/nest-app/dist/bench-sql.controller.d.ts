import { DbService } from './db.service.js';
export declare class BenchSqlController {
    private readonly db;
    constructor(db: DbService);
    health(): {
        status: string;
    };
    login(body: any): Promise<{
        token: string;
    }>;
    index(req: any, q: any): {
        data: {
            id: any;
            title: any;
            body: any;
            done: boolean;
            priority: any;
            created_at: any;
            updated_at: any;
        }[];
        total: any;
        per_page: number;
        current_page: number;
    };
    show(req: any, id: string): {
        data: {
            id: any;
            title: any;
            body: any;
            done: boolean;
            priority: any;
            created_at: any;
            updated_at: any;
        };
    };
    store(req: any, body: any): {
        data: {
            id: any;
            title: string;
            body: any;
            done: boolean;
            priority: any;
            created_at: string;
            updated_at: string;
        };
    };
}
