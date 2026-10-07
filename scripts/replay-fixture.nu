#!/usr/bin/env nu

const root = path self | path dirname | path dirname

# Replay a timed ROM fixture log through the real binary.
#
# Arguments after `--` are passed to ROM. ROM is never built implicitly.
def main [
  fixture: string                # fixture name or path to a `.log` file
  --rom: path                    # ROM binary (default: target/debug/rom)
  --speed: float = 1.0           # playback speed multiplier
  --instant                      # ignore timestamps
  --step                         # wait for a key at each checkpoint
  ...rom_args: string            # arguments passed to ROM
] {
  let log = if ($fixture | path exists) { $fixture } else {
    $root | path join rom tests fixtures $"($fixture).log"
  }
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

  let started = date now
  $events
  | each {|event|
      if not $instant {
        let due = ($event.ms | into int | into duration --unit ms) / $speed
        let wait = $due - ((date now) - $started)
        if $wait > 0sec { sleep $wait }
      }
      if ($event.line | str starts-with "= ") {
        if $step { input listen --types [key] | ignore }
      } else {
        $event.line
      }
    }
  | compact
  | to text
  | ^$rom ...$rom_args
}
