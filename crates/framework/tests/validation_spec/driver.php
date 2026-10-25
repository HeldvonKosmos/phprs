<?php
// R3: Laravel's own Validator as the specification of OpenGrid's validation.
// Reads cases.txt (rules JSON <TAB> data JSON per line) and prints one JSON
// line per case: {"errors": {...}} or {"valid": <validated data>}. Uses the
// vendor directory of the Laravel comparison application (LV1, setup.sh);
// nothing of Laravel is copied into the repository, the output is cases.out.
//   php driver.php [vendor-dir] < cases.txt > cases.out
$vendor = $argv[1] ?? __DIR__ . '/../../../../../bench/laravel-app/vendor';
require $vendor . '/autoload.php';

use Illuminate\Translation\ArrayLoader;
use Illuminate\Translation\Translator;
use Illuminate\Validation\Factory;

$loader = new ArrayLoader();
$loader->addMessages('en', 'validation', require $vendor . '/laravel/framework/src/Illuminate/Translation/lang/en/validation.php');
$factory = new Factory(new Translator($loader, 'en'));

while (($line = fgets(STDIN)) !== false) {
    $line = rtrim($line, "\n");
    if ($line === '' || $line[0] === '#') {
        continue;
    }
    [$rules, $data] = explode("\t", $line, 2);
    $v = $factory->make(json_decode($data, true), json_decode($rules, true));
    echo json_encode($v->fails() ? ['errors' => $v->errors()->toArray()] : ['valid' => $v->validated()], JSON_UNESCAPED_SLASHES), "\n";
}
