DEFINE TABLE t SCHEMAFULL;
DEFINE FIELD a ON t VALUE 'x;y' ASSERT $value != 'it''s; ok';
DEFINE FIELD b ON t VALUE "q;\"r" ;
DEFINE FIELD `my;field` ON t TYPE string;
DEFINE FIELD c ON t VALUE 'a -- not a comment';
