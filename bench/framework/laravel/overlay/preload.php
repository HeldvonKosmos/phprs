<?php

/*
 * Measurement tool (BENCH3, Jonas 28.10.): opcache.preload for the
 * benchmark. Every class of the Composer classmap is compiled into opcache
 * when a FrankenPHP worker starts, so no request compiles a file. Files that
 * do not compile on their own (platform-specific stubs, polyfills for other
 * PHP versions) are skipped quietly.
 */
$classmap = require __DIR__.'/vendor/composer/autoload_classmap.php';
foreach (array_unique($classmap) as $file) {
    if (str_contains($file, '/vendor/composer/') || str_contains($file, '/vendor/bin/')) {
        continue;
    }
    @opcache_compile_file($file);
}
