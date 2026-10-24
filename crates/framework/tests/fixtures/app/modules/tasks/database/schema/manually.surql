-- Hand edits (as from Surrealist); merged after every blueprint.surql.
DEFINE FIELD notes ON TABLE tasks TYPE option<string>;
DEFINE INDEX tasks_title_unique ON TABLE tasks FIELDS title UNIQUE;
