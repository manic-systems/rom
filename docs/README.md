<!-- markdownlint-disable MD033 MD041 -->
<div align="center">
  <h1 id="header">rom</h1>
  <a alt="CI" href="https://github.com/manic-systems/rom/actions">
    <img
      src="https://github.com/manic-systems/rom/actions/workflows/ci.yml/badge.svg"
      alt="Build Status"
    />
  </a>
  <a alt="Deps" href="https://deps.rs/repo/github/manic-systems/rom">
    <img
      src="https://deps.rs/repo/github/manic-systems/rom/status.svg"
      alt="Dependency Status"
    />
  </a>
  <a alt="License" href="https://github.com/manic-systems/rom/blob/master/LICENSE">
    <img
      src="https://img.shields.io/github/license/manic-systems/rom?label=License"
      alt="License"
    />
  </a>
  <br/>
  <br/>
  <a href="#what-does-it-do">Synopsis</a><br/>
  <a href="#features">Features</a> | <a href="#usage">Usage</a><br/>
  <a href="#hacking">Contributing</a>
  <br/>
</div>

---

> **rom** /`rɒm`/ _noun_
>
> 1. A Nix build output monitor for hopeless **rom**antics.
> 2. A flamboyant output monitor for your Nix builds.

---

## What Does it Do?

TL;DR: Nix in, nice output out.

rom is the ultimate output monitor for your Nix builds. It does so by reading
and parsing Nix's build logs and displaying them in a nice-to-look at graph!

<!-- TODO: add a nice picture here -->

To get started with rom, skip to the [Usage](#usage) section.

## Features

- **Dependency-tree visualization of builds and their inputs**: Great-looking
  visualization of the whole build tree of a Nix build.
- **Local and remote builders**: Remote builds show up nicely in rom, and you'll
  get information about where your builds run, without compromises.
- **Dynamic derivations and content-addressed derivations**: Both
  content-addressed and dynamic derivations are explicitly supported, we display
  all the extra information that Nix build logs give us on them to you too!
- **Excellent Documentation**: Everything you can do with rom is documented.
  Everything rom _does_ is documented. The user-facing and developer-facing
  documentation is, and will always remain, up to date.
- **Eye candy**: We aim to make rom look _amazing_. If you think anything does
  not look _amazing_ and think you can do better, please refer to the
  [Contributing](#hacking) section or tell us.
- **Build phases, elapsed timers, and progress bars**
- **Historical duration estimates**
- **Linux cgroup CPU telemetry**.
- **Download and upload tracking**: We track both downloads and upload stats
  and give you byte counts, progress, remote hosts, _and_ aggregation of all of
  these! How nice is that!

## Status

[update request]: https://github.com/NixOS/nixpkgs/issues

<a href="https://repology.org/project/rom/versions">
    <img
      src="https://repology.org/badge/vertical-allrepos/rom.svg"
      alt="Packaging status"
      align="right"
      style="padding-left: 20px"
    >
</a>

Rom is packaged in nixpkgs, and is available only under nixpkgs unstable for
now; nixpkgs-26.11 will be the first commit including it (but who needs stable
anyways, those are for horses) Outside of extreme circumstances or breakign
changes, all updates will be backported to the stable branch. Refer to the
[installation](#installation) section for more details. Make sure you submit an
[update request] in Nixpkgs if the package is outdated.

### Installation

The latest, tagged version is available in Nixpkgs as **rom stable**. This is
recommended for most users, as tagged releases will usually undergo more
testing. This repository also provides the latest development version of rom,
which you can get from the flake outputs.

```sh
nix shell nixpkgs#rom # stable
nix shell github:manic-systems/rom # dev
```

You _can_ of course try rom in a Nix shell today, no setup required!

```sh
nix develop
cargo build
```

## Usage

Put `rom` in place of the Nix command you would have run anyway:

```sh
rom build nixpkgs#hello
rom build .#myPackage -- --no-link
rom shell .#myShell
rom develop . -- --impure
```

Other than nice output, rom behaves exactly like Nix!

Arguments after `--` get forwarded and go to Nix untouched, `build` returns the
exit status of the build it wraps, and `shell` and `develop` build the
environment under rom first, then drop you into the real shell.

Any other subcommand is monitored too:

```sh
rom eval .#version
rom flake check
rom run .#app -- --help
```

### Piping

Without a command, rom reads a log from standard input. Lines starting with
`@nix` are drawn, and everything else passes through byte for byte:

```sh
nix build --log-format internal-json nixpkgs#hello 2>&1 | rom
```

Use `--json` for a stream of bare internal-JSON records instead.

### Options

| Option                             | Default   | What it changes                            |
| ---------------------------------- | --------- | ------------------------------------------ |
| `--format tree\|plain\|dashboard`  | `tree`    | dependency graph, one line each, or totals |
| `--legend compact\|table\|verbose` | `table`   | the status counts under the graph          |
| `--summary concise\|table\|full`   | `concise` | the summary when the build ends            |
| `--icons nerd\|unicode`            | `nerd`    | Nerd Font glyphs, or plain Unicode         |
| `--log-prefix short\|full\|none`   | `short`   | what precedes each builder log line        |
| `--log-lines N`                    | no limit  | builder log lines shown per build          |
| `--platform nix\|lix`              | detected  | which evaluator to run                     |
| `--silent`                         |           | hide logs and the graph, keep errors       |
| `-v`, `-vv`, …                     |           | more diagnostics from Nix and rom          |

> [!NOTE]
> [`FORMATS.md`](./FORMATS.md) shows you what each format looks like.

### Configuration

The display options above, from `--format` to `--log-lines`, can also live in
`$XDG_CONFIG_HOME/rom/config.toml` (usually `~/.config/rom/config.toml`), under
the same names:

```toml
format = "tree"
icons = "unicode"
log-lines = 20
```

> [!NOTE]
> Flags on the command line win over the file.

rom remembers how long your builds took in
`$XDG_STATE_HOME/rom/build-reports-v1.csv`, keeping the last ten runs per
derivation and host, and uses their median to estimate how long the next one
will take.

## Hacking

Contributions are always welcome. To get started, just clone the repository and
run `nix develop`. We also provide a `.envrc` for Direnv users, who may use
`direnv allow` to enter a shell with the necessary dependencies.

### Structure

rom is written in the Rust programming language, and consists of two crates,
`cognos` and `rom` itself.

### Submitting Changes

Once your changes are complete, please remember to run `nix flake check` to
apply general formatter and linter rules that will be expected by the CI.

You will also want to update the [changelog](/CHANGELOG.md) with sufficient
amount of information to detail the new behaviour before you create your
changes.

This might seem daunting, but it isn't. Even if you _don't_ meet those
requirements, you'll be gently nudged to make your changes. Friendly
contributions are always welcome.

## Attributions

[nixos-output-monitor]: https://github.com/maralorn/nix-output-monitor
[nous]: https://git.atagen.co/atagen/nous

rom initially started as a replacement for [nixos-output-monitor] (hereby
further referred to as _nom_), and while it has expanded from there, we thank
maralorn and the other nom contributors dearly for their great work. Comparing
ourselves to and trying to match nom's quality has been a major part of initial
rom development.

Conos's ATerm and internal-JSON parser were heavily inspired by
[nous]. Thank you @atagen!

## License

ROM and Cognos are licensed under the European Union Public Licence v. 1.2
([EUPL-1.2](https://joinup.ec.europa.eu/collection/eupl/eupl-text-eupl-12)). See
[`LICENSE`](./../LICENSE).
