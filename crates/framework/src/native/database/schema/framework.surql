-- Tables of the framework itself (R4e). First in the merge order, so a
-- module can extend them in its manually.surql but never loses them.
-- Plain DEFINE (no IF NOT EXISTS): the merge reads statements like the
-- platform's PHP classes, which take `IF` for the table name (R4b2).
DEFINE TABLE _opengrid_sessions SCHEMALESS PERMISSIONS NONE;
