-- Users of the API (TP5), written by hand (schema.surql, D10).
DEFINE TABLE users SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD name ON TABLE users TYPE string;
DEFINE FIELD email ON TABLE users TYPE string;
DEFINE FIELD password ON TABLE users TYPE string;
DEFINE FIELD created_at ON TABLE users TYPE option<datetime>;
DEFINE FIELD updated_at ON TABLE users TYPE option<datetime>;
DEFINE INDEX users_email_unique ON TABLE users FIELDS email UNIQUE;
