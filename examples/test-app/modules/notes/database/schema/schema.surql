-- Schema of the notes module, written by hand (schema.surql, D10); `opengrid task migrate_push` applies it.
DEFINE TABLE notes SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD user ON TABLE notes TYPE record<users>;
DEFINE FIELD title ON TABLE notes TYPE string ASSERT string::len($value) > 0;
DEFINE FIELD body ON TABLE notes TYPE option<string>;
DEFINE FIELD done ON TABLE notes TYPE bool DEFAULT false;
DEFINE FIELD priority ON TABLE notes TYPE int DEFAULT 3;
DEFINE FIELD created_at ON TABLE notes TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE notes TYPE option<datetime>;
DEFINE FIELD deleted_at ON TABLE notes TYPE option<datetime>;
DEFINE INDEX notes_user_index ON TABLE notes FIELDS user;
DEFINE INDEX notes_done_index ON TABLE notes FIELDS done;
-- DBP1: the list query (owner, optional done filter, soft delete, sort by
-- priority) is answered from the index: the page without sorting all hits,
-- the total by counting index entries
DEFINE INDEX notes_user_done_deleted_priority ON TABLE notes FIELDS user, done, deleted_at, priority;
DEFINE INDEX notes_user_deleted_priority ON TABLE notes FIELDS user, deleted_at, priority;
