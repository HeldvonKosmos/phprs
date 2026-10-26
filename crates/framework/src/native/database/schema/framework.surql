-- Tables of the framework itself (R4e). First in the merge order, so a
-- module can extend them in its manually.surql but never loses them.
-- Plain DEFINE (no IF NOT EXISTS): the merge reads statements like the
-- platform's PHP classes, which take `IF` for the table name (R4b2).
DEFINE TABLE _opengrid_sessions SCHEMALESS PERMISSIONS NONE;

-- Durable jobs (J4), as Laravel's database queue: waiting and running jobs,
-- and the ones that failed for good.
DEFINE TABLE jobs SCHEMALESS PERMISSIONS NONE;
DEFINE INDEX jobs_due ON TABLE jobs FIELDS reserved_at, available_at;
DEFINE TABLE failed_jobs SCHEMALESS PERMISSIONS NONE;
DEFINE INDEX failed_jobs_uuid ON TABLE failed_jobs FIELDS uuid UNIQUE;

-- API tokens (AU1): only the SHA-256 hash of a token is stored.
DEFINE TABLE personal_access_tokens SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD user ON TABLE personal_access_tokens TYPE record;
DEFINE FIELD name ON TABLE personal_access_tokens TYPE string DEFAULT 'api';
DEFINE FIELD token_hash ON TABLE personal_access_tokens TYPE string;
DEFINE FIELD created_at ON TABLE personal_access_tokens TYPE datetime;
DEFINE FIELD last_used_at ON TABLE personal_access_tokens TYPE option<datetime>;
DEFINE FIELD expires_at ON TABLE personal_access_tokens TYPE option<datetime>;
DEFINE INDEX personal_access_tokens_token_hash ON TABLE personal_access_tokens FIELDS token_hash UNIQUE;
