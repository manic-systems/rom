#!/usr/bin/env nu

# Generates the fixture directories under `rom/tests/fixtures`: each timed
# `build.log`, along with the `derivations.json`, `history.csv` and `cpu.csv`
# sidecars some of them need. Each `variants.toml` is written by hand and left
# alone.
#
# Every fixture is defined here, so retiming or extending one means editing
# this file and running it again, then blessing the snapshots with
# `ROM_BLESS=1 cargo test --test fixtures`.
#
# Builds and transfers only get a row once they have run for a second, so a
# checkpoint must land at least a second after the work it is meant to show.

const root = path self | path dirname | path dirname
const fixtures = $root | path join rom tests fixtures
const alphabet = "0123456789abcdfghijklmnpqrsvwxyz"
const cache = "https://cache.nixos.org"
const mib = 1048576

const words = [
  bash coreutils gcc glibc openssl zlib curl python3 perl git ncurses readline
  sqlite libffi xz gettext systemd dbus util-linux linux-pam kmod kbd nftables
  iproute2 openssh shadow sudo less man-db pipewire
]

# Writes every fixture, or only those named.
def main [...names: string] {
  let all = scope commands
    | where name starts-with "fixture "
    | get name
    | str replace "fixture " ""
  let names = if ($names | is-empty) { $all } else { $names }
  for name in $names {
    if $name not-in $all { error make { msg: $"unknown fixture ($name)" } }
    print --stderr $"generating ($name)"
    write-fixture $name (run-fixture $name)
  }
}

# Nushell cannot call a command by a computed name.
def run-fixture [name: string] {
  match $name {
    "big-graph" => { fixture big-graph }
    "deep-chain" => { fixture deep-chain }
    "wide-fanout" => { fixture wide-fanout }
    "grow-shrink" => { fixture grow-shrink }
    "shared-names" => { fixture shared-names }
    "planned-lifecycle" => { fixture planned-lifecycle }
    "sub-second-builds" => { fixture sub-second-builds }
    "phases-and-estimates" => { fixture phases-and-estimates }
    "cpu-load" => { fixture cpu-load }
    "failed-build" => { fixture failed-build }
    "dependency-failure" => { fixture dependency-failure }
    "multiple-failures" => { fixture multiple-failures }
    "packages-and-units" => { fixture packages-and-units }
    "deep-units" => { fixture deep-units }
    "ca-and-dynamic" => { fixture ca-and-dynamic }
    "downloads" => { fixture downloads }
    "unsized-download" => { fixture unsized-download }
    "mixed-transfers" => { fixture mixed-transfers }
    "substitution-heavy" => { fixture substitution-heavy }
    "source-fetches" => { fixture source-fetches }
    "remote-build" => { fixture remote-build }
    "remote-mix" => { fixture remote-mix }
    "chatty-build" => { fixture chatty-build }
    "hostile-logs" => { fixture hostile-logs }
    "passthrough" => { fixture passthrough }
    "unsupported-record" => { fixture unsupported-record }
    "long-names" => { fixture long-names }
    "odd-names" => { fixture odd-names }
    "overflow" => { fixture overflow }
    "empty-run" => { fixture empty-run }
    "instant-finish" => { fixture instant-finish }
    "interrupted" => { fixture interrupted }
    "long-run" => { fixture long-run }
  }
}

# Saves a fixture's log and sidecars, removing sidecars it no longer has.
def write-fixture [name: string, fixture: record] {
  let directory = $fixtures | path join $name
  mkdir $directory
  $fixture.events
    | flatten
    | sort-by ms
    | each {|event| $"($event.ms) ($event.line)" }
    | str join "\n"
    | $in + "\n"
    | save --raw --force ($directory | path join build.log)
  let drvs = $directory | path join derivations.json
  if ($fixture.drvs? | is-empty) {
    rm --force $drvs
  } else {
    $fixture.drvs | to json --indent 2 | $in + "\n" | save --raw --force $drvs
  }
  let history = $directory | path join history.csv
  if ($fixture.history? | is-empty) {
    rm --force $history
  } else {
    $fixture.history | to csv | save --raw --force $history
  }
  let cpu = $directory | path join cpu.csv
  if ($fixture.cpu? | is-empty) {
    rm --force $cpu
  } else {
    $fixture.cpu | to csv | save --raw --force $cpu
  }
}

# ── Store paths ─────────────────────────────────────────────────────────────

# A store hash derived from `seed`, so paths stay stable across runs.
def store-hash [seed: string] {
  $seed
    | hash sha256
    | split chars
    | first 32
    | each {|digit| let index = $digit | into int --radix 16; $alphabet | str substring $index..$index }
    | str join
}

# The derivation for `name`. Different seeds give same-named derivations.
def drv [name: string, seed?: string] {
  $"/nix/store/(store-hash ($seed | default $name))-($name).drv"
}

# The output of `drv $name $seed`, which the fixture resolver assumes.
def out [name: string, seed?: string] {
  $"/nix/store/(store-hash ($seed | default $name))-($name)"
}

# A deterministic number in `lo..hi` for `index`.
def pick [index: int, lo: int, hi: int] {
  $lo + ((($index + 1) * 2654435761) mod 4294967296) mod ($hi - $lo)
}

# A history row in ROM's build report format.
def report [name: string, seconds: int, host: string = "localhost"] {
  { hostname: $host, derivation_name: $name, utc_time: "2026-10-01T12:00:00Z", build_seconds: $seconds }
}

# ── Events ──────────────────────────────────────────────────────────────────

def nix [record: record] { $"@nix ($record | to json --raw)" }

def at [ms: int, line: string] { { ms: $ms, line: $line } }

def check [ms: int, name: string] { at $ms $"= ($name)" }

