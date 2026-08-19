# ROM

Visual build monitor for Nix that transforms cryptic build logs into a clean,
real-time dependency graph. Think of it as `NOM`, but written in Rust with a
focus on speed, configurability, and showing you exactly what Nix is doing with
your builds.

Built with a modular parser under [`crates/cognos`](../crates/cognos) that handles
the ATerm and internal-json log formats from Nix.

> [!NOTE]
> ROM is still under active development. Things may break, output formats may
> change, and bugs are to be expected. If you encounter any issues, please
> report them!

## Usage

ROM is primarily designed to wrap the Nix installation on your system. As such,
the _recommended_ interface is using `rom build`, `rom shell` and `rom develop`
for their Nix counterparts.

<!--markdownlint-disable MD013-->

```terminal
$ rom -h
ROM - A Nix build output monitor

Usage: rom [OPTIONS] [COMMAND]

Commands:
  build    Run nix build with monitoring
  shell    Run nix shell with monitoring
  develop  Run nix develop with monitoring
  help     Print this message or the help of the given subcommand(s)

Options:
      --json                     Parse JSON output from nix --log-format=internal-json
      --silent                   Minimal output
      --format <FORMAT>          Output format: tree, plain, dashboard [default: tree]
      --legend <LEGEND>          Legend display style: compact, table, verbose [default: table]
      --summary <SUMMARY>        Summary display style: concise, table, full [default: concise]
      --log-prefix <LOG_PREFIX>  Log prefix style: short, full, none [default: short]
      --log-lines <LOG_LINES>    Maximum number of log lines to display
      --platform <PLATFORM>      Nix-family evaluator to use. Auto-detected by default
  -v...                          Increase verbosity; controls nix log level and rom diagnostic output. Repeatable: -v (info), -vv (debug), -vvv (trace)
  -h, --help                     Print help
  -V, --version                  Print version
```

<!--markdownlint-enable MD013-->

To build a package with Nix, let's say `pkgs.hello`, you can do:

```terminal
$ rom build nixpkgs#hello
┏━ Dependency Graph:
┃ ⏵ hello-2.12.2 (configurePhase) ⏱ 2s
┣━━━ Builds
┗━ ∑ ⏵ 1 │ ✔ 0 │ ✗ 0 │ ⏸ 4 │ ⏱ 2s
```

and the dependency tree will appear below. Each package in your closure appears
as a node, with spinners and timers showing real-time progress. When a build
finishes, you'll see a clear status with neat little glyphs.

### Argument Passthrough

At times, especially while you're calling ROM as a standalone executable, you
might need to pass additional flags to the Nix command being invoked. ROM allows
for this behaviour by accepting `--` as a delimiter and passing any arguments
that come after to Nix. For example:

```terminal
$ rom develop nixpkgs#hello -- --substituters ""
fetching git input 'git+file:///home/notashelf/Dev/notashelf/rom'
┗━ ⏵ 0 │ ✔ 2 │ ✗ 0 │ ⏸ 0 │ ⏱ 1s


notashelf@enyo ~/Dev/notashelf/rom [git:(9e83f57...) *]
i $ hello
Hello, world!
```

## FAQ

**Q**: If "NOM" is nix-output-monitor, what does "ROM stand for"?

**A**: It doesn't stand for anything, I named it _rom_ because it sounds like
_rum_. I like rum. However you may choose to name it "rusty output monitor" or
"raf's output monitor" at your convenience. I don't know, be creative.

## Attributions

This project is clearly inspired by the famous
<https://github.com/maralorn/nix-output-monitor>. I am a huge fan of NOM's
design, but I am a little disappointed by its lack of configurability. This is a
more flexible replacement that makes both my life, and displaying build graphs
easier.

The ATerm and internal-json log parser was inspired, and mostly copied from
<https://git.atagen.co/atagen/nous> with consolidation, cleaner repo layout, and
a better separation of concerns. rom builds on the ideas previously pondered by
nous, and provides a subcrate under [`crates/cognos`](../crates/cognos) for easy
parsing. Thank you Atagen for letting me play with the idea.

## License

<!--markdownlint-disable MD059-->

This project is made available under Mozilla Public License (MPL) version 2.0.
See [LICENSE](LICENSE) for more details on the exact conditions. An online copy
is provided [here](https://www.mozilla.org/en-US/MPL/2.0/).

<!--markdownlint-enable MD059-->
