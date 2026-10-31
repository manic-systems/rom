#!/usr/bin/env nu

const root = path self | path dirname | path dirname

# Replay a timed ROM fixture log through the real binary.
#
# Arguments after `--` are passed to ROM. ROM is never built implicitly.
def main [
  fixture: string                # fixture name or path to a fixture directory
  --rom: path                    # ROM binary (default: target/debug/rom)
  --speed: float = 1.0           # playback speed multiplier
  --instant                      # ignore timestamps
  --step                         # wait for a key at each checkpoint
  ...rom_args: string            # arguments passed to ROM
] {
  let directory = if ($fixture | path exists) { $fixture } else {
    $root | path join rom tests fixtures $fixture
  }
  let log = $directory | path join build.log
  if not ($log | path exists) { error make { msg: $"fixture not found: ($fixture)" } }
  let rom = $rom | default ($root | path join target debug rom)
  if not ($rom | path exists) {
    error make { msg: $"ROM binary not found at ($rom); build it with `cargo build -p rom`" }
  }
  if $speed <= 0 { error make { msg: "--speed must be greater than zero" } }

  let events = open --raw $log | lines | parse --regex '^(?<ms>\d+) (?<line>.*)$'
  if $step {
    let names = $events | where line starts-with "= " | get line | str substring 2..
    print --stderr $"Step mode: press any key at each checkpoint: ($names | str join ', ')"
  }

  # Stepping reads keys straight from the terminal. cbreak mode with echo off
  # keeps output newline translation intact (raw mode would staircase ROM's
  # logs) and stops keypresses from being drawn into the live frame.
  let saved = if $step { ^sh -c 'stty -g < /dev/tty' | str trim } else { null }
  if $step { ^sh -c 'stty -icanon -echo min 1 < /dev/tty' }

  # Fixture derivations are not in the store, so their edges come from a
  # sidecar. A scratch state directory keeps the replay's build times out of
  # the real history and supplies the fixture's own.
  let derivations = $directory | path join derivations.json
  let state = mktemp --directory
  let history = $directory | path join history.csv
  if ($history | path exists) {
    mkdir ($state | path join rom)
    cp $history ($state | path join rom build-reports-v1.csv)
  }
  let fixture_env = {
    XDG_STATE_HOME: $state
    ROM_FIXTURE_DERIVATIONS: (if ($derivations | path exists) { $derivations | path expand } else { null })
  } | compact

  let started = date now
  let restore = {|| if $step { ^sh -c $"stty ($saved) < /dev/tty" } }
  try {
    $events
    | each {|event|
        if not $instant {
          let due = ($event.ms | into int | into duration --unit ms) / $speed
          let wait = $due - ((date now) - $started)
          if $wait > 0sec { sleep $wait }
        }
        if ($event.line | str starts-with "= ") {
          if $step { ^sh -c 'dd bs=1 count=1 < /dev/tty > /dev/null 2>&1' }
        } else {
          $event.line
        }
      }
    | compact
    | to text
    | with-env $fixture_env { ^$rom ...$rom_args }
  } catch {
    do $restore
    rm --recursive $state
    exit 1
  }
  do $restore
  rm --recursive $state
}
