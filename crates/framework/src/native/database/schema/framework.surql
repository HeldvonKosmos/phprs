-- Tables of the framework itself (R4e). First in the merge order, so a
-- module can extend them in its manually.surql but never loses them.
DEFINE TABLE IF NOT EXISTS _opengrid_sessions SCHEMALESS PERMISSIONS NONE;
