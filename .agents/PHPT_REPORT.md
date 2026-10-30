# PHPT_REPORT — php-src tests as probes for the dialect (R1)

Tool: `phprs/tools/phpt/run.py` (reads the `.phpt` files of the php-src test suite (PHP 8.4) at run time; the suite is fetched
separately and is not part of the repository, nothing is copied into the repo; individual results in `phprs/target/refs/phpt/results.json`). Per test: `--FILE--` with PHP 8.4 twice
(reference; unequal runs = not deterministic, excluded) and with grd (`GRD_JIT=0`), each in its
own temp directory with a 10 s and 2 GB limit.

Classification: **identical** (same output) · **both reject** (PHP reports a parse/compile error, grd
rejects as well) · **dialect-induced** (GRD code in grd's output, or an exception where PHP prints a warning:
DIALECT §5) · **gap** (everything else, with reason) · **excluded** (not runnable as a plain script:
INI, ENV, SAPI sections; PHP not deterministic or timeout).

Reading: most php-src tests check edge cases, warnings and error texts; "identical" is therefore
low and not a measure for everyday code. What matters are (1) crashes and timeouts (correctness, phase 1), (2) "PHP rejects,
grd accepts" (missing compile checks), (3) differing output in core semantics. Missing builtins are
only added as needed (R1, driven by the requirements of a private reference implementation that is not part of this repository); filesystem and streams are GRD010 (no filesystem in the dialect).

## Run 2026-10-25 after the compile and link checks (commit 63de5f0)

