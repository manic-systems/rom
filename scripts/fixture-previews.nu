#!/usr/bin/env nu

# Before/after GIF previews of every fixture, which are posted on pull
# requests.
#
# NOTE: `record` and `compare` run on untrusted pull request code, we
# have to be careful with that.

const marker = "<!-- rom-fixture-previews -->"
const max_gif_bytes = 10MB
const max_fixtures = 100
const name_pattern = '^[a-z0-9][a-z0-9-]{0,63}$'
# Fixtures whose timeline is too long to record in real time.
const unpreviewed = [long-run]

def main [] { help main }

# Records every fixture log in `fixtures` through `rom` into `out`.
#
# Each fixture gets a real-time `<name>.gif` and an instant-replay
# `<name>.txt`, whose bytes decide whether the output changed.
def "main record" [
  --rom: path                    # ROM binary
  --replay: path                 # `replay-fixture.nu` matching `rom`
  --fixtures: path               # directory of `.log` fixtures
  --out: path                    # output directory
] {
  let fonts = $env.ROM_PREVIEW_FONTS? | default ""
  if $fonts == "" { error make { msg: "ROM_PREVIEW_FONTS is unset; run inside `nix develop .#previews`" } }
  mkdir $out
  let scratch = mktemp --directory
  # An empty config dir keeps a local `rom/config.toml` out of the recordings.
  with-env { XDG_CONFIG_HOME: ($scratch | path join config) } {
    ls ($fixtures | path join "*.log" | into glob) | get name | sort | each {|log|
      let name = $log | path parse | get stem
      if $name in $unpreviewed { return }
      if $name !~ $name_pattern { error make { msg: $"fixture name ($name) must match ($name_pattern)" } }
      print --stderr $"recording ($name)"
      let instant = $scratch | path join $"($name)-instant.cast"
      let timed = $scratch | path join $"($name).cast"
      record $instant $"nu '($replay)' '($log)' --rom '($rom)' --instant"
      record $timed $"nu '($replay)' '($log)' --rom '($rom)'"
      cast-output $instant | save --raw --force ($out | path join $"($name).txt")
      (^agg --quiet --font-dir $fonts --font-family "JetBrainsMono Nerd Font Mono"
        --theme dracula --last-frame-duration 3 $timed ($out | path join $"($name).gif"))
    }
  }
  rm --recursive $scratch
  null
}

def record [cast: path, command: string] {
  ^asciinema rec --quiet --headless --window-size 100x30 --overwrite --command $command $cast
}

# Concatenates a cast's output events, dropping their timing.
def cast-output [cast: path] {
  open --raw $cast | lines | skip 1 | each { from json } | where {|event| $event.1 == "o" } | each { get 2 } | str join
}

# Compares `base` and `head` recordings into `out/manifest.json` plus the GIFs
# the comment needs.
def "main compare" [
  --base: path                   # base recordings, missing if the base failed
  --head: path                   # head recordings
  --out: path                    # artifact directory
] {
  let base_failed = not ($base | path exists)
  let names = {|dir| if ($dir | path exists) { ls ($dir | path join "*.txt" | into glob) | get name | each { path parse | get stem } } else { [] } }
  let base_names = do $names $base
  let head_names = do $names $head
  let shared = $head_names | where $it in $base_names
  let differs = {|name| (open --raw ($base | path join $"($name).txt")) != (open --raw ($head | path join $"($name).txt")) }
  let manifest = {
    base_failed: $base_failed
    changed: ($shared | where {|name| do $differs $name })
    unchanged: ($shared | where {|name| not (do $differs $name) })
    added: ($head_names | where $it not-in $base_names)
    removed: ($base_names | where $it not-in $head_names)
  }
  mkdir $out
  for name in $manifest.changed {
    cp ($base | path join $"($name).gif") ($out | path join $"($name).base.gif")
    cp ($head | path join $"($name).gif") ($out | path join $"($name).head.gif")
  }
  for name in $manifest.added {
    cp ($head | path join $"($name).gif") ($out | path join $"($name).head.gif")
  }
  $manifest | to json | save --force ($out | path join manifest.json)
}

