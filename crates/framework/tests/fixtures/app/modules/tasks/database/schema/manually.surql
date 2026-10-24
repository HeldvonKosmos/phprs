-- Hand edits (as from Surrealist); merged after every blueprint.surql.
DEFINE FIELD IF NOT EXISTS notes ON TABLE tasks TYPE option<string>;
DEFINE INDEX IF NOT EXISTS tasks_title_unique ON TABLE tasks FIELDS title UNIQUE;
