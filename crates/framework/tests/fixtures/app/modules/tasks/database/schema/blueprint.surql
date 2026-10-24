-- Fixture schema of the `tasks` module (generated form, as written by Schema::sql).
DEFINE TABLE tasks SCHEMALESS;
DEFINE FIELD title ON TABLE tasks TYPE string;
DEFINE FIELD done ON TABLE tasks TYPE bool DEFAULT false;
DEFINE FIELD priority ON TABLE tasks TYPE option<int>;
DEFINE FIELD due_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD created_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD deleted_at ON TABLE tasks TYPE option<datetime>;
DEFINE INDEX tasks_title_index ON TABLE tasks FIELDS title;

DEFINE TABLE comments SCHEMALESS;
DEFINE FIELD body ON TABLE comments TYPE string;
DEFINE FIELD hours ON TABLE comments TYPE int DEFAULT 0;
DEFINE FIELD task_id ON TABLE comments TYPE record<tasks>;
DEFINE FIELD created_at ON TABLE comments TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE comments TYPE option<datetime>;

DEFINE TABLE measurements SCHEMALESS;
DEFINE FIELD label ON TABLE measurements TYPE string;
DEFINE FIELD amount ON TABLE measurements TYPE decimal;
DEFINE FIELD took ON TABLE measurements TYPE duration;
DEFINE FIELD tag ON TABLE measurements TYPE uuid;
DEFINE FIELD blob ON TABLE measurements TYPE bytes;
DEFINE FIELD where ON TABLE measurements TYPE geometry<point>;
DEFINE FIELD at ON TABLE measurements TYPE datetime;
DEFINE FIELD task_id ON TABLE measurements TYPE record<tasks>;
DEFINE FIELD note ON TABLE measurements TYPE option<string>;
DEFINE FIELD count ON TABLE measurements TYPE option<int>;
DEFINE FIELD ratio ON TABLE measurements TYPE option<float>;
DEFINE FIELD created_at ON TABLE measurements TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE measurements TYPE option<datetime>;