# Validates an untrusted artifact, copies its GIFs into `dest` and prints the
# comment body linking to them under `url`.
def "main comment" [
  --artifact: path               # downloaded artifact directory
  --dest: path                   # directory on the previews branch
  --url: string                  # raw URL of `dest`
] {
  let manifest = open --raw ($artifact | path join manifest.json) | from json
  let list = {|key|
    let value = $manifest | get --optional $key
    if ($value | describe | str starts-with "list") and ($value | all {|name| ($name | describe) == "string" and $name =~ $name_pattern }) {
      $value
    } else {
      error make { msg: $"manifest.($key) must be a list of fixture names" }
    }
  }
  let changed = do $list changed
  let unchanged = do $list unchanged
  let added = do $list added
  let removed = do $list removed
  let base_failed = $manifest | get --optional base_failed
  if ($base_failed | describe) != "bool" { error make { msg: "manifest.base_failed must be a bool" } }
  if ([$changed $unchanged $added $removed] | flatten | length) > $max_fixtures {
    error make { msg: "manifest lists too many fixtures" }
  }

  mkdir $dest
  let gif = {|name, side|
    let file = $"($name).($side).gif"
    let source = $artifact | path join $file
    if (($source | path type) != "file") { error make { msg: $"missing ($file)" } }
    if (ls $source | get 0.size) > $max_gif_bytes { error make { msg: $"($file) is too large" } }
    if (open --raw $source | first 6 | decode) not-in ["GIF87a" "GIF89a"] { error make { msg: $"($file) is not a GIF" } }
    cp $source ($dest | path join $file)
    $"<img src=\"($url)/($file)\" alt=\"($name) ($side)\">"
  }

  let changed_blocks = $changed | each {|name|
    let before = do $gif $name base
    let after = do $gif $name head
    $"<details><summary><code>($name)</code></summary>\n\n**Before**\n\n($before)\n\n**After**\n\n($after)\n\n</details>"
  }
  let added_blocks = $added | each {|name|
    $"<details><summary><code>($name)</code></summary>\n\n(do $gif $name head)\n\n</details>"
  }
  let codes = {|names| $names | each { $"`($in)`" } | str join ", " }

  [
    $marker
    "## Fixture previews"
    (if $base_failed { "> [!WARNING]\n> The base failed to build or record, so HEAD can be shown." })
    (if not $base_failed and ($changed | is-empty) and ($added | is-empty) and ($removed | is-empty) { "No visual changes." })
    (if ($changed | is-not-empty) { $"### Changed \(($changed | length)\)\n\n($changed_blocks | str join "\n\n")" })
    (if ($added | is-not-empty) {
      let title = if $base_failed { "Head" } else { "Added" }
      $"### ($title) \(($added | length)\)\n\n($added_blocks | str join "\n\n")"
    })
    (if ($removed | is-not-empty) { $"### Removed \(($removed | length)\)\n\n(do $codes $removed)" })
    (if ($unchanged | is-not-empty) { $"Unchanged, skipped \(($unchanged | length)\): (do $codes $unchanged)" })
  ] | compact | str join "\n\n"
}

# Creates or updates the sticky preview comment on pull request `pr`.
def "main post" [
  --repo: string                 # owner/name
  --pr: int                      # pull request number
  --body: path                   # comment body
] {
  let existing = ^gh api --paginate $"repos/($repo)/issues/($pr)/comments" --jq $".[] | select\(.user.login == \"github-actions[bot]\" and \(.body | startswith\(\"($marker)\"\)\)\) | .id"
    | lines
  let payload = { body: (open --raw $body) } | to json
  if ($existing | is-empty) {
    $payload | ^gh api --silent --method POST $"repos/($repo)/issues/($pr)/comments" --input -
  } else {
    $payload | ^gh api --silent --method PATCH $"repos/($repo)/issues/comments/($existing | first)" --input -
  }
}
