$f = Join-Path (Split-Path $PSScriptRoot -Parent) 'crates\smu-hal-win\src\wasapi.rs'
$t = [IO.File]::ReadAllText($f)
# Damaged shape (lost open paren): `ident(vtbl!(o, T).m)(args))`
#   -> needs `ident((vtbl!(o, T).m)(args))`
# Fine shapes: statement-start `(vtbl!(o, T).m)(args);` (paren inserted by
# wrap step, balanced) and already-doubled `((vtbl!`.
$pat = '(?<=[A-Za-z0-9_])\((vtbl!\([^,]+, [A-Za-z0-9]+\)\.[a-z_0-9]+\)\()'
$n = [regex]::Matches($t, $pat).Count
$t2 = [regex]::Replace($t, $pat, '(($1')
# second pass: paren followed by newline (multi-line arg position)
$pat2 = '(?<=\()\s*\n(?<ind>[ \t]*)(vtbl!\([^,]+, [A-Za-z0-9]+\)\.)'
$m2 = [regex]::Matches($t2, $pat2)
$n2 = $m2.Count
$t3 = [regex]::Replace($t2, $pat2, "`$ind(`$2")
[IO.File]::WriteAllText($f, $t3)
Write-Output "inline-fixed=$n multiline-fixed=$n2 len=$($t3.Length)"
