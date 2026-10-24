-- Fixture schema of the `tasks` module (generated form, as written by Schema::sql).
DEFINE TABLE IF NOT EXISTS tasks SCHEMALESS;
DEFINE FIELD IF NOT EXISTS title ON TABLE tasks TYPE string;
DEFINE FIELD IF NOT EXISTS done ON TABLE tasks TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS priority ON TABLE tasks TYPE option<int>;
DEFINE FIELD IF NOT EXISTS due_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE tasks TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS deleted_at ON TABLE tasks TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS tasks_title_index ON TABLE tasks FIELDS title;

DEFINE TABLE IF NOT EXISTS comments SCHEMALESS;
DEFINE FIELD IF NOT EXISTS body ON TABLE comments TYPE string;
DEFINE FIELD IF NOT EXISTS hours ON TABLE comments TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS task_id ON TABLE comments TYPE record<tasks>;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE comments TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE comments TYPE option<datetime>;

DEFINE TABLE IF NOT EXISTS measurements SCHEMALESS;
DEFINE FIELD IF NOT EXISTS label ON TABLE measurements TYPE string;
DEFINE FIELD IF NOT EXISTS amount ON TABLE measurements TYPE decimal;
DEFINE FIELD IF NOT EXISTS took ON TABLE measurements TYPE duration;
DEFINE FIELD IF NOT EXISTS tag ON TABLE measurements TYPE uuid;
DEFINE FIELD IF NOT EXISTS blob ON TABLE measurements TYPE bytes;
DEFINE FIELD IF NOT EXISTS where ON TABLE measurements TYPE geometry<point>;
DEFINE FIELD IF NOT EXISTS at ON TABLE measurements TYPE datetime;
DEFINE FIELD IF NOT EXISTS task_id ON TABLE measurements TYPE record<tasks>;
DEFINE FIELD IF NOT EXISTS note ON TABLE measurements TYPE option<string>;
DEFINE FIELD IF NOT EXISTS count ON TABLE measurements TYPE option<int>;
DEFINE FIELD IF NOT EXISTS ratio ON TABLE measurements TYPE option<float>;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE measurements TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE measurements TYPE option<datetime>;
