<?php
// S1/S1b spec driver (R3): evaluates every line of cases.txt with Laravel's own
// Illuminate\Support classes from the vendor directory of the Laravel
// comparison application (LV1: Laravel 13.38 with its real dependencies,
// e.g. doctrine/inflector for plural, voku/portable-ascii) and prints
// json_encode of each result, one per line. Nothing of Laravel is copied.
//   php driver_vendor.php <vendor-dir> <cases.txt>
declare(strict_types=1);

// PHP 8.5 functions and enum Laravel 13 uses (polyfills for PHP 8.4)
if (!function_exists('array_first')) { function array_first(array $a): mixed { return $a === [] ? null : $a[array_key_first($a)]; } }
if (!function_exists('array_last')) { function array_last(array $a): mixed { return $a === [] ? null : $a[array_key_last($a)]; } }
if (!enum_exists('SortDirection')) { eval('enum SortDirection { case Ascending; case Descending; }'); }

[$self, $vendor, $cases] = $argv;
require $vendor . '/autoload.php';
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
