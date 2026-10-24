<?php
// S1 spec driver (R3): evaluates every line of cases.txt with Laravel's own
// Illuminate\Support classes, loaded at run time from refs/ (nothing copied),
// and prints json_encode of each result, one per line.
//   php driver.php <laravel-framework-src> <cases.txt>
declare(strict_types=1);

namespace Illuminate\Support\Traits {
    trait Macroable { protected static $macros = []; public static function hasMacro($name) { return false; } }
    trait Conditionable {
        public function when($value = null, ?callable $callback = null, ?callable $default = null) { return $value ? ($callback ? ($callback($this, $value) ?? $this) : $this) : ($default ? ($default($this, $value) ?? $this) : $this); }
        public function unless($value = null, ?callable $callback = null, ?callable $default = null) { return $this->when(!$value, $callback, $default); }
    }
}

namespace voku\helper {
    // identity: the cases use ASCII only
    class ASCII { public static function to_ascii(string $str, string $language = 'en', bool $remove_unsupported_chars = true, bool $replace_extra_symbols = false, bool $use_transliterate = false, ?bool $replace_single_chars_only = null) { return $str; } }
}

namespace {
    // PHP 8.5 functions Laravel 13 uses (polyfills)
    if (!function_exists('array_first')) { function array_first(array $a): mixed { return $a === [] ? null : $a[array_key_first($a)]; } }
    if (!enum_exists('SortDirection')) { eval('enum SortDirection { case Ascending; case Descending; }'); }
    if (!function_exists('array_last')) { function array_last(array $a): mixed { return $a === [] ? null : $a[array_key_last($a)]; } }
    [$self, $src, $cases] = $argv;
    spl_autoload_register(function (string $c) use ($src) {
        if (!str_starts_with($c, 'Illuminate\\')) { return; }
        $parts = explode('\\', substr($c, 11));
        foreach ([$src . '/Illuminate/' . implode('/', $parts) . '.php', $src . '/Illuminate/Collections/' . implode('/', array_slice($parts, 1)) . '.php'] as $f) {
            if (is_file($f)) { require $f; return; }
        }
        if ($parts[0] === 'Contracts') {
            // contracts are not in the sparse checkout: empty interfaces suffice
            eval('namespace Illuminate\\' . implode('\\', array_slice($parts, 0, -1)) . '; interface ' . end($parts) . ' {}');
        }
    });
    require_once $src . '/Illuminate/Collections/helpers.php';
    require_once $src . '/Illuminate/Collections/functions.php';
    class_alias(\Illuminate\Support\Str::class, 'Str');
    // the dialect has no references: Arr::set/forget return the new root array
    final class Arr
    {
        public static function set($array, $key, $value) { \Illuminate\Support\Arr::set($array, $key, $value); return $array; }
        public static function forget($array, $keys) { \Illuminate\Support\Arr::forget($array, $keys); return $array; }
        public static function __callStatic($method, $args) { return \Illuminate\Support\Arr::$method(...$args); }
    }
    foreach (file($cases, FILE_IGNORE_NEW_LINES) as $line) {
        if (trim($line) === '') { continue; }
        $v = eval('return ' . $line . ';');
        echo json_encode($v), "\n";
    }
}
