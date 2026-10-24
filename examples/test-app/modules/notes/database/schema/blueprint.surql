-- Schema of the notes module (declarative; `opengrid task migrate_push` applies it).
DEFINE TABLE notes SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD title ON TABLE notes TYPE string ASSERT string::len($value) > 0;
DEFINE FIELD body ON TABLE notes TYPE option<string>;
DEFINE FIELD done ON TABLE notes TYPE bool DEFAULT false;
DEFINE FIELD priority ON TABLE notes TYPE int DEFAULT 3;
DEFINE FIELD created_at ON TABLE notes TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE notes TYPE option<datetime>;
DEFINE INDEX notes_done_index ON TABLE notes FIELDS done;