def start [ms: int, id: int, type: int, text: string, fields: list, parent: int = 0] {
  at $ms (nix { action: start, id: $id, level: 3, parent: $parent, text: $text, type: $type, fields: $fields })
}

def stop [ms: int, id: int] { at $ms (nix { action: stop, id: $id }) }

def result [ms: int, id: int, type: int, fields: list] {
  at $ms (nix { action: result, id: $id, type: $type, fields: $fields })
}

def msg [ms: int, level: int, text: string] {
  at $ms (nix { action: msg, level: $level, msg: $text })
}

def build [ms: int, id: int, drv: string, --host: string = "", --parent: int = 0] {
  let text = if $host == "" { $"building '($drv)'" } else { $"building '($drv)' on '($host)'" }
  start $ms $id 105 $text [$drv $host 1 1] $parent
}

def phase [ms: int, id: int, name: string] { result $ms $id 104 [$name] }

def log [ms: int, id: int, line: string] { result $ms $id 101 [$line] }

def progress [ms: int, id: int, done: int, expected: int, running: int = 1] {
  result $ms $id 105 [$done $expected $running 0]
}

# Nix's announcement of the derivations it will build.
def plan-builds [ms: int, drvs: list] {
  let count = $drvs | length
  let header = if $count == 1 { "this derivation will be built:" } else { $"these ($count) derivations will be built:" }
  [(msg $ms 3 $header)] ++ ($drvs | sort | each {|drv| msg $ms 3 $"  ($drv)" })
}

# Nix's announcement of the paths it will substitute.
def plan-fetches [ms: int, paths: list, download: string, unpacked: string] {
  let count = $paths | length
  let header = if $count == 1 {
    $"this path will be fetched \(($download) download, ($unpacked) unpacked\):"
  } else {
    $"these ($count) paths will be fetched \(($download) download, ($unpacked) unpacked\):"
  }
  [(msg $ms 3 $header)] ++ ($paths | sort | each {|path| msg $ms 3 $"  ($path)" })
}

def build-failed [ms: int, drv: string, code: int = 2] {
  msg $ms 0 $"error: builder for '($drv)' failed with exit code ($code)"
}

def deps-failed [ms: int, drv: string, count: int] {
  msg $ms 0 $"error: ($count) dependencies of derivation '($drv)' failed to build"
}