"PHP rejects, grd accepts": **513 → 237**; "both reject": 454 → 730; identical unchanged 1110; no
crashes. New (one commit each with a test against PHP's messages, `crates/vm/tests/suite/inheritance.rs`, 114 cases):
inheritance checks at link time (af352cf, ffbf5f5: signature compatibility, static, visibility,
abstract methods, properties, final constants, `#[\Override]`), declarations and magic methods
(7e1097d), types and `return` (1ce52cb), namespace position, reserved names, kinds, trait conflicts (a235b62),
class modifiers, `switch`, promoted parameters (63de5f0). Found along the way: the prelude declared
`IteratorAggregate::getIterator(): Iterator` instead of `Traversable`.

The remaining 237 are a long tail (1–4 tests each): attribute targets, `#[AllowDynamicProperties]`,
`new` in constants, `list()` rules, `finally` jumps, typed constants (`const int X`), property hooks,
qualified scalar types (`\int`), and others.

State: phprs 63de5f0, php-src php-8.4.21, PHP 8.4.21; 11502 tests, of which 10172 runnable.

| | Tests | Share of runnable |
|---|---:|---:|
| identical | 1110 | 10.9 % |
| both reject (compile error) | 730 | 7.2 % |
| dialect-induced | 4691 | 46.1 % |
| gap | 3641 | 35.8 % |
| excluded | 1330 | – |

### Per directory

| Directory | runnable | identical | both reject | dialect-induced | gap | excluded |
|---|---:|---:|---:|---:|---:|---:|
| Zend/tests | 4703 | 652 | 714 | 1796 | 1541 | 262 |
| ext/standard/tests | 3046 | 235 | 11 | 2007 | 793 | 703 |
| ext/spl/tests | 717 | 10 | 3 | 323 | 381 | 49 |
| ext/date/tests | 522 | 88 | 1 | 153 | 280 | 164 |
| ext/reflection/tests | 454 | 59 | 0 | 134 | 261 | 18 |
| ext/mbstring/tests | 336 | 17 | 0 | 122 | 197 | 82 |
| ext/pcre/tests | 151 | 9 | 1 | 81 | 60 | 14 |
| ext/filter/tests | 87 | 13 | 0 | 42 | 32 | 26 |
| ext/json/tests | 77 | 24 | 0 | 22 | 31 | 11 |
| ext/hash/tests | 79 | 3 | 0 | 11 | 65 | 1 |

### Dialect-induced by reason

| Reason | Tests |
|---|---:|
| GRD004 | 1794 |
| GRD010 | 609 |
| GRD006 | 521 |
| GRD001 | 456 |
| GRD007 | 218 |
| GRD017 | 195 |
| exception instead of warning (§5) | 178 |
| GRD021 | 133 |
| GRD015 | 122 |
| GRD002 | 82 |
| GRD020 | 71 |
| GRD012 | 66 |
| GRD013 | 58 |
| GRD014 | 51 |
| GRD005 | 42 |
| GRD003 | 38 |
| GRD011 | 20 |
| GRD008 | 18 |
| GRD009 | 9 |
| GRD016 | 5 |

### Gaps by reason

| Reason | Tests |
|---|---:|
| different output | 1892 |
| missing function | 968 |
| missing class | 544 |
| PHP rejects, grd accepts | 237 |

### Excluded by reason

| Reason | Tests |
|---|---:|
| section INI | 1005 |
| PHP not deterministic | 246 |
| section ENV | 25 |
| section GET | 14 |
| section CGI | 14 |
| section ARGS | 10 |
| section POST | 4 |
| PHP timeout | 4 |
| section POST_RAW | 3 |
| section COOKIE | 3 |
| section STDIN | 1 |
| no --FILE-- | 1 |

### Missing functions (40 most frequent)

`fopen` (125), `error_reporting` (41), `file_put_contents` (26), `mkdir` (20), `pack` (19), `mb_ereg_replace` (18), `mb_regex_encoding` (17), `debug_print_backtrace` (16), `stream_wrapper_register` (15), `hash_init` (15), `touch` (13), `stream_context_create` (13), `crypt` (12), `get_class_vars` (11), `mb_substitute_character` (11), `mb_detect_encoding` (9), `mb_ereg_search_init` (9), `stream_socket_server` (9), `class_alias` (8), `timezone_open` (8), `gethostbyname` (8), `foo\class_alias` (7), `date_parse` (7), `tmpfile` (7), `mb_stripos` (7), `str_getcsv` (7), `putenv` (6), `get_called_class` (6), `zend_test_zend_ini_parse_quantity` (6), `mb_decode_mimeheader` (6), `mb_strcut` (6), `mb_ereg` (6), `mb_regex_set_options` (6), `mb_strstr` (6), `file_exists` (6), `chdir` (6), `basename` (6), `func_get_args` (5), `dirname` (5), `addcslashes` (5)

### Missing classes (40 most frequent)

`ArrayObject` (66), `Fiber` (49), `SplFixedArray` (46), `SplFileObject` (39), `SplDoublyLinkedList` (35), `DatePeriod` (31), `SplFileInfo` (21), `SplObjectStorage` (20), `ReflectionExtension` (18), `SplTempFileObject` (15), `SplPriorityQueue` (13), `Serializable` (11), `WeakMap` (9), `WeakReference` (8), `ReflectionGenerator` (8), `SplMaxHeap` (8), `IteratorIterator` (7), `AppendIterator` (7), `ZendTestUnitEnum` (6), `SensitiveParameterValue` (6), `DirectoryIterator` (6), `_ZendTestClass` (5), `EmptyIterator` (5), `Reflection` (5), `SplStack` (5), `SplMinHeap` (5), `SplHeap` (5), `Deprecated` (4), `MultipleIterator` (4), `RecursiveArrayIterator` (4), `SplQueue` (4), `GlobIterator` (3), `RecursiveIteratorIterator` (3), `RecursiveDirectoryIterator` (3), `RegexIterator` (3), `FileSystemIterator` (3), `FilesystemIterator` (3), `Foo\Foo` (2), `bar` (2), `LongCastableNoOperations` (2)

## Run 2026-10-25 after the crash fixes (commit e84781f)

**No more crashes and no more timeouts** (before: 22 aborts through stack overflow, 14 Rust panics, 7 timeouts).
Fixed, one commit each with a probe against the PHP output: self-referencing class constants (876c6a7), limit for
recursion through native code (d505a67), guards of the magic property methods and `isset()` without `__get` (246c88c),
cycles in `var_dump`/`print_r`/`json_encode` (ed5979e), four panics: reflection of native classes, iterator slot in the
generator, `intval`/`base_convert` base (2b55aca), `range()` (581a0e4), `wordwrap()` and the `strcmp` family (95e8ee6),
DateTime arithmetic across daylight-saving time (61560eb), `serialize` with `r:N` (c3a35fd), `http_build_query` and
`array_merge` limit (e84781f).

State: phprs e84781f, php-src php-8.4.21, PHP 8.4.21; 11502 tests, of which 10169 runnable.

| | Tests | Share of runnable |
|---|---:|---:|
| identical | 1110 | 10.9 % |
| both reject (compile error) | 454 | 4.5 % |
| dialect-induced | 4693 | 46.2 % |
| gap | 3912 | 38.5 % |
| excluded | 1333 | – |

### Per directory

| Directory | runnable | identical | both reject | dialect-induced | gap | excluded |
|---|---:|---:|---:|---:|---:|---:|
| Zend/tests | 4703 | 652 | 440 | 1799 | 1812 | 262 |
| ext/standard/tests | 3045 | 235 | 11 | 2008 | 791 | 704 |
| ext/spl/tests | 717 | 10 | 3 | 323 | 381 | 49 |
| ext/date/tests | 520 | 88 | 0 | 151 | 281 | 166 |
| ext/reflection/tests | 454 | 59 | 0 | 134 | 261 | 18 |
| ext/mbstring/tests | 336 | 17 | 0 | 122 | 197 | 82 |
| ext/pcre/tests | 151 | 9 | 0 | 81 | 61 | 14 |
| ext/filter/tests | 87 | 13 | 0 | 42 | 32 | 26 |
| ext/json/tests | 77 | 24 | 0 | 22 | 31 | 11 |
| ext/hash/tests | 79 | 3 | 0 | 11 | 65 | 1 |

### Dialect-induced by reason

| Reason | Tests |
|---|---:|
| GRD004 | 1792 |
| GRD010 | 609 |
| GRD006 | 521 |
| GRD001 | 456 |
| GRD007 | 218 |
| GRD017 | 198 |
| exception instead of warning (§5) | 178 |
| GRD021 | 134 |
| GRD015 | 122 |
| GRD002 | 82 |
| GRD020 | 71 |
| GRD012 | 66 |
| GRD013 | 58 |
| GRD014 | 51 |
| GRD005 | 42 |
| GRD003 | 38 |
| GRD011 | 20 |
| GRD008 | 18 |
| GRD009 | 9 |
| GRD016 | 5 |

### Gaps by reason

| Reason | Tests |
|---|---:|
| different output | 1889 |
| missing function | 966 |
| missing class | 544 |
| PHP rejects, grd accepts | 513 |

### Excluded by reason

| Reason | Tests |
|---|---:|
| section INI | 1005 |
| PHP not deterministic | 249 |
| section ENV | 25 |
| section GET | 14 |
| section CGI | 14 |
| section ARGS | 10 |
| section POST | 4 |
| PHP timeout | 4 |
| section POST_RAW | 3 |
| section COOKIE | 3 |
| section STDIN | 1 |
| no --FILE-- | 1 |

### Missing functions (40 most frequent)

`fopen` (125), `error_reporting` (41), `file_put_contents` (26), `mkdir` (20), `pack` (19), `mb_ereg_replace` (18), `mb_regex_encoding` (17), `debug_print_backtrace` (16), `stream_wrapper_register` (15), `hash_init` (15), `touch` (13), `stream_context_create` (13), `crypt` (12), `get_class_vars` (11), `mb_substitute_character` (11), `mb_detect_encoding` (9), `mb_ereg_search_init` (9), `stream_socket_server` (9), `class_alias` (8), `timezone_open` (8), `gethostbyname` (8), `foo\class_alias` (7), `date_parse` (7), `tmpfile` (7), `mb_stripos` (7), `str_getcsv` (7), `putenv` (6), `get_called_class` (6), `zend_test_zend_ini_parse_quantity` (6), `mb_decode_mimeheader` (6), `mb_strcut` (6), `mb_ereg` (6), `mb_regex_set_options` (6), `mb_strstr` (6), `file_exists` (6), `chdir` (6), `basename` (6), `func_get_args` (5), `dirname` (5), `addcslashes` (5)

### Missing classes (40 most frequent)

`ArrayObject` (66), `Fiber` (49), `SplFixedArray` (46), `SplFileObject` (39), `SplDoublyLinkedList` (35), `DatePeriod` (31), `SplFileInfo` (21), `SplObjectStorage` (20), `ReflectionExtension` (18), `SplTempFileObject` (15), `SplPriorityQueue` (13), `Serializable` (11), `WeakMap` (9), `WeakReference` (8), `ReflectionGenerator` (8), `SplMaxHeap` (8), `IteratorIterator` (7), `AppendIterator` (7), `ZendTestUnitEnum` (6), `SensitiveParameterValue` (6), `DirectoryIterator` (6), `_ZendTestClass` (5), `EmptyIterator` (5), `Reflection` (5), `SplStack` (5), `SplMinHeap` (5), `SplHeap` (5), `Deprecated` (4), `MultipleIterator` (4), `RecursiveArrayIterator` (4), `SplQueue` (4), `GlobIterator` (3), `RecursiveIteratorIterator` (3), `RecursiveDirectoryIterator` (3), `RegexIterator` (3), `FileSystemIterator` (3), `FilesystemIterator` (3), `Foo\Foo` (2), `bar` (2), `LongCastableNoOperations` (2)

## Run 2026-10-25 (before the crash fixes)

State: phprs 876c6a7, php-src php-8.4.21, PHP 8.4.21; 11502 tests, of which 10168 runnable.

| | Tests | Share of runnable |
|---|---:|---:|
| identical | 1065 | 10.5 % |
| both reject (compile error) | 454 | 4.5 % |
| dialect-induced | 4692 | 46.1 % |
| gap | 3957 | 38.9 % |
| excluded | 1334 | – |

### Per directory

| Directory | runnable | identical | both reject | dialect-induced | gap | excluded |
|---|---:|---:|---:|---:|---:|---:|
| Zend/tests | 4703 | 646 | 440 | 1797 | 1820 | 262 |
| ext/standard/tests | 3044 | 206 | 11 | 2007 | 820 | 705 |
| ext/spl/tests | 716 | 10 | 3 | 323 | 380 | 50 |
| ext/date/tests | 521 | 81 | 0 | 153 | 287 | 165 |
| ext/reflection/tests | 454 | 59 | 0 | 134 | 261 | 18 |
| ext/mbstring/tests | 336 | 17 | 0 | 122 | 197 | 82 |
| ext/pcre/tests | 151 | 9 | 0 | 81 | 61 | 14 |
| ext/filter/tests | 87 | 13 | 0 | 42 | 32 | 26 |
| ext/json/tests | 77 | 21 | 0 | 22 | 34 | 11 |
| ext/hash/tests | 79 | 3 | 0 | 11 | 65 | 1 |

### Dialect-induced by reason

| Reason | Tests |
|---|---:|
| GRD004 | 1793 |
| GRD010 | 609 |
| GRD006 | 521 |
| GRD001 | 456 |
| GRD007 | 218 |
| GRD017 | 198 |
| exception instead of warning (§5) | 176 |
| GRD021 | 134 |
| GRD015 | 122 |
| GRD002 | 82 |
| GRD020 | 71 |
| GRD012 | 66 |
| GRD013 | 58 |
| GRD014 | 51 |
| GRD005 | 42 |
| GRD003 | 38 |
| GRD011 | 20 |
| GRD008 | 18 |
| GRD009 | 9 |
| GRD016 | 5 |

### Gaps by reason

| Reason | Tests |
|---|---:|
| different output | 1892 |
| missing function | 966 |
| missing class | 543 |
| PHP rejects, grd accepts | 513 |
| grd crash (exit -6) | 22 |
| grd crash (exit 101) | 14 |
| grd timeout | 7 |

### Excluded by reason

| Reason | Tests |
|---|---:|
| section INI | 1005 |
| PHP not deterministic | 251 |
| section ENV | 25 |
| section GET | 14 |
| section CGI | 14 |
| section ARGS | 10 |
| section POST | 4 |
| section POST_RAW | 3 |
| section COOKIE | 3 |
| PHP timeout | 3 |
| section STDIN | 1 |
| no --FILE-- | 1 |

### Missing functions (40 most frequent)

`fopen` (125), `error_reporting` (41), `file_put_contents` (26), `mkdir` (20), `pack` (19), `mb_ereg_replace` (18), `mb_regex_encoding` (17), `debug_print_backtrace` (16), `stream_wrapper_register` (15), `hash_init` (15), `touch` (13), `stream_context_create` (13), `crypt` (12), `get_class_vars` (11), `mb_substitute_character` (11), `mb_detect_encoding` (9), `mb_ereg_search_init` (9), `stream_socket_server` (9), `class_alias` (8), `timezone_open` (8), `gethostbyname` (8), `foo\class_alias` (7), `date_parse` (7), `tmpfile` (7), `mb_stripos` (7), `str_getcsv` (7), `putenv` (6), `get_called_class` (6), `zend_test_zend_ini_parse_quantity` (6), `mb_decode_mimeheader` (6), `mb_strcut` (6), `mb_ereg` (6), `mb_regex_set_options` (6), `mb_strstr` (6), `file_exists` (6), `chdir` (6), `basename` (6), `func_get_args` (5), `dirname` (5), `addcslashes` (5)

### Missing classes (40 most frequent)

`ArrayObject` (66), `Fiber` (49), `SplFixedArray` (46), `SplFileObject` (39), `SplDoublyLinkedList` (35), `DatePeriod` (31), `SplFileInfo` (21), `SplObjectStorage` (20), `ReflectionExtension` (18), `SplTempFileObject` (15), `SplPriorityQueue` (13), `Serializable` (11), `WeakMap` (9), `WeakReference` (8), `ReflectionGenerator` (8), `SplMaxHeap` (8), `IteratorIterator` (7), `AppendIterator` (7), `ZendTestUnitEnum` (6), `SensitiveParameterValue` (6), `_ZendTestClass` (5), `EmptyIterator` (5), `Reflection` (5), `DirectoryIterator` (5), `SplStack` (5), `SplMinHeap` (5), `SplHeap` (5), `Deprecated` (4), `MultipleIterator` (4), `RecursiveArrayIterator` (4), `SplQueue` (4), `GlobIterator` (3), `RecursiveIteratorIterator` (3), `RecursiveDirectoryIterator` (3), `RegexIterator` (3), `FileSystemIterator` (3), `FilesystemIterator` (3), `Foo\Foo` (2), `bar` (2), `LongCastableNoOperations` (2)
