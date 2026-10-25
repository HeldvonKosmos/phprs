<?php
// c3_compute: work a JIT speeds up — loops, integer and float arithmetic,
// function calls, recursion, method calls and property access on objects,
// array indexing. Compared WITH JIT on both sides (PHP opcache JIT against
// the dialect with grd-jit); until grd-jit exists the dialect column is the
// interpreter baseline. Builtins are avoided in the hot paths on purpose.
// Same source for PHP and dialect (lines marked `// grd: …` are replaced by
// gen.sh). Output line 1: checksum; then phases in ms.

final class Vec
{
    public function __construct(public float $x, public float $y) {}
    public function add(Vec $o): Vec { return new Vec($this->x + $o->x, $this->y + $o->y); }
    public function scale(float $f): Vec { return new Vec($this->x * $f, $this->y * $f); }
    public function len2(): float { return $this->x * $this->x + $this->y * $this->y; }
}

final class Particle
{
    public function __construct(public Vec $pos, public Vec $vel, public int $hits = 0) {}
    public function step(float $dt): void
    {
        $this->pos = $this->pos->add($this->vel->scale($dt));
        if ($this->pos->x < 0.0 || $this->pos->x > 100.0) { $this->vel = new Vec(-$this->vel->x, $this->vel->y); $this->hits++; }
        if ($this->pos->y < 0.0 || $this->pos->y > 100.0) { $this->vel = new Vec($this->vel->x, -$this->vel->y); $this->hits++; }
    }
}

function fib(int $n): int { return $n < 2 ? $n : fib($n - 1) + fib($n - 2); }

function ackermann_small(int $m, int $n): int
{
    if ($m === 0) { return $n + 1; }
    if ($n === 0) { return ackermann_small($m - 1, 1); }
    return ackermann_small($m - 1, ackermann_small($m, $n - 1));
}

function collatz_len(int $n): int
{
    $steps = 0;
    while ($n !== 1) { $n = $n % 2 === 0 ? intdiv($n, 2) : 3 * $n + 1; $steps++; }
    return $steps;
}

function mandel(int $w, int $h, int $maxIter): int
{
    $inside = 0;
    for ($py = 0; $py < $h; $py++) {
        $ci = ($py / $h) * 2.0 - 1.0;
        for ($px = 0; $px < $w; $px++) {
            $cr = ($px / $w) * 3.0 - 2.0;
            $zr = 0.0; $zi = 0.0; $i = 0;
            while ($i < $maxIter && $zr * $zr + $zi * $zi < 4.0) {
                $t = $zr * $zr - $zi * $zi + $cr;
                $zi = 2.0 * $zr * $zi + $ci;
                $zr = $t;
                $i++;
            }
            if ($i === $maxIter) { $inside++; }
        }
    }
    return $inside;
}

$phases = ['int_loops' => 0, 'float_math' => 0, 'calls' => 0, 'recursion' => 0, 'objects' => 0, 'arrays' => 0];
$checksum = 0;
$t = fn () => hrtime(true);

for ($round = 1; $round <= 3; $round++) {
    // integer loops: sieve of Eratosthenes + Collatz
    $t0 = $t();
    $n = 300000;
    $sieve = array_fill(0, $n + 1, true);
    $primes = 0;
    for ($i = 2; $i <= $n; $i++) {
        if ($sieve[$i]) {
            $primes++;
            for ($j = $i * $i; $j <= $n; $j += $i) { $sieve[$j] = false; }
        }
    }
    $longest = 0;
    for ($k = 1; $k < 60000; $k++) { $l = collatz_len($k + $round); if ($l > $longest) { $longest = $l; } }
    $phases['int_loops'] += $t() - $t0;

    // float math
    $t0 = $t();
    $inside = mandel(160 + $round, 120, 200);
    $phases['float_math'] += $t() - $t0;

    // many small function calls
    $t0 = $t();
    $acc = 0;
    for ($i = 0; $i < 400000; $i++) { $acc = ($acc + max($i % 13, $round) * 3 - min($i % 7, 4)) % 1000003; }
    $phases['calls'] += $t() - $t0;

    // recursion
    $t0 = $t();
    $f = fib(24 + $round);
    $a = ackermann_small(2, 300 + $round);
    $phases['recursion'] += $t() - $t0;

    // objects: method calls, property reads/writes, allocation
    $t0 = $t();
    $ps = [];
    for ($i = 0; $i < 400; $i++) { $ps[] = new Particle(new Vec(($i * 7) % 100 + 0.5, ($i * 13) % 100 + 0.5), new Vec((($i % 9) - 4) * 1.5, (($i % 7) - 3) * 2.0)); }
    for ($s = 0; $s < 150; $s++) { foreach ($ps as $p) { $p->step(0.5); } }
    $hits = 0; $energy = 0.0;
    foreach ($ps as $p) { $hits += $p->hits; $energy += $p->vel->len2(); }
    $phases['objects'] += $t() - $t0;

    // array indexing: matrix multiplication on nested lists
    $t0 = $t();
    $size = 60;
    $ma = []; $mb = [];
    for ($i = 0; $i < $size; $i++) { for ($j = 0; $j < $size; $j++) { $ma[$i][$j] = ($i + $j + $round) % 10; $mb[$i][$j] = ($i * $j + $round) % 10; } }
    $mc = [];
    for ($i = 0; $i < $size; $i++) {
        for ($j = 0; $j < $size; $j++) {
            $sum = 0;
            for ($k = 0; $k < $size; $k++) { $sum += $ma[$i][$k] * $mb[$k][$j]; }
            $mc[$i][$j] = $sum;
        }
    }
    $trace = 0;
    for ($i = 0; $i < $size; $i++) { $trace += $mc[$i][$i]; }
    $phases['arrays'] += $t() - $t0;

    $checksum = ($checksum * 31 + $primes + $longest + $inside + $acc + $f + $a + $hits + (int) ($energy * 100) + $trace) % 1000000007;
}

echo 'checksum ', $checksum, "\n";
$total = 0;
foreach ($phases as $name => $ns) { $total += $ns; echo $name, ' ', round($ns / 1e6, 2), "\n"; }
echo 'total ', round($total / 1e6, 2), "\n";
