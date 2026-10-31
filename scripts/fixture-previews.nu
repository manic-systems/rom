#!/usr/bin/env nu

# Before/after GIF previews of every fixture, which are posted on pull
# requests.
#
# NOTE: `record` and `compare` run on untrusted pull request code, we
# have to be careful with that.

const marker = "<!-- rom-fixture-previews -->"
const max_gif_bytes = 10MB
const max_fixtures = 100
# `<fixture>` or `<fixture>.<variant>`.
const name_pattern = '^[a-z0-9][a-z0-9-]{0,63}(\.[a-z0-9][a-z0-9-]{0,63})?$'
# Fixtures whose timeline is too long to record in real time.
const unpreviewed = [long-run]

def main [] { help main }

# Records every fixture directory in `fixtures` through `rom` into `out`.
#
# Each fixture and each of its variants gets a real-time `<name>.gif` and an
# instant-replay `<name>.txt`, whose bytes decide whether the output changed.
# Variants are named `<fixture>.<variant>`.
def "main record" [
  --rom: path                    # ROM binary
  --replay: path                 # `replay-fixture.nu` matching `rom`
  --fixtures: path               # directory of fixture directories
  --out: path                    # output directory
] {
  let fonts = $env.ROM_PREVIEW_FONTS? | default ""
  if $fonts == "" { error make { msg: "ROM_PREVIEW_FONTS is unset; run inside `nix develop .#previews`" } }
  mkdir $out
  let scratch = mktemp --directory
  # An empty config dir keeps a local `rom/config.toml` out of the recordings.
  with-env { XDG_CONFIG_HOME: ($scratch | path join config) } {
    ls $fixtures | where type == dir | get name | where { $in | path join build.log | path exists } | sort | each {|fixture|
      let base_name = $fixture | path basename
      if $base_name in $unpreviewed { return }
      let variants_file = $fixture | path join variants.toml
      let variants = if ($variants_file | path exists) { open $variants_file } else { {} }
      ["default" ...($variants | columns | where $it != "default")] | each {|variant|
        let name = if $variant == "default" { $base_name } else { $"($base_name).($variant)" }
        if $name !~ $name_pattern { error make { msg: $"fixture name ($name) must match ($name_pattern)" } }
        print --stderr $"recording ($name)"
        let terminal = $variants | get --optional $variant | get --optional terminal | default {}
        let size = $"($terminal.width? | default 100)x($terminal.height? | default 30)"
        # Replays from before variants existed take no `--variant`.
        let selected = if $variant == "default" { "" } else { $" --variant '($variant)'" }
        let replay_command = $"nu '($replay)' '($fixture)'($selected) --rom '($rom)'"
        let instant = $scratch | path join $"($name)-instant.cast"
        let timed = $scratch | path join $"($name).cast"
        record $instant $size $"($replay_command) --instant"
        let output = cast-output $instant
        # agg cannot draw a recording without output, and there is nothing to see.
        if $output == "" { print --stderr $"skipping ($name), which prints nothing"; return }
        record $timed $size $replay_command
        $output | save --raw --force ($out | path join $"($name).txt")
        (^agg --quiet --font-dir $fonts --font-family "JetBrainsMono Nerd Font Mono"
          --theme dracula --last-frame-duration 3 $timed ($out | path join $"($name).gif"))
      }
    }
  }
  rm --recursive $scratch
  null
}

def record [cast: path, size: string, command: string] {
  ^asciinema rec --quiet --headless --window-size $size --overwrite --command $command $cast
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

  # Variants are listed under their fixture, `default` first.
  let fixture = {|name| $name | split row "." | first }
  let variant = {|name| $name | split row "." | get --optional 1 | default "default" }
  let group = {|key, names| $names | where {|name| (do $fixture $name) == $key } }
  let shown = $changed ++ $added | each {|name| do $fixture $name } | uniq | sort
  let codes = {|names| $names | each { $"`($in)`" } | str join ", " }
  let blocks = $shown | each {|name|
    let changed = do $group $name $changed
    let added = do $group $name $added
    let unchanged = do $group $name $unchanged
    let removed = do $group $name $removed
    let total = [$changed $added $unchanged] | flatten | length
    let counts = [
      (if ($changed | is-not-empty) { $"($changed | length) of ($total) changed" })
      (if ($added | is-not-empty) { $"($added | length) added" })
      (if ($removed | is-not-empty) { $"($removed | length) removed" })
    ] | compact | str join ", "
    # A before column only when something has a before.
    let before = $changed | is-not-empty
    let header = if $before { "<th>variant</th><th>before</th><th>after</th>" } else { "<th>variant</th><th>preview</th>" }
    let rows = $changed ++ $added | sort-by {|full| do $variant $full | if $in == "default" { "" } else { $in } } | each {|full|
      let cells = if $full in $changed {
        [(do $gif $full base) (do $gif $full head)]
      } else if $before {
        ["" (do $gif $full head)]
      } else {
        [(do $gif $full head)]
      }
      $"<tr><td><code>(do $variant $full)</code></td>($cells | each { $"<td>($in)</td>" } | str join)</tr>"
    }
    let rest = [
      (if ($unchanged | is-not-empty) { $"Unchanged: (do $codes ($unchanged | each {|full| do $variant $full }))" })
      (if ($removed | is-not-empty) { $"Removed: (do $codes ($removed | each {|full| do $variant $full }))" })
    ] | compact
    [
      $"<details><summary><code>($name)</code>: ($counts)</summary>"
      $"<table><tr>($header)</tr>($rows | str join)</table>"
      ...$rest
      "</details>"
    ] | str join "\n\n"
  }
  # Fixtures without anything to show are only named, once each.
  let quiet = {|names| $names | each {|name| do $fixture $name } | where $it not-in $shown | uniq | sort }

  [
    $marker
    "## Fixture previews"
    (if $base_failed { "> [!WARNING]\n> The base failed to build or record, so HEAD can be shown." })
    (if not $base_failed and ($changed | is-empty) and ($added | is-empty) and ($removed | is-empty) { "No visual changes." })
    (if ($blocks | is-not-empty) {
      let title = if $base_failed { "Head" } else { "Changed" }
      $"### ($title) \(($shown | length)\)\n\n($blocks | str join "\n\n")"
    })
    (if (do $quiet $removed | is-not-empty) { $"### Removed \((do $quiet $removed | length)\)\n\n(do $codes (do $quiet $removed))" })
    (if (do $quiet $unchanged | is-not-empty) { $"Unchanged, skipped \((do $quiet $unchanged | length)\): (do $codes (do $quiet $unchanged))" })
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
