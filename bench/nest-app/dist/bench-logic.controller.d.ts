import { DbService } from './db.service.js';
declare class ItemDto {
    title: string;
    priority: number;
    tags?: string[];
}
declare class ItemsDto {
    items: ItemDto[];
}
export declare class BenchLogicController {
    private readonly db;
    constructor(db: DbService);
    private n;
    private rows;
    hydrate(req: any, q: any): {
        data: {
            id: number;
            title: string;
            body: string | null;
            done: boolean;
            priority: number;
            created_at: string;
            user: {
                id: number;
                name: string;
                email: string;
            };
        }[];
    };
    collect(req: any, q: any): {
        groups: Record<number, any>;
    };
    validate(data: ItemsDto): {
        valid: number;
    };
    json(q: any): {
        count: number;
        items: {
            id: number;
            name: string;
            tags: string[];
            meta: {
                created: string;
                score: number;
                flags: {
                    x: boolean;
                    y: boolean;
                };
            };
        }[];
    };
}
export {};