# A substitution the way Nix reports it: the substitution, the copy below it
# that carries progress, and the transfer below that. `stall` holds progress
# at that share of `size` until `resume`.
def substitution [
  id: int, path: string, size: int, from: int, to: int,
  --cache: string = $cache
  --parent: int = 0
  --stall: float
  --resume: int
] {
  let name = $path | path basename
  let steps = (($to - $from) // 400) | into int | [$in 1] | math max
  let progress_at = {|ms|
    let share = ($ms - $from) / ($to - $from)
    let share = if $stall == null { $share } else if $ms < $resume { [$share $stall] | math min } else { $share }
    ($size * $share) | math round | into int
  }
  let ticks = 1..$steps | each {|step| $from + ($to - $from) * $step // $steps }
    | where {|ms| $stall == null or $ms < ($from + ($resume - $from) * $stall) or $ms >= $resume }
  [
    (start $from $id 108 $"copying path '($path)' from '($cache)'" [$path $cache] $parent)
    (start $from ($id + 1) 100 $"copying path '($path)' from '($cache)'" [$path $cache "local"] $id)
    (start ($from + 20) ($id + 2) 101 $"downloading '($cache)/nar/($name).nar.xz'" [$"($cache)/nar/($name).nar.xz"] ($id + 1))
    ($ticks | each {|ms| progress $ms ($id + 1) (do $progress_at $ms) $size })
    (stop $to ($id + 2))
    (stop $to ($id + 1))
    (stop $to $id)
  ] | flatten
}

# A copy between this machine and `host`, in either direction.
def copy [
  id: int, path: string, size: int, from: int, to: int,
  --to-host: string
  --from-host: string
  --parent: int = 0
] {
  let ends = if $to_host != null {
    { source: "local", target: $to_host, text: $"copying path '($path)' to '($to_host)'" }
  } else {
    { source: $from_host, target: "local", text: $"copying path '($path)' from '($from_host)'" }
  }
  let steps = (($to - $from) // 400) | into int | [$in 1] | math max
  [
    (start $from $id 100 $ends.text [$path $ends.source $ends.target] $parent)
    (1..$steps | each {|step| progress ($from + ($to - $from) * $step // $steps) $id ($size * $step // $steps) $size })
    (stop $to $id)
  ] | flatten
}

# Schedules builds like Nix: each starts once its inputs are built and one
# of `slots` is free. Tasks are `{drv, inputs, ms}` with optional `host`,
# `phases` and `logs`, spread evenly over the build.
def schedule [tasks: list, --start: int = 0, --slots: int = 8, --first-id: int = 100] {
  mut time = $start
  mut built = []
  mut running = []
  mut pending = $tasks
  mut events = []
  mut id = $first_id
  while ($pending | is-not-empty) or ($running | is-not-empty) {
    let ready_built = $built
    let free = $slots - ($running | length)
    let starting = $pending
      | where {|task| $task.inputs | all {|input| $input in $ready_built } }
      | first ([$free 0] | math max)
    let started = $starting | get drv
    $pending = $pending | where drv not-in $started
    for entry in ($starting | enumerate) {
      let task = $entry.item
      let begin = $time + $entry.index * 15
      $events = $events ++ [(build $begin $id $task.drv --host ($task.host? | default ""))]
      let phases = $task.phases? | default []
      for p in ($phases | enumerate) {
        $events = $events ++ [(phase ($begin + 20 + $task.ms * $p.index // ($phases | length)) $id $p.item)]
      }
      let logs = $task.logs? | default []
      for l in ($logs | enumerate) {
        $events = $events ++ [(log ($begin + 40 + $task.ms * $l.index // ($logs | length)) $id $l.item)]
      }
      $running = $running ++ [{ drv: $task.drv, id: $id, end: ($begin + $task.ms) }]
      $id += 1
    }
    if ($running | is-empty) { error make { msg: "schedule: tasks wait on inputs that never build" } }
    let next = $running | get end | math min
    $time = $next
    let finished = $running | where end == $next
    for task in $finished { $events = $events ++ [(stop $task.end $task.id)] }
    $built = $built ++ ($finished | get drv)
    $running = $running | where end != $next
  }
  { events: $events, end: $time, next_id: $id }
}

# Sidecar entries for `{drv, inputs}` tasks.
def edges [tasks: list] {
  $tasks | reduce --fold {} {|task, acc| $acc | insert $task.drv { inputs: $task.inputs } }
}

# ── Graph size and shape ────────────────────────────────────────────────────

# A NixOS system: twelve groups of twelve leaves, at most 32 building at once.
def "fixture big-graph" [] {
  let groups = [etc system-path user-units system-units initrd firmware fonts man-pages locale udev-rules dbus-1 activation]
  let leaves = 0..143 | each {|k|
    let word = $words | get ($k mod ($words | length))
    let name = $"($word)-($k // ($words | length) + 1).($k mod 7).($k mod 5)"
    {
      drv: (drv $name)
      inputs: []
      ms: (pick $k 1200 6000)
      group: ($k // 12)
      phases: [unpackPhase configurePhase buildPhase installPhase]
    }
  }
  let group_tasks = $groups | enumerate | each {|g|
    { drv: (drv $g.item), inputs: ($leaves | where group == $g.index | get drv), ms: (pick ($g.index + 500) 1500 2500) }
  }
  let system = drv "nixos-system-rom-26.11.20261031.abcdef0"
  let tasks = $leaves ++ $group_tasks ++ [{ drv: $system, inputs: ($group_tasks | get drv), ms: 2000 }]
  let run = schedule $tasks --start 200 --slots 32
  {
    events: [
      (plan-builds 0 ($tasks | get drv))
      $run.events
      (check 1500 filling)
      (check 6000 saturated)
      (check 12000 draining)
      (check ($run.end - 1000) system)
    ]
    drvs: { derivations: (edges $tasks) }
  }
}

# A fourteen-level chain, built bottom up.
def "fixture deep-chain" [] {
  let levels = 0..13 | each {|level| drv $"python3.12-layer-($level)-with-a-rather-long-descriptive-name-1.($level)" }
  let tasks = $levels | enumerate | each {|level|
    { drv: $level.item, inputs: ($levels | skip ($level.index + 1) | first 1), ms: 1300 }
  }
  let run = schedule $tasks --start 100 --slots 4
  {
    events: [
      (plan-builds 0 $levels)
      $run.events
      (check 1500 deepest)
      (check 9600 halfway)
      (check ($run.end - 300) top)
    ]
    drvs: { derivations: (edges $tasks) }
  }
}

# Fifty inputs of one environment, finishing in no particular order.
def "fixture wide-fanout" [] {
  let tools = 0..49 | each {|k| { drv: (drv $"($words | get ($k mod ($words | length)))-tool-($k)"), inputs: [], ms: (pick $k 1200 9000) } }
  let shell = drv "devshell-env"
  let tasks = $tools ++ [{ drv: $shell, inputs: ($tools | get drv), ms: 1500 }]
  let run = schedule $tasks --start 100 --slots 50
  {
    events: [
      (plan-builds 0 ($tasks | get drv))
      $run.events
      (check 1500 all-running)
      (check 4000 thinning)
      (check 7000 stragglers)
    ]
    drvs: { derivations: (edges $tasks) }
  }
}

# Two builds, then forty, then one, then thirty: the graph's height jumps.
def "fixture grow-shrink" [] {
  let wave = {|name, count, inputs, lo, hi|
    0..($count - 1) | each {|k| { drv: (drv $"($name)-($k)"), inputs: $inputs, ms: (pick ($k + ($name | str length) * 100) $lo $hi) } }
  }
  let a = do $wave "small-a" 2 [] 3000 3100
  let b = do $wave "big-b" 40 ($a | get drv) 2500 3500
  let c = do $wave "single-c" 1 ($b | get drv) 2500 2600
  let d = do $wave "big-d" 30 ($c | get drv) 2000 3000
  let tasks = $a ++ $b ++ $c ++ $d ++ [{ drv: (drv "grow-shrink-root"), inputs: ($d | get drv), ms: 1200 }]
  let run = schedule $tasks --start 100 --slots 40
  {
    events: [
      (plan-builds 0 ($tasks | get drv))
      $run.events
      (check 1500 small)
      (check 4800 big)
      (check 8500 small-again)
      (check 11300 big-again)
    ]
    drvs: { derivations: (edges $tasks) }
  }
}

# Same-named derivations, which share no estimate.
def "fixture shared-names" [] {
  let sources = 1..6 | each {|k| drv "source" $"source-($k)" }
  let hooks = 1..3 | each {|k| drv "hook" $"hook-($k)" }
  let app = drv "app-1.0"
  let lib = drv "lib-2.0"
  let bundle = drv "bundle"
  let tasks = [
    ...($sources | enumerate | each {|s| { drv: $s.item, inputs: [], ms: (pick $s.index 2000 4000) } })
    ...($hooks | enumerate | each {|h| { drv: $h.item, inputs: [], ms: (pick ($h.index + 9) 1500 2500) } })
    { drv: $app, inputs: [...($sources | first 3) ($hooks | first)], ms: 3000 }
    { drv: $lib, inputs: [...($sources | skip 3) ...($hooks | skip 1)], ms: 2500 }
    { drv: $bundle, inputs: [$app $lib], ms: 1200 }
  ]
  let run = schedule $tasks --start 100 --slots 8
  {
    events: [
      (plan-builds 0 ($tasks | get drv))
      $run.events
      (check 1500 sources)
      (check 5500 consumers)
    ]
    drvs: { derivations: (edges $tasks) }
    history: [(report "source" 5) (report "hook" 2) (report "app-1.0" 6) (report "lib-2.0" 3)]
  }
}

# ── Build lifecycle ─────────────────────────────────────────────────────────

# Announced builds run one after another; one is never built.
def "fixture planned-lifecycle" [] {
  let dep = drv "dep-1.0"
  let lib = drv "lib-1.0"
  let app = drv "app-1.0"
  let docs = drv "docs-1.0"
  {
    events: [
      (plan-builds 0 [$dep $lib $app $docs])
      (check 800 planned)
      (build 300 10 $dep)
      (check 1500 first-running)
      (stop 1800 10)
      (build 1850 11 $lib)
      (check 3000 second-running)
      (stop 3400 11)
      (build 3450 12 $app)
      (check 4600 last-running)
      (stop 5000 12)
    ]
    drvs: { derivations: { $app: { inputs: [$lib] }, $lib: { inputs: [$dep] } } }
  }
}

# Builds shorter than a second among a few long ones.
def "fixture sub-second-builds" [] {
  let tasks = 0..43 | each {|k|
    let slow = $k mod 11 == 0
    { drv: (drv $"(if $slow { 'slow' } else { 'quick' })-($k)"), inputs: [], ms: (if $slow { pick $k 3000 6000 } else { pick $k 100 900 }) }
  }
  let run = schedule $tasks --start 100 --slots 8
  {
    events: [
      (plan-builds 0 ($tasks | get drv))
      $run.events
      (check 1500 early)
      (check 3000 middle)
      (check 5000 late)
    ]
    drvs: { derivations: (edges $tasks) }
  }
}

# Phase changes, a build within its estimate, one past it and one without.
def "fixture phases-and-estimates" [] {
  let estimated = drv "estimated-1.0"
  let overrun = drv "overrun-1.0"
  let phased = drv "phased-1.0"
  let fresh = drv "fresh-1.0"
  let phases = [unpackPhase patchPhase configurePhase buildPhase installPhase fixupPhase]
  {
    events: [
      (build 0 10 $estimated)
      (phase 50 10 buildPhase)
      (build 50 11 $overrun)
      (phase 100 11 checkPhase)
      (build 100 12 $phased)
      ($phases | enumerate | each {|p| phase (150 + $p.index * 800) 12 $p.item })
      (build 150 13 $fresh)
      (check 1500 starting)
      (check 2600 overrun)
      (stop 3000 10)
      (check 4500 late)
      (stop 4000 13)
      (stop 4900 12)
      (stop 5050 11)
    ]
    history: [
      (report "estimated-1.0" 4) (report "estimated-1.0" 4)
      (report "overrun-1.0" 2) (report "overrun-1.0" 2) (report "overrun-1.0" 2)
      (report "phased-1.0" 6)
    ]
  }
}

# Local builds whose CPU use earns each load label: steady, serial, idle
# and heavy. Labels need their use to last, up to a minute, and once the
# serial build settles the builds fit in the 8 cores fixtures assume.
def "fixture cpu-load" [] {
  let builds = [
    [name cores];
    [steady-1.0 2.0] [serial-1.0 2.0] [idle-1.0 0.0] [heavy-1.0 4.5]
  ] | insert drv {|build| drv $build.name }
  {
    events: [
      (plan-builds 0 ($builds | get drv))
      ($builds | enumerate | each {|build| build 200 (10 + $build.index) $build.item.drv })
      (check 5000 starting)
      (check 40000 idle)
      (check 70000 serial-and-heavy)
      ($builds | enumerate | each {|build| stop 71000 (10 + $build.index) })
    ]
    # The serial build starts out parallel, which its label waits out.
    cpu: (
      $builds | each {|build| { after_ms: 0, drv: $build.drv, cores: $build.cores } }
      | append { after_ms: 3000, drv: (drv serial-1.0), cores: 1.0 }
    )
  }
}

# A failing input nested under its consumer by activity, without metadata.
def "fixture failed-build" [] {
  let app = drv "app-1.0" "failed-build-app"
  let lib = drv "lib-1.0" "failed-build-lib"
  {
    events: [
      (build 0 10 $app)
      (build 50 11 $lib --parent 10)
      (phase 100 11 buildPhase)
      (log 150 11 "make: *** [Makefile:12: lib.o] Error 1")
      (check 1500 building)
      (build-failed 2000 $lib)
      (stop 2000 11)
      (deps-failed 2050 $app 1)
      (stop 2050 10)
      (check 2500 failed)
    ]
  }
}

# One input fails while its siblings keep building.
def "fixture dependency-failure" [] {
  let app = drv "app-1.0"
  let tool = drv "tool-1.0"
  let a = drv "lib-a-1.0"
  let b = drv "lib-b-1.0"
  let c = drv "lib-c-1.0"
  let errors = [
    "src/parse.c: In function 'parse_header':"
    "src/parse.c:118:9: error: implicit declaration of function 'strlcpy' [-Wimplicit-function-declaration]"
    "  118 |         strlcpy(header->name, line, sizeof header->name);"
    "      |         ^~~~~~~"
    "make[1]: *** [Makefile:41: src/parse.o] Error 1"
    "make: *** [Makefile:12: all] Error 2"
  ]
  {
    events: [
      (plan-builds 0 [$app $tool $a $b $c])
      (build 100 10 $a)
      (phase 150 10 buildPhase)
      (build 150 11 $b)
      (phase 200 11 buildPhase)
      (build 200 12 $c)
      (phase 250 12 buildPhase)
      (check 1500 building)
      (stop 2200 12)
      ($errors | enumerate | each {|e| log (2300 + $e.index * 50) 10 $e.item })
      (check 2700 about-to-fail)
      (build-failed 2800 $a)
      (stop 2800 10)
      (deps-failed 2850 $tool 1)
      (check 4000 failed)
      (stop 7000 11)
      (deps-failed 7050 $app 2)
    ]
    drvs: { derivations: { $app: { inputs: [$a $b $c $tool] }, $tool: { inputs: [$a] } } }
  }
}

# Three independent failures in Nix's different wordings, and one success.
def "fixture multiple-failures" [] {
  let source = drv "source-1.2.3"
  let slow = drv "slow-tests-1.0"
  let broken = drv "compiler-errors-2.0"
  let fine = drv "fine-1.0"
  let tail = 1..30 | each {|k| $"src/module_($k).rs:($k * 7):5: error[E0425]: cannot find value `item_($k)` in this scope" }
  let last = $tail | last 10 | each {|line| $"       > ($line)" } | str join "\n"
  {
    events: [
      (plan-builds 0 [$source $slow $broken $fine])
      (build 100 10 $source)
      (build 150 11 $slow)
      (phase 200 11 checkPhase)
      (build 200 12 $broken)
      (phase 250 12 buildPhase)
      (build 250 13 $fine)
      (check 1500 building)
      (msg 1800 0 $"error: hash mismatch in fixed-output derivation '($source)':\n         specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n            got:    sha256-1b4bqP0lTrLRqzYI1C2pd0mrYyq5Tv3OVfUbjXsTi3o=")
      (stop 1800 10)
      (check 2400 hash-mismatch)
      ($tail | enumerate | each {|t| log (2500 + $t.index * 20) 12 $t.item })
      (msg 3200 0 $"error: Cannot build '($broken)'.\n       Reason: builder failed with exit code 101.\n       Output paths:\n         (out 'compiler-errors-2.0')\n       Last 10 log lines:\n($last)")
      (stop 3200 12)
      (check 3800 compile-error)
      (stop 4000 13)
      (msg 5000 0 $"error: building of '($slow)' timed out after 5 seconds of silence")
      (stop 5000 11)
      (check 5600 timeout)
    ]
  }
}

# A dynamic derivation producer whose units appear once it has run.
def "fixture packages-and-units" [] {
  let producer = drv "hello.drv"
  let package = drv "hello"
  let wrapped = drv "hello-wrapped-1.0"
  let unit = {|name| drv $"hello-($name)" }
  let objects = [main.o util.o io.o cli.o] | each {|o| do $unit $o }
  let link = do $unit "link"
  let floating = { out: "" }
  {
    events: [
      (plan-builds 0 [$producer $wrapped])
      (build 300 10 $producer)
      (phase 350 10 configurePhase)
      (check 1500 producer)
      (stop 1800 10)
      (build 2000 11 ($objects | get 0))
      (build 2050 12 ($objects | get 2))
      (build 2100 13 ($objects | get 3))
      (check 3200 units)
      (stop 3300 13)
      (stop 3500 11)
      (stop 4000 12)
      (build 4100 14 $link)
      (check 4600 link)
      (stop 5300 14)
      (build 5400 15 $package)
      (stop 6200 15)
      (build 6300 16 $wrapped)
      (check 7400 consumer)
      (stop 7500 16)
    ]
    drvs: {
      derivations: (
        { $producer: { outputs: $floating }, $wrapped: { dynamic: [$producer] }, $package: { inputs: [$link], outputs: $floating }, $link: { inputs: $objects, outputs: $floating } }
        | merge ($objects | reduce --fold {} {|o, acc| $acc | insert $o { outputs: $floating } })
      )
      produced: { $producer: $package }
      realised: [($objects | get 1)]
    }
  }
}

# Units nested deep enough that their running leaves push the bars right, or
# drop them on a narrow terminal, to keep their names.
def "fixture deep-units" [] {
  let producer = drv "gh.drv"
  let package = drv "gh"
  let wrapped = drv "gh-wrapped-1.0"
  let chain = [
    cmd-root cmd-repo cmd-factory pkg-cmdutil context api internal-ghrepo
  ] | each {|name| drv $"github.com-cli-cli-v2-($name)" }
    | append (drv "github.com-cli-go-gh-v2-pkg-repository")
  let leaves = [
    github.com-cli-go-gh-v2-pkg-ssh github.com-cli-safeexec
    github.com-cli-cli-v2-pkg-iostreams
  ] | each {|name| drv $name }
  let floating = { out: "" }
  let inputs = $chain | enumerate | each {|unit|
    let below = if $unit.index + 1 < ($chain | length) {
      [($chain | get ($unit.index + 1))]
    } else {
      $leaves | first 2
    }
    # A shallower leaf beside the chain.
    let beside = if $unit.index == 4 { [($leaves | last)] } else { [] }
    { $unit.item: { inputs: ($below | append $beside), outputs: $floating } }
  }
  {
    events: [
      (plan-builds 0 [$producer $wrapped])
      (build 100 10 $producer)
      (stop 500 10)
      (build 600 11 ($leaves | get 0))
      (build 600 12 ($leaves | get 1))
      (build 600 13 ($leaves | get 2))
      (check 3000 deep)
      (stop 4000 11)
      (stop 4000 12)
      (stop 4000 13)
    ]
    drvs: {
      derivations: (
        { $producer: { outputs: $floating }, $wrapped: { dynamic: [$producer] }, $package: { inputs: [($chain | first)], outputs: $floating } }
        | merge ($inputs | reduce --fold {} {|unit, acc| $acc | merge $unit })
        | merge ($leaves | reduce --fold {} {|leaf, acc| $acc | insert $leaf { outputs: $floating } })
      )
      produced: { $producer: $package }
    }
  }
}

# Content-addressed derivations: resolution, a realised one Nix skips, and a
# dynamic producer an earlier run built.
def "fixture ca-and-dynamic" [] {
  let lib = drv "ca-lib-1.0"
  let app = drv "ca-app-1.0"
  let resolved = drv "ca-app-1.0" "ca-app-resolved"
  let cached = drv "ca-cached-1.0"
  let tool = drv "ca-tool-1.0"
  let generator = drv "gen.drv"
  let generated = drv "gen"
  let consumer = drv "uses-gen-1.0"
  let floating = { out: "" }
  {
    events: [
      (plan-builds 0 [$lib $app $cached $tool $consumer])
      (msg 50 1 $"warning: Ignoring dynamic derivation ($generator)^out while planning, because it is already built")
      (check 1000 planned)
      (build 200 10 $lib)
      (stop 2200 10)
      (start 2300 20 111 $"resolved derivation: '($app)' -> '($resolved)'" [$app $resolved])
      (build 2400 11 $resolved)
      (build 2500 12 $tool)
      (check 3500 resolved)
      (stop 4000 12)
      (stop 4200 11)
      (stop 4200 20)
      (build 4400 13 $generated)
      (stop 5600 13)
      (build 5700 14 $consumer)
      (check 6800 dynamic)
      (stop 6900 14)
    ]
    drvs: {
      derivations: {
        $lib: { outputs: $floating }
        $app: { inputs: [$lib], outputs: $floating }
        $resolved: { sources: [(out "ca-lib-1.0")], outputs: $floating }
        $cached: { outputs: $floating }
        $tool: { inputs: [$cached], outputs: $floating }
        $generator: { outputs: $floating }
        $generated: { outputs: $floating }
        $consumer: { dynamic: [$generator] }
      }
      produced: { $generator: $generated }
      realised: [$cached]
    }
  }
}

# ── Transfers ───────────────────────────────────────────────────────────────

# Concurrent substitutions: quick, large, stalled, from a second cache, and
# one whose size Nix never reports.
def "fixture downloads" [] {
  let paths = [openssl-3.0.15 zlib-1.3.1 glibc-2.40 llvm-19.1.7 helix-25.07 python3-3.13.7] | each {|name| out $name }
  let unsized = $paths | get 5
  {
    events: [
      (plan-fetches 0 $paths "130.2 MiB" "512.9 MiB")
      (substitution 10 ($paths | get 0) (8 * $mib) 300 3300)
      (substitution 20 ($paths | get 1) ($mib // 5) 350 900)
      (substitution 30 ($paths | get 2) (40 * $mib) 400 7400)
      (substitution 40 ($paths | get 3) (60 * $mib) 450 11000 --stall 0.3 --resume 9000)
      (substitution 50 ($paths | get 4) (12 * $mib) 600 5000 --cache "https://nix-community.cachix.org")
      (start 700 60 108 $"copying path '($unsized)' from '($cache)'" [$unsized $cache])
      (stop 4000 60)
      (check 1500 started)
      (check 4000 midway)
      (check 8000 stalled)
    ]
  }
}

# A substitution without progress, under a build that logs a styled and an
# over-width line. `pty_live.rs` replays it through the real CLI.
def "fixture unsized-download" [] {
  let demo = drv "demo" "unsized-demo"
  let openssl = out "openssl-3.0.15" "unsized-openssl"
  {
    events: [
      (build 0 10 $demo)
      (start 100 20 108 $"copying path '($openssl)' from '($cache)'" [$openssl $cache] 10)
      (log 1200 10 "\u{1b}[1;35mchecking styled output\u{1b}[0m")
      (log 1300 10 "configuring flags: --disable-static --disable-dependency-tracking --prefix=/nix/store/8vqx-example-imagemagick-7.1.2 --bindir=/nix/store/8vqx-example-imagemagick-7.1.2/bin --includedir=/nix/store/8vqx-example-imagemagick-7.1.2-dev/include")
      (check 1500 downloading)
      (stop 2500 20)
      (stop 2600 10)
    ]
  }
}

# Downloads and uploads at once: inputs to a remote builder, its result back,
# and a push to a second host.
def "fixture mixed-transfers" [] {
  let app = drv "app-1.0"
  let src = out "app-src"
  let vendor = out "vendor-deps"
  {
    events: [
      (plan-builds 0 [$app])
      (plan-fetches 0 [(out "rustc-1.89.0") (out "cargo-1.89.0")] "40.0 MiB" "160.0 MiB")
      (substitution 10 (out "rustc-1.89.0") (30 * $mib) 200 4200)
      (substitution 20 (out "cargo-1.89.0") (10 * $mib) 300 2800)
      (copy 30 $src (5 * $mib) 400 3400 --to-host "ssh-ng://chariot")
      (copy 31 $vendor (25 * $mib) 500 6000 --to-host "ssh-ng://chariot")
      (build 3500 40 $app --host "ssh-ng://chariot")
      (phase 3550 40 buildPhase)
      (stop 6200 40)
      (copy 41 (out "app-1.0") (3 * $mib) 6200 7600 --from-host "ssh-ng://chariot")
      (copy 42 (out "app-1.0") (8 * $mib) 6500 9000 --to-host "ssh://cache-push")
      (check 1500 both-directions)
      (check 4500 remote-build)
      (check 7200 result-and-push)
      (check 8000 after-download)
    ]
    drvs: { derivations: { $app: { sources: [$src $vendor] } } }
  }
}

# A hundred substitutions and nothing to build.
def "fixture substitution-heavy" [] {
  let paths = 0..99 | each {|k| out $"($words | get ($k mod ($words | length)))-lib-($k // ($words | length)).($k mod 9)" }
  mut slots = 0..9 | each { 100 }
  mut events = []
  for entry in ($paths | enumerate) {
    let size = pick $entry.index ($mib // 10) (50 * $mib)
    let slot = $slots | enumerate | sort-by item | first
    let from = $slot.item
    let to = $from + 400 + $size // 20000
    $events = $events ++ (substitution (1000 + $entry.index * 4) $entry.item $size $from $to)
    $slots = $slots | update $slot.index ($to + 30)
  }
  {
    events: [
      (plan-fetches 0 $paths "1.2 GiB" "4.8 GiB")
      $events
      (check 2000 early)
      (check 6000 busy)
      (check 12000 late)
    ]
  }
}

# Sources downloaded for planned consumers, one of them shared.
def "fixture source-fetches" [] {
  let app = drv "app-1.0"
  let lib = drv "lib-1.0"
  let tool = drv "tool-1.0"
  let bundle = drv "bundle"
  let app_src = out "source" "app-source"
  let shared = out "shared-source"
  let patches = out "patches"
  {
    events: [
      (plan-builds 0 [$app $lib $tool $bundle])
      (substitution 10 $app_src (20 * $mib) 300 4300)
      (substitution 20 $shared (6 * $mib) 400 3000)
      (substitution 30 $patches ($mib // 10) 450 800)
      (build 3100 40 $lib)
      (build 3150 41 $tool)
      (build 4400 42 $app)
      (stop 5500 41)
      (stop 5800 40)
      (stop 6500 42)
      (build 6600 43 $bundle)
      (stop 7400 43)
      (check 1500 fetching)
      (check 3500 shared-done)
      (check 5000 building)
    ]
    drvs: {
      derivations: {
        $app: { sources: [$app_src $patches] }
        $lib: { sources: [$shared] }
        $tool: { sources: [$shared] }
        $bundle: { inputs: [$app $lib $tool] }
      }
    }
  }
}

# ── Remote builds ───────────────────────────────────────────────────────────

def "fixture remote-build" [] {
  let hello = drv "hello-2.12.3" "remote-hello"
  {
    events: [
      (build 0 10 $hello --host "ssh://builder")
      (phase 50 10 buildPhase)
      (check 1500 building)
      (stop 2500 10)
    ]
  }
}

# Local and remote builds side by side, one waiting for a machine and one
# for a lock another process holds.
def "fixture remote-mix" [] {
  let local_a = drv "local-a-1.0"
  let local_b = drv "local-b-1.0"
  let remote_c = drv "remote-c-1.0"
  let remote_d = drv "remote-d-1.0"
  let remote_e = drv "remote-e-1.0"
  let waiting = drv "remote-f-1.0"
  let locked = drv "locked-g-1.0"
  let deploy = drv "deploy"
  let all = [$local_a $local_b $remote_c $remote_d $remote_e $waiting $locked]
  {
    events: [
      (plan-builds 0 ($all ++ [$deploy]))
      (build 0 10 $local_a)
      (phase 50 10 buildPhase)
      (build 200 11 $local_b)
      (phase 250 11 configurePhase)
      (build 300 12 $remote_c --host "ssh-ng://chariot")
      (phase 350 12 buildPhase)
      (build 400 13 $remote_d --host "ssh-ng://chariot")
      (phase 450 13 checkPhase)
      (build 500 14 $remote_e --host "ssh://eu-builder.example.org")
      (phase 550 14 buildPhase)
      (start 600 20 111 $"waiting for a machine to build '($waiting)'" [])
      (start 700 21 111 $"waiting for lock on '(out 'locked-g-1.0')'" [])
      (check 1500 mixed)
      (stop 3000 11)
      (stop 3500 21)
      (check 4000 waiting)
      (stop 4500 13)
      (stop 4600 20)
      (build 4600 15 $waiting --host "ssh-ng://chariot")
      (stop 5000 10)
      (stop 6000 12)
      (check 6500 draining)
      (stop 7000 14)
      (stop 7600 15)
      (build 7700 16 $deploy)
      (stop 8500 16)
    ]
    # Listing every derivation registers the output the lock names.
    drvs: { derivations: ($all | reduce --fold { $deploy: { inputs: $all } } {|drv, acc| $acc | insert $drv {} }) }
  }
}

# ── Logs and output ─────────────────────────────────────────────────────────

# Hundreds of log lines from one build while another stays quiet.
def "fixture chatty-build" [] {
  let chatty = drv "chatty-1.0"
  let quiet = drv "quiet-1.0"
  {
    events: [
      (build 0 10 $chatty)
      (phase 20 10 buildPhase)
      (build 100 11 $quiet)
      (1..400 | each {|k| log (40 + $k * 14) 10 $"[($k * 100 // 400 | fill --alignment right --width 3)%] Building CXX object src/CMakeFiles/core.dir/file_($k | fill --alignment right --width 3 --character '0').cpp.o" })
      (check 1500 chatting)
      (check 4000 still-chatting)
      (stop 5000 11)
      (stop 6000 10)
    ]
  }
}

# Build output that tries to move the cursor, plus wide and long lines.
def "fixture hostile-logs" [] {
  let hostile = drv "hostile-1.0"
  let lines = [
    "cursor up and erase:\u{1b}[1A\u{1b}[2K"
    "progress 10%\rprogress 50%\rprogress 100%"
    "save\u{1b}7 and restore\u{1b}8 the cursor"
    "reverse index\u{1b}M up a line"
    "clear the screen\u{1b}[2J\u{1b}[H"
    "alternate screen\u{1b}[?1049h and back\u{1b}[?1049l"
    "form\u{c}feed and vertical\u{b}tab"
    "\u{1b}]8;;https://example.org\u{7}a hyperlink\u{1b}]8;;\u{7}"
    "tab\tseparated\tcolumns"
    "\u{1b}[1;31mred\u{1b}[0m and \u{1b}[38;5;208morange\u{1b}[0m"
    "wide: 构建成功 ✅ 🚀 ｗｉｄｅ"
    ("long: " + (1..40 | each { "0123456789" } | str join))
  ]
  {
    events: [
      (build 0 10 $hostile)
      ($lines | enumerate | each {|l| log (1100 + $l.index * 50) 10 $l.item })
      (check 1500 after-logs)
      (stop 2500 10)
    ]
  }
}

# Lines that are not Nix records, around a build.
def "fixture passthrough" [] {
  let hello = drv "hello-2.12.3" "passthrough-hello"
  {
    events: [
      (at 0 "warning: Git tree '/home/user/src/app' is dirty")
      (build 50 10 $hello)
      (at 100 "evaluation warning: 'system' has been renamed to 'stdenv.hostPlatform.system'")
      (log 150 10 "unpacking sources")
      (check 1500 building)
      (stop 2500 10)
    ]
  }
}

# Activity types ROM does not know.
def "fixture unsupported-record" [] {
  let hello = drv "hello-2.12.3" "unsupported-hello"
  {
    events: [
      (build 0 10 $hello)
      (start 100 50 199 "a future activity" [] 10)
      (start 150 51 199 "the same future activity" [] 10)
      (check 1500 building)
      (stop 2500 10)
    ]
  }
}

# A name too long for its column.
def "fixture long-names" [] {
  let long = drv "python3.12-some-extraordinarily-long-package-name-with-many-words-2024.10.07"
  {
    events: [
      (build 0 10 $long)
      (phase 50 10 installCheckPhase)
      (check 1500 building)
      (stop 2500 10)
    ]
  }
}

# Names Nix allows that look unlike packages.
def "fixture odd-names" [] {
  let names = ["foo+bar=baz-1.0" "_private-0.1" "1.0" "a" "x86_64-unknown-linux-gnu-binutils-2.44" "nixos-rebuild-ng-26.11pre-git" "hello-2.12.3-dev"]
  {
    events: [
      ($names | enumerate | each {|n| build ($n.index * 30) (10 + $n.index) (drv $n.item) })
      (check 1500 building)
      ($names | enumerate | each {|n| stop (2500 + $n.index * 30) (10 + $n.index) })
    ]
  }
}

# Seventeen builds nested by activity alone, without derivation metadata.
def "fixture overflow" [] {
  let system = drv "system" "overflow-system"
  let names = [bash coreutils gcc glibc openssl zlib curl python3 perl git ncurses readline sqlite libffi xz gettext]
  {
    events: [
      (build 0 10 $system)
      ($names | enumerate | each {|n| build (10 + $n.index * 10) (11 + $n.index) (drv $n.item $"overflow-($n.item)") --parent 10 })
      ($names | enumerate | each {|n| phase 200 (11 + $n.index) buildPhase })
      (check 1300 all-running)
      ($names | enumerate | first 8 | each {|n| stop 1400 (11 + $n.index) })
      (check 1600 half-done)
      ($names | enumerate | skip 8 | each {|n| stop 1700 (11 + $n.index) })
      (stop 1800 10)
    ]
  }
}

# ── Edges ───────────────────────────────────────────────────────────────────

# Nothing to build or fetch.
def "fixture empty-run" [] {
  { events: [(check 500 idle)] }
}

# Everything finishes before anything earns a row.
def "fixture instant-finish" [] {
  let one = drv "one-1.0"
  let two = drv "two-1.0"
  let three = drv "three-1.0"
  {
    events: [
      (plan-builds 0 [$one $two $three])
      (build 0 10 $one)
      (stop 300 10)
      (build 300 11 $two)
      (substitution 20 (out "tiny-dep") ($mib // 4) 100 600)
      (stop 500 11)
      (build 500 12 $three)
      (check 650 almost-done)
      (stop 700 12)
    ]
    drvs: { derivations: { $three: { inputs: [$two] }, $two: { inputs: [$one] } } }
  }
}

# The stream ends with a build and a download still running.
def "fixture interrupted" [] {
  let hello = drv "hello-2.12.3" "interrupted-hello"
  let glibc = out "glibc-2.40" "interrupted-glibc"
  {
    events: [
      (build 0 10 $hello)
      (phase 50 10 configurePhase)
      (log 100 10 "configuring")
      (start 300 20 108 $"copying path '($glibc)' from '($cache)'" [$glibc $cache])
      (start 300 21 100 $"copying path '($glibc)' from '($cache)'" [$glibc $cache "local"] 20)
      (progress 800 21 (10 * $mib) (40 * $mib))
      (check 1500 building)
    ]
  }
}

# A build running past an hour, beyond its estimate. Too long to preview.
def "fixture long-run" [] {
  let chromium = drv "chromium-140.0.7339.80"
  {
    events: [
      (build 0 10 $chromium)
      (phase 100 10 configurePhase)
      (phase 600000 10 buildPhase)
      (check 1500 starting)
      (check 1800000 half-hour)
      (phase 3500000 10 installPhase)
      (check 3700000 overrun)
      (stop 3900000 10)
    ]
    history: [(report "chromium-140.0.7339.80" 3600) (report "chromium-140.0.7339.80" 3600)]
  }
}
