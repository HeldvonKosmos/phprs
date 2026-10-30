<?php
// Golden driver for R4a: runs the platform's schema classes (loaded at run
// time from refs/platform/schema-tooling, never copied into this repo) and
// prints SurqlGenerator's output.
//   php driver.php <tooling-dir> files <file>...    merge the files in order
//   php driver.php <tooling-dir> project <root>      MigratePush's file order
declare(strict_types=1);

[$self, $tool, $mode] = $argv + [null, null, null];
spl_autoload_register(function (string $class) use ($tool): void {
    $prefix = 'App\\Database\\Migration\\';
    if (str_starts_with($class, $prefix)) {
        require $tool . '/Migration/' . str_replace('\\', '/', substr($class, strlen($prefix))) . '.php';
    }
});

if ($mode === 'project') {
    $root = $argv[3];
    $files = [];
    if (file_exists("$root/database/sources/main.surql")) {
        $files[] = "$root/database/sources/main.surql";
    }
    foreach (glob("$root/vendor/*/*/database/schema/blueprint.surql") ?: [] as $f) {
        $files[] = $f;
    }
    foreach (glob("$root/vendor/*/*/database/schema/manually.surql") ?: [] as $f) {
        $files[] = $f;
    }
} else {
    $files = array_slice($argv, 3);
}

$schema = App\Database\Migration\SurqlSchema::empty();
foreach ($files as $f) {
    $schema->merge(App\Database\Migration\SurqlFileParser::parse(file_get_contents($f)));
}
echo App\Database\Migration\SurqlGenerator::generate($schema);
