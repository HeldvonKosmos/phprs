-- header comment; with semicolon
DEFINE TABLE t SCHEMAFULL; -- trailing
// slash comment
DEFINE FIELD a ON t TYPE int;
# hash comment
DEFINE FIELD b ON t TYPE int;
/* block; comment */ DEFINE FIELD c ON t TYPE int;
DEFINE FIELD d ON t -- inline
  TYPE int;
